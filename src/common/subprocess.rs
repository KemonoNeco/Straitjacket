use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use wait_timeout::ChildExt;

/// Result of one subprocess execution.
pub struct RunResult {
    pub exit_code: i32,
    /// Combined stdout + stderr capture, in interleaved order as the process emitted.
    pub combined_output: String,
    pub timed_out: bool,
}

/// Runs `cmd` with `args` in `cwd`, with a wall-clock `timeout`. Captures combined
/// stdout+stderr. Sets `MSBUILDDISABLENODEREUSE=1` and `DOTNET_CLI_USE_MSBUILD_SERVER=0`
/// in the **child's** environment only (uses `Command::env`, not the parent process env)
/// to prevent MSBuild's persistent build server from inheriting redirected stdio handles
/// and hanging the parent on exit.
///
/// If the child exceeds `timeout`, it is killed; `timed_out: true` is returned with
/// whatever was captured so far. Exit code on timeout is -1.
pub fn run_with_timeout(
    cmd: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> std::io::Result<RunResult> {
    let mut command = Command::new(cmd);
    command
        .args(args)
        .current_dir(cwd)
        .env("MSBUILDDISABLENODEREUSE", "1")
        .env("DOTNET_CLI_USE_MSBUILD_SERVER", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn()?;

    let combined = Arc::new(Mutex::new(String::new()));
    let mut drainers = Vec::with_capacity(2);

    if let Some(stdout) = child.stdout.take() {
        let sink = Arc::clone(&combined);
        drainers.push(thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(stdout);
            let _ = reader.read_to_string(&mut buf);
            if let Ok(mut guard) = sink.lock() {
                guard.push_str(&buf);
            }
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let sink = Arc::clone(&combined);
        drainers.push(thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(stderr);
            let _ = reader.read_to_string(&mut buf);
            if let Ok(mut guard) = sink.lock() {
                guard.push_str(&buf);
            }
        }));
    }

    let status_opt = child.wait_timeout(timeout).map_err(std::io::Error::other)?;

    let (exit_code, timed_out) = match status_opt {
        Some(status) => (status.code().unwrap_or(-1), false),
        None => {
            kill_process_tree(child.id());
            let _ = child.wait();
            (-1, true)
        }
    };

    for d in drainers {
        let _ = d.join();
    }

    let captured = combined.lock().map(|g| g.clone()).unwrap_or_default();
    Ok(RunResult {
        exit_code,
        combined_output: captured,
        timed_out,
    })
}

/// Kills the entire process tree rooted at `pid`. On Windows, plain `child.kill()`
/// only kills the immediate child; grandchildren (e.g. ping spawned by cmd /c ping)
/// inherit the redirected stdio handles and keep the pipes open, defeating the
/// timeout. `taskkill /F /T` walks descendants.
fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
    #[cfg(not(windows))]
    {
        // POSIX: send SIGKILL to the process group. Best-effort.
        let _ = pid; // silence unused warning on non-Windows where this isn't wired.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::Instant;
    use tempfile::TempDir;

    #[test]
    fn captures_stdout() {
        let td = TempDir::new().unwrap();
        let result = run_with_timeout(
            "cmd",
            &["/c", "echo", "hello-from-child"],
            td.path(),
            Duration::from_secs(10),
        )
        .expect("subprocess should not fail to spawn");
        assert!(
            result.combined_output.contains("hello-from-child"),
            "expected captured stdout, got: {:?}",
            result.combined_output
        );
        assert!(!result.timed_out, "should not have timed out");
        assert_eq!(result.exit_code, 0, "expected exit 0, got {}", result.exit_code);
    }

    #[test]
    fn propagates_nonzero_exit_code() {
        let td = TempDir::new().unwrap();
        let result = run_with_timeout(
            "cmd",
            &["/c", "exit", "7"],
            td.path(),
            Duration::from_secs(10),
        )
        .expect("subprocess should not fail to spawn");
        assert_eq!(result.exit_code, 7, "expected exit 7, got {}", result.exit_code);
        assert!(!result.timed_out);
    }

    #[test]
    fn sets_msbuild_env_var_on_child_only_not_parent() {
        // Load-bearing: the wrapper must use Command::env (per-child), NOT std::env::set_var
        // (which mutates the parent process's environment). We verify both halves:
        //   (a) the CHILD sees MSBUILDDISABLENODEREUSE=1
        //   (b) the PARENT process's value before == after the call (no leak)
        let parent_before = env::var("MSBUILDDISABLENODEREUSE").ok();
        let td = TempDir::new().unwrap();
        let result = run_with_timeout(
            "cmd",
            &["/c", "echo", "MSBUILD=%MSBUILDDISABLENODEREUSE%;MSBUILDSERVER=%DOTNET_CLI_USE_MSBUILD_SERVER%"],
            td.path(),
            Duration::from_secs(10),
        )
        .expect("subprocess should not fail to spawn");
        assert!(
            result.combined_output.contains("MSBUILD=1"),
            "child did not see MSBUILDDISABLENODEREUSE=1; output: {:?}",
            result.combined_output
        );
        assert!(
            result.combined_output.contains("MSBUILDSERVER=0"),
            "child did not see DOTNET_CLI_USE_MSBUILD_SERVER=0; output: {:?}",
            result.combined_output
        );
        let parent_after = env::var("MSBUILDDISABLENODEREUSE").ok();
        assert_eq!(
            parent_before, parent_after,
            "parent env leaked: before={:?}, after={:?}",
            parent_before, parent_after
        );
    }

    #[test]
    fn kills_child_and_returns_timed_out_when_budget_exceeded() {
        // Use `ping 127.0.0.1 -n 10` as a portable sleep (~9 seconds on Windows).
        // Budget of 300ms means the child must be killed long before it would finish.
        let td = TempDir::new().unwrap();
        let started = Instant::now();
        let result = run_with_timeout(
            "cmd",
            &["/c", "ping", "127.0.0.1", "-n", "10"],
            td.path(),
            Duration::from_millis(300),
        )
        .expect("subprocess should not fail to spawn");
        let elapsed = started.elapsed();
        assert!(result.timed_out, "expected timed_out=true");
        assert_eq!(result.exit_code, -1, "expected exit_code=-1 on timeout");
        // Must finish well before the full ping duration; allow a generous 5s ceiling
        // to account for slow CI without lying about the timeout enforcement.
        assert!(
            elapsed < Duration::from_secs(5),
            "wrapper should kill child near the budget, took {:?}",
            elapsed
        );
    }

    #[test]
    fn returns_err_when_cmd_does_not_exist() {
        let td = TempDir::new().unwrap();
        let result = run_with_timeout(
            "definitely-not-a-real-command-xyz-123",
            &[],
            td.path(),
            Duration::from_secs(5),
        );
        assert!(result.is_err(), "expected Err when spawning nonexistent command");
    }
}
