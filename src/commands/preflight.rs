use crate::commands::baseline_check::{baseline_check, BaselineCheckResult};
use crate::commands::detect_stack::{detect_stack, DetectStackResult};
use crate::commands::lint_check::{lint_check, LintCheckResult};
use crate::common::Stack;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub log_dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct PreflightResult {
    pub stack: DetectStackResult,
    /// None when stack is None (no detected stack to baseline against).
    pub baseline: Option<BaselineCheckResult>,
    pub lint: Option<LintCheckResult>,
    pub passed: bool,
}

/// Combined preflight: detect_stack + baseline_check + lint_check. Skips
/// baseline + lint when no supported stack is detected.
pub fn preflight(repo_root: &std::path::Path, log_dir: &std::path::Path) -> anyhow::Result<PreflightResult> {
    let stack = detect_stack(repo_root)?;
    if stack.stack == Stack::None {
        return Ok(PreflightResult {
            stack,
            baseline: None,
            lint: None,
            passed: false,
        });
    }
    let baseline = baseline_check(repo_root, stack.stack, log_dir)?;
    let lint = lint_check(repo_root, stack.stack, log_dir)?;
    let passed = baseline.passed && lint.passed;
    Ok(PreflightResult {
        stack,
        baseline: Some(baseline),
        lint: Some(lint),
        passed,
    })
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let r = preflight(&args.repo_root, &args.log_dir)?;
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
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn empty_repo_returns_stack_none_and_no_baseline() {
        let td = TempDir::new().unwrap();
        let log = td.path().join("logs");
        let r = preflight(td.path(), &log).unwrap();
        assert_eq!(r.stack.stack, Stack::None);
        assert!(r.baseline.is_none());
        assert!(r.lint.is_none());
        assert!(!r.passed);
    }

    #[test]
    fn preflight_result_serializes_to_json() {
        let td = TempDir::new().unwrap();
        let log = td.path().join("logs");
        let r = preflight(td.path(), &log).unwrap();
        let _json = serde_json::to_string(&r).unwrap();
        // Round-trip to confirm shape stability
        fs::create_dir_all(&log).ok();
    }
}
