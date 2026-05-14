use crate::common::subprocess::{run_with_timeout, RunResult};
use crate::common::Stack;
use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long, value_enum)]
    pub stack: Stack,
    #[arg(long)]
    pub log_dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaselineCheckResult {
    pub passed: bool,
    pub failing_tests: Vec<String>,
    pub log_path: PathBuf,
}

/// Parses failing Rust test names from `cargo test` output.
///
/// cargo emits lines like `test foo::bar ... FAILED`. We capture the second
/// whitespace-separated token.
pub fn parse_rust_failing_tests(output: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)^test\s+(\S+)\s+\.\.\.\s+FAILED").expect("valid regex");
    re.captures_iter(output)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Parses failing C# test names from `dotnet test` output.
///
/// dotnet test emits lines like `  Failed Foo.Bar.Baz [12 ms]`. We capture
/// the second whitespace-separated token after "Failed".
pub fn parse_csharp_failing_tests(output: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)^\s*Failed\s+(\S+)").expect("valid regex");
    re.captures_iter(output)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn append_section(log_path: &Path, header: &str, body: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("open log {}", log_path.display()))?;
    writeln!(f, "===== {} =====", header)?;
    f.write_all(body.as_bytes())?;
    writeln!(f)?;
    Ok(())
}

fn run_cargo_test(repo_root: &Path, log_path: &Path) -> anyhow::Result<RunResult> {
    let r = run_with_timeout(
        "cargo",
        &["test", "--workspace", "--no-fail-fast"],
        repo_root,
        Duration::from_secs(900),
    )
    .context("invoke cargo test")?;
    append_section(
        log_path,
        &format!("cargo test --workspace (exit {})", r.exit_code),
        &r.combined_output,
    )?;
    Ok(r)
}

fn run_dotnet_test(repo_root: &Path, log_path: &Path) -> anyhow::Result<RunResult> {
    let r = run_with_timeout(
        "dotnet",
        &["test", "--nologo", "--verbosity", "minimal"],
        repo_root,
        Duration::from_secs(900),
    )
    .context("invoke dotnet test")?;
    append_section(
        log_path,
        &format!("dotnet test (exit {})", r.exit_code),
        &r.combined_output,
    )?;
    Ok(r)
}

pub fn baseline_check(
    repo_root: &Path,
    stack: Stack,
    log_dir: &Path,
) -> anyhow::Result<BaselineCheckResult> {
    fs::create_dir_all(log_dir)
        .with_context(|| format!("create log dir {}", log_dir.display()))?;
    let log_path = log_dir.join("baseline.log");
    fs::write(
        &log_path,
        format!("Baseline check started {}\n", chrono::Utc::now().to_rfc3339()),
    )?;

    let mut failures: Vec<String> = Vec::new();
    let mut all_passed = true;

    if matches!(stack, Stack::Rust | Stack::Both) {
        let r = run_cargo_test(repo_root, &log_path)?;
        if r.exit_code != 0 {
            all_passed = false;
            let parsed = parse_rust_failing_tests(&r.combined_output);
            if parsed.is_empty() {
                failures.push(format!("rust:cargo-test-failed-exit-{}", r.exit_code));
            } else {
                for t in parsed {
                    failures.push(format!("rust:{}", t));
                }
            }
        }
    }

    if matches!(stack, Stack::Csharp | Stack::Both) {
        let r = run_dotnet_test(repo_root, &log_path)?;
        if r.exit_code != 0 {
            all_passed = false;
            let parsed = parse_csharp_failing_tests(&r.combined_output);
            if parsed.is_empty() {
                failures.push(format!("csharp:dotnet-test-failed-exit-{}", r.exit_code));
            } else {
                for t in parsed {
                    failures.push(format!("csharp:{}", t));
                }
            }
        }
    }

    Ok(BaselineCheckResult {
        passed: all_passed,
        failing_tests: failures,
        log_path,
    })
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let r = baseline_check(&args.repo_root, args.stack, &args.log_dir)?;
    println!("{}", serde_json::to_string_pretty(&r)?);
    if r.passed {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_failing_tests_extracted_from_cargo_output() {
        let output = "running 3 tests\n\
                      test foo::bar ... ok\n\
                      test foo::baz ... FAILED\n\
                      test foo::quux ... FAILED\n\
                      \n\
                      failures:\n\
                      ";
        let f = parse_rust_failing_tests(output);
        assert_eq!(f, vec!["foo::baz".to_string(), "foo::quux".to_string()]);
    }

    #[test]
    fn rust_no_failures_returns_empty() {
        let output = "test foo::a ... ok\ntest foo::b ... ok\n";
        let f = parse_rust_failing_tests(output);
        assert!(f.is_empty());
    }

    #[test]
    fn rust_handles_module_paths_with_double_colon() {
        let output = "test parser::header::parse_truncated ... FAILED\n";
        let f = parse_rust_failing_tests(output);
        assert_eq!(f, vec!["parser::header::parse_truncated"]);
    }

    #[test]
    fn csharp_failing_tests_extracted_from_dotnet_output() {
        let output = "Test Run Successful.\n\
                      Failed Foo.BarTest.A [10ms]\n\
                      Failed Foo.BarTest.B [12ms]\n\
                      Total: 5, Failed: 2, Passed: 3\n";
        let f = parse_csharp_failing_tests(output);
        assert_eq!(
            f,
            vec!["Foo.BarTest.A".to_string(), "Foo.BarTest.B".to_string()]
        );
    }

    #[test]
    fn csharp_handles_leading_whitespace() {
        let output = "  Failed Foo.BarTest.A\n    Failed Foo.BarTest.B\n";
        let f = parse_csharp_failing_tests(output);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn csharp_no_failures_returns_empty() {
        let output = "Test Run Successful. Total: 3, Failed: 0, Passed: 3\n";
        let f = parse_csharp_failing_tests(output);
        assert!(f.is_empty());
    }

    #[test]
    fn test_parse_rust_failing_tests_ignores_panic_messages_and_unanchored_failed_text() {
        // Panic message ("assertion failed:"), a finished-line with FAILED mid-string,
        // and a summary line "test ... FAILED" that is not anchored to line start must
        // NOT contribute entries. Only the canonical `^test <name> ... FAILED` line counts.
        let output = "thread 'main' panicked: assertion failed: x == y\n\
                      Finished test target FAILED\n\
                      test foo::real ... FAILED\n";
        let parsed = parse_rust_failing_tests(output);
        assert_eq!(parsed, vec!["foo::real".to_string()]);
    }
}
