use crate::common::json_io::{parse_work_units_array, read_json_file};
use crate::common::subprocess::{run_with_timeout, RunResult};
use crate::common::Stack;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    Pass,
    Fail,
}

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub work_units_file: PathBuf,
    #[arg(long, value_enum)]
    pub stack: Stack,
    #[arg(long)]
    pub log_dir: PathBuf,
    #[arg(long, default_value_t = 3)]
    pub runs: u32,
    #[arg(long, value_enum, default_value_t = Expect::Pass)]
    pub expect: Expect,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Pass,
    Fail,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    AllPass,
    AllFail,
    Flaky,
    NeverFound,
    /// TDD red-check passed: test failed every run, which is the desired outcome
    /// when `--expect fail` was specified (stub still in place, test pinning a
    /// behavior not yet implemented).
    RedOk,
    /// TDD red-check failed: test passed when it should have failed (likely vacuous).
    VacuousPreImpl,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct PerUnitResult {
    pub work_unit_id: String,
    pub output_test_name: String,
    pub output_file_path: String,
    pub per_run_statuses: Vec<TestStatus>,
    pub classification: Classification,
    pub recommended_status: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunNewTestsResult {
    pub runs: u32,
    pub summary: String,
    pub per_unit_results: Vec<PerUnitResult>,
    pub log_path: PathBuf,
}

/// Determines whether a Rust test by `name` passed, failed, or was not present in the output.
pub fn rust_test_status(output: &str, name: &str) -> TestStatus {
    let escaped = regex::escape(name);
    let fail_re = Regex::new(&format!(r"test\s+[\w:]*\b{}\s+\.\.\.\s+FAILED", escaped)).unwrap();
    let pass_re = Regex::new(&format!(r"test\s+[\w:]*\b{}\s+\.\.\.\s+ok", escaped)).unwrap();
    if fail_re.is_match(output) {
        TestStatus::Fail
    } else if pass_re.is_match(output) {
        TestStatus::Pass
    } else {
        TestStatus::Unknown
    }
}

/// Determines whether a C# test by `name` passed, failed, or was not present in the output.
pub fn csharp_test_status(output: &str, name: &str) -> TestStatus {
    let escaped = regex::escape(name);
    let fail_re =
        Regex::new(&format!(r"(?m)^\s*Failed\s+\S*\b{}\b", escaped)).unwrap();
    let pass_re =
        Regex::new(&format!(r"(?m)^\s*Passed\s+\S*\b{}\b", escaped)).unwrap();
    if fail_re.is_match(output) {
        TestStatus::Fail
    } else if pass_re.is_match(output) {
        TestStatus::Pass
    } else {
        TestStatus::Unknown
    }
}

/// Classifies a unit's per-run results into one of the documented classifications,
/// taking `expect` into account (Expect::Fail inverts the success interpretation
/// for TDD red-check).
pub fn classify(statuses: &[TestStatus], expect: Expect, runs: u32) -> (Classification, String) {
    // Zero-runs is a degenerate case: the test was never executed, so we have no signal.
    // Without this guard, `pass_count == runs` evaluates `0 == 0` and silently green-washes
    // an unrun test as AllPass/"written".
    if runs == 0 {
        return (Classification::NeverFound, "quarantined".into());
    }
    let pass_count = statuses.iter().filter(|s| **s == TestStatus::Pass).count() as u32;
    let fail_count = statuses.iter().filter(|s| **s == TestStatus::Fail).count() as u32;
    let unknown_count = statuses.iter().filter(|s| **s == TestStatus::Unknown).count() as u32;

    match expect {
        Expect::Pass => {
            if pass_count == runs {
                (Classification::AllPass, "written".into())
            } else if fail_count == runs {
                (Classification::AllFail, "surfaced_bug".into())
            } else if pass_count > 0 && fail_count > 0 {
                (Classification::Flaky, "quarantined".into())
            } else if unknown_count == runs {
                (Classification::NeverFound, "quarantined".into())
            } else {
                (Classification::Flaky, "quarantined".into())
            }
        }
        Expect::Fail => {
            // TDD red-check: failure is the desired outcome.
            if fail_count == runs {
                (Classification::RedOk, "pending".into())
            } else if pass_count > 0 {
                (Classification::VacuousPreImpl, "rejected_lint".into())
            } else if unknown_count == runs {
                (Classification::NeverFound, "quarantined".into())
            } else {
                (Classification::Flaky, "quarantined".into())
            }
        }
    }
}

#[derive(Clone)]
struct NewUnit {
    id: String,
    test_name: String,
    file_path: String,
    is_rust: bool,
    is_csharp: bool,
}

fn collect_new_units(work_units_file: &Path) -> anyhow::Result<Vec<NewUnit>> {
    let v: serde_json::Value = read_json_file(work_units_file)?;
    // Silent-accept: an unrecognized shape becomes an empty slice. Strict shape errors
    // would belong in the orchestrator's preflight, not here in Phase 5.
    let arr = parse_work_units_array(&v).unwrap_or(&[]);
    let mut out = Vec::new();
    for u in arr {
        let status = u.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if status != "written" {
            continue;
        }
        let id = match u.get("id").and_then(|s| s.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let test_name = match u.get("output_test_name").and_then(|s| s.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let file_path = match u.get("output_file_path").and_then(|s| s.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let ext = Path::new(&file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());
        let is_rust = ext.as_deref() == Some("rs");
        let is_csharp = ext.as_deref() == Some("cs");
        out.push(NewUnit {
            id,
            test_name,
            file_path,
            is_rust,
            is_csharp,
        });
    }
    Ok(out)
}

fn append_section(log_path: &Path, header: &str, body: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(f, "===== {} =====", header)?;
    f.write_all(body.as_bytes())?;
    writeln!(f)?;
    Ok(())
}

fn run_one_round(repo_root: &Path, stack: Stack, log_path: &Path, round: u32) -> anyhow::Result<(String, String)> {
    append_section(log_path, &format!("----- RUN {} -----", round), "")?;
    let mut rust_out = String::new();
    let mut csharp_out = String::new();
    if matches!(stack, Stack::Rust | Stack::Both) {
        let r: RunResult = run_with_timeout(
            "cargo",
            &["test", "--workspace", "--no-fail-fast"],
            repo_root,
            Duration::from_secs(900),
        )?;
        append_section(
            log_path,
            &format!("cargo test (exit {})", r.exit_code),
            &r.combined_output,
        )?;
        rust_out = r.combined_output;
    }
    if matches!(stack, Stack::Csharp | Stack::Both) {
        let r: RunResult = run_with_timeout(
            "dotnet",
            &["test", "--nologo", "--verbosity", "normal"],
            repo_root,
            Duration::from_secs(900),
        )?;
        append_section(
            log_path,
            &format!("dotnet test (exit {})", r.exit_code),
            &r.combined_output,
        )?;
        csharp_out = r.combined_output;
    }
    Ok((rust_out, csharp_out))
}

pub fn run_new_tests(
    repo_root: &Path,
    work_units_file: &Path,
    stack: Stack,
    log_dir: &Path,
    runs: u32,
    expect: Expect,
) -> anyhow::Result<RunNewTestsResult> {
    fs::create_dir_all(log_dir)?;
    let log_path = log_dir.join("run_new_tests.log");
    fs::write(
        &log_path,
        format!(
            "run_new_tests started {}\n",
            chrono::Utc::now().to_rfc3339()
        ),
    )?;

    let new_units = collect_new_units(work_units_file)?;
    if new_units.is_empty() {
        return Ok(RunNewTestsResult {
            runs,
            summary: "no newly-written tests".into(),
            per_unit_results: vec![],
            log_path,
        });
    }

    let mut per_unit: std::collections::HashMap<String, Vec<TestStatus>> =
        std::collections::HashMap::new();
    for u in &new_units {
        per_unit.insert(u.id.clone(), Vec::with_capacity(runs as usize));
    }

    for round in 1..=runs {
        let (rust_out, cs_out) = run_one_round(repo_root, stack, &log_path, round)?;
        for u in &new_units {
            let status = if u.is_rust {
                rust_test_status(&rust_out, &u.test_name)
            } else if u.is_csharp {
                csharp_test_status(&cs_out, &u.test_name)
            } else {
                TestStatus::Unknown
            };
            per_unit.get_mut(&u.id).unwrap().push(status);
        }
    }

    let per_unit_results: Vec<PerUnitResult> = new_units
        .iter()
        .map(|u| {
            let statuses = per_unit.remove(&u.id).unwrap_or_default();
            let (classification, recommended) = classify(&statuses, expect, runs);
            PerUnitResult {
                work_unit_id: u.id.clone(),
                output_test_name: u.test_name.clone(),
                output_file_path: u.file_path.clone(),
                per_run_statuses: statuses,
                classification,
                recommended_status: recommended,
            }
        })
        .collect();

    Ok(RunNewTestsResult {
        runs,
        summary: format!(
            "Classified {} new tests across {} runs.",
            per_unit_results.len(),
            runs
        ),
        per_unit_results,
        log_path,
    })
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let r = run_new_tests(
        &args.repo_root,
        &args.work_units_file,
        args.stack,
        &args.log_dir,
        args.runs,
        args.expect,
    )?;
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_test_status_pass() {
        let out = "test foo::bar ... ok\ntest foo::baz ... ok\n";
        assert_eq!(rust_test_status(out, "bar"), TestStatus::Pass);
    }

    #[test]
    fn rust_test_status_fail() {
        let out = "test foo::bar ... FAILED\n";
        assert_eq!(rust_test_status(out, "bar"), TestStatus::Fail);
    }

    #[test]
    fn rust_test_status_not_found_is_unknown() {
        let out = "test foo::bar ... ok\n";
        assert_eq!(rust_test_status(out, "absent"), TestStatus::Unknown);
    }

    #[test]
    fn rust_test_status_handles_module_path_prefix() {
        let out = "test parser::header::parse_truncated ... FAILED\n";
        assert_eq!(rust_test_status(out, "parse_truncated"), TestStatus::Fail);
    }

    #[test]
    fn csharp_test_status_pass() {
        let out = "  Passed Foo.Bar.Baz [12ms]\n";
        assert_eq!(csharp_test_status(out, "Baz"), TestStatus::Pass);
    }

    #[test]
    fn csharp_test_status_fail() {
        let out = "  Failed Foo.Bar.Quux [12ms]\n";
        assert_eq!(csharp_test_status(out, "Quux"), TestStatus::Fail);
    }

    #[test]
    fn classify_all_pass_with_expect_pass_is_written() {
        let (c, s) = classify(&[TestStatus::Pass; 3], Expect::Pass, 3);
        assert_eq!(c, Classification::AllPass);
        assert_eq!(s, "written");
    }

    #[test]
    fn classify_all_fail_with_expect_pass_is_surfaced_bug() {
        let (c, s) = classify(&[TestStatus::Fail; 3], Expect::Pass, 3);
        assert_eq!(c, Classification::AllFail);
        assert_eq!(s, "surfaced_bug");
    }

    #[test]
    fn classify_mixed_is_flaky_quarantined() {
        let (c, s) = classify(
            &[TestStatus::Pass, TestStatus::Fail, TestStatus::Pass],
            Expect::Pass,
            3,
        );
        assert_eq!(c, Classification::Flaky);
        assert_eq!(s, "quarantined");
    }

    #[test]
    fn classify_all_fail_with_expect_fail_is_red_ok() {
        // TDD red-check: this is the desired outcome.
        let (c, s) = classify(&[TestStatus::Fail; 3], Expect::Fail, 3);
        assert_eq!(c, Classification::RedOk);
        assert_eq!(s, "pending");
    }

    #[test]
    fn classify_any_pass_with_expect_fail_is_vacuous_pre_impl() {
        // A test that passes against an unimplemented!() stub is vacuous.
        let (c, s) = classify(
            &[TestStatus::Pass, TestStatus::Fail, TestStatus::Fail],
            Expect::Fail,
            3,
        );
        assert_eq!(c, Classification::VacuousPreImpl);
        assert_eq!(s, "rejected_lint");
    }

    #[test]
    fn classify_never_found_is_quarantined() {
        let (c, _) = classify(&[TestStatus::Unknown; 3], Expect::Pass, 3);
        assert_eq!(c, Classification::NeverFound);
    }

    // --- appended by unit-test-author (regression-tests, 2026-05-14) ---

    #[test]
    fn test_rust_test_status_distinguishes_exact_name_from_longer_prefix_sibling() {
        // Both `parse_truncated` (ok) and `parse_truncated_extended` (FAILED) appear.
        // The regex must not match the longer sibling as a Fail for the shorter name.
        let output = "test mod::parse_truncated ... ok\ntest mod::parse_truncated_extended ... FAILED\n";
        assert_eq!(
            rust_test_status(output, "parse_truncated"),
            TestStatus::Pass
        );
    }

    #[test]
    fn test_csharp_test_status_does_not_match_longer_identifier_suffix() {
        // `Quux` appears only as part of `QuuxExtended`; `\b` word-boundary must prevent a match.
        let output = "  Failed Foo.BarTest.QuuxExtended [12ms]\n";
        assert_eq!(
            csharp_test_status(output, "Quux"),
            TestStatus::Unknown
        );
    }

    #[test]
    fn test_classify_expect_fail_with_any_pass_among_unknowns_is_vacuous_pre_impl() {
        // Under Expect::Fail, a single Pass among Unknowns (zero Fails) means the test
        // passed against an unimplemented!() stub — it must be classified as VacuousPreImpl
        // with recommended_status "rejected_lint".
        let statuses = [TestStatus::Pass, TestStatus::Unknown, TestStatus::Unknown];
        let (classification, recommended_status) = classify(&statuses, Expect::Fail, 3);
        assert_eq!(classification, Classification::VacuousPreImpl);
        assert_eq!(recommended_status, "rejected_lint");
    }

    #[test]
    fn test_classify_with_zero_runs_does_not_report_all_pass_or_surfaced_bug() {
        // With an empty statuses slice and runs=0, the contract requires recommended_status
        // to be "quarantined", never "written" (AllPass) or "surfaced_bug" (AllFail).
        // Classification must be NeverFound or Flaky.
        let (classification, recommended_status) = classify(&[], Expect::Pass, 0);
        assert_eq!(recommended_status, "quarantined");
        assert!(
            matches!(classification, Classification::NeverFound | Classification::Flaky),
            "expected NeverFound or Flaky but got {:?}",
            classification
        );
    }

    #[test]
    fn test_only_written_status_units_are_collected_for_classification() {
        // collect_new_units must skip units whose status is not exactly "written".
        let work_units_json = serde_json::json!([
            {
                "id": "unit-written",
                "status": "written",
                "output_test_name": "test_written",
                "output_file_path": "src/foo.rs"
            },
            {
                "id": "unit-pending",
                "status": "pending",
                "output_test_name": "test_pending",
                "output_file_path": "src/foo.rs"
            },
            {
                "id": "unit-implemented",
                "status": "implemented",
                "output_test_name": "test_implemented",
                "output_file_path": "src/foo.rs"
            },
            {
                "id": "unit-rejected",
                "status": "rejected_lint",
                "output_test_name": "test_rejected",
                "output_file_path": "src/foo.rs"
            },
            {
                "id": "unit-quarantined",
                "status": "quarantined",
                "output_test_name": "test_quarantined",
                "output_file_path": "src/foo.rs"
            },
            {
                "id": "unit-surfaced",
                "status": "surfaced_bug",
                "output_test_name": "test_surfaced",
                "output_file_path": "src/foo.rs"
            }
        ]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let units = collect_new_units(&path).unwrap();

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "unit-written");
    }

    /// Lock the wrapper-object acceptance: `{"work_units": [...], "scope_summary": ...}`
    /// must yield the units from the inner array, not zero units (which would silently
    /// green-wash Phase 5).
    #[test]
    fn test_collect_new_units_accepts_orchestrator_wrapper_object_shape() {
        let wrapper = serde_json::json!({
            "work_units": [
                {
                    "id": "inner-written",
                    "status": "written",
                    "output_test_name": "test_inner",
                    "output_file_path": "src/foo.rs"
                },
                {
                    "id": "inner-pending",
                    "status": "pending",
                    "output_test_name": "test_pending",
                    "output_file_path": "src/foo.rs"
                }
            ],
            "scope_summary": "wrapper metadata that must NOT be treated as a unit"
        });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, wrapper.to_string()).unwrap();

        let units = collect_new_units(&path).unwrap();

        // Only the `written` entry from the INNER array should be collected.
        assert_eq!(
            units.len(),
            1,
            "wrapper-shape work-units.json must extract its inner array; got: {:?}",
            units.iter().map(|u| &u.id).collect::<Vec<_>>()
        );
        assert_eq!(units[0].id, "inner-written");
    }
}
