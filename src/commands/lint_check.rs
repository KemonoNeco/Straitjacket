use crate::commands::detect_stack::detect_stack;
use crate::common::cargo_target::{cargo_invocation, resolve_cargo_target, CargoInvocation};
use crate::common::subprocess::{run_with_timeout, RunResult};
use crate::common::Stack;
use anyhow::Context;
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
pub struct Diagnostic {
    pub step: String,
    pub excerpt: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LintCheckResult {
    pub passed: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub log_path: PathBuf,
}

/// Returns the first `max_lines` lines of the given output, joined with `\n`.
/// Used to produce a compact diagnostic excerpt for the JSON output.
pub fn extract_excerpt(output: &str, max_lines: usize) -> String {
    output
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
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

fn run_step(
    label: &str,
    cmd: &str,
    args: &[&str],
    cwd: &Path,
    log_path: &Path,
) -> anyhow::Result<RunResult> {
    let r = run_with_timeout(cmd, args, cwd, Duration::from_secs(900))
        .with_context(|| format!("invoke {}", label))?;
    append_section(log_path, &format!("{} (exit {})", label, r.exit_code), &r.combined_output)?;
    Ok(r)
}

fn dotnet_format_available() -> bool {
    std::process::Command::new("dotnet")
        .args(["format", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn lint_check(
    repo_root: &Path,
    stack: Stack,
    log_dir: &Path,
) -> anyhow::Result<LintCheckResult> {
    fs::create_dir_all(log_dir)
        .with_context(|| format!("create log dir {}", log_dir.display()))?;
    let log_path = log_dir.join("lint.log");
    fs::write(
        &log_path,
        format!("Lint check started {}\n", chrono::Utc::now().to_rfc3339()),
    )?;

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut all_passed = true;

    if matches!(stack, Stack::Rust | Stack::Both) {
        let manifests = detect_stack(repo_root)?.rust_manifests;
        let target = resolve_cargo_target(&manifests, repo_root);

        match cargo_invocation(&target, &["check", "--all-targets"]) {
            CargoInvocation::Run { cwd, args } => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let r1 = run_step("cargo check --all-targets", "cargo", &argv, &cwd, &log_path)?;
                if r1.exit_code != 0 {
                    all_passed = false;
                    diagnostics.push(Diagnostic {
                        step: "cargo check --all-targets".into(),
                        excerpt: extract_excerpt(&r1.combined_output, 50),
                    });
                }

                // Re-derive the clippy invocation from the same resolved target so it
                // runs from the identical cwd with --workspace inserted iff applicable.
                if let CargoInvocation::Run { cwd, args } =
                    cargo_invocation(&target, &["clippy", "--all-targets", "--", "-D", "warnings"])
                {
                    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                    let r2 = run_step(
                        "cargo clippy --all-targets -- -D warnings",
                        "cargo",
                        &argv,
                        &cwd,
                        &log_path,
                    )?;
                    if r2.exit_code != 0 {
                        all_passed = false;
                        diagnostics.push(Diagnostic {
                            step: "cargo clippy --all-targets -- -D warnings".into(),
                            excerpt: extract_excerpt(&r2.combined_output, 50),
                        });
                    }
                }
            }
            CargoInvocation::Skip => {
                // No Rust target — skip the rust lint steps without failing.
            }
            CargoInvocation::Ambiguous { candidates } => {
                all_passed = false;
                let joined = candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                diagnostics.push(Diagnostic {
                    step: "cargo-target-resolution".into(),
                    excerpt: format!(
                        "ambiguous: multiple crates with no root manifest: {}",
                        joined
                    ),
                });
            }
        }
    }

    if matches!(stack, Stack::Csharp | Stack::Both) {
        let r1 = run_step(
            "dotnet build --no-restore",
            "dotnet",
            &["build", "--no-restore", "--nologo", "--verbosity", "minimal"],
            repo_root,
            &log_path,
        )?;
        if r1.exit_code != 0 {
            all_passed = false;
            diagnostics.push(Diagnostic {
                step: "dotnet build --no-restore".into(),
                excerpt: extract_excerpt(&r1.combined_output, 50),
            });
        }

        if dotnet_format_available() {
            let r2 = run_step(
                "dotnet format --verify-no-changes",
                "dotnet",
                &["format", "--verify-no-changes", "--no-restore"],
                repo_root,
                &log_path,
            )?;
            if r2.exit_code != 0 {
                all_passed = false;
                diagnostics.push(Diagnostic {
                    step: "dotnet format --verify-no-changes".into(),
                    excerpt: extract_excerpt(&r2.combined_output, 50),
                });
            }
        } else {
            append_section(
                &log_path,
                "dotnet format probe",
                "dotnet format not installed; skipping format check",
            )?;
        }
    }

    Ok(LintCheckResult {
        passed: all_passed,
        diagnostics,
        log_path,
    })
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let r = lint_check(&args.repo_root, args.stack, &args.log_dir)?;
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
    fn excerpt_returns_first_n_lines() {
        let output = "line 1\nline 2\nline 3\nline 4\nline 5\n";
        assert_eq!(extract_excerpt(output, 3), "line 1\nline 2\nline 3");
    }

    #[test]
    fn excerpt_handles_fewer_lines_than_max() {
        let output = "only one";
        assert_eq!(extract_excerpt(output, 50), "only one");
    }

    #[test]
    fn excerpt_handles_empty_input() {
        assert_eq!(extract_excerpt("", 10), "");
    }

    #[test]
    fn excerpt_does_not_emit_trailing_newline() {
        let output = "a\nb\n";
        let r = extract_excerpt(output, 50);
        assert!(!r.ends_with('\n'));
    }
}
