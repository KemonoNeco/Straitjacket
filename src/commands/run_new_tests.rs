use crate::commands::detect_stack::detect_stack;
use crate::common::cargo_target::{cargo_invocation, resolve_cargo_target, CargoInvocation};
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
    pub nothing_to_run: bool,
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

#[derive(Clone, Debug)]
pub struct NewUnit {
    pub id: String,
    pub test_name: String,
    pub file_path: String,
    pub is_rust: bool,
    pub is_csharp: bool,
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
        let manifests = detect_stack(repo_root)?.rust_manifests;
        let target = resolve_cargo_target(&manifests, repo_root);
        match cargo_invocation(&target, &["test", "--no-fail-fast"]) {
            CargoInvocation::Run { cwd, args } => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let r: RunResult =
                    run_with_timeout("cargo", &argv, &cwd, Duration::from_secs(900))?;
                append_section(
                    log_path,
                    &format!("cargo test (exit {})", r.exit_code),
                    &r.combined_output,
                )?;
                rust_out = r.combined_output;
            }
            CargoInvocation::Skip => {
                // No Rust target — leave rust_out empty; units classify as NeverFound.
            }
            CargoInvocation::Ambiguous { candidates } => {
                let joined = candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                append_section(
                    log_path,
                    "cargo target resolution",
                    &format!(
                        "ambiguous-cargo-target: multiple crates with no root manifest: {}",
                        joined
                    ),
                )?;
                // Leave rust_out empty so tests classify as NeverFound/quarantined,
                // never a silent pass.
            }
        }
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
            nothing_to_run: true,
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
        nothing_to_run: false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurvivalReport {
    pub ok: bool,
    pub nothing_to_verify: bool,
    pub survived: Vec<String>,
    pub regressed: Vec<String>,
    pub missing: Vec<String>,
}

/// Compares the names that were RED in the red phase against their status after the green run.
pub fn name_survival(expected_red_names: &[&str], green_statuses: &[(String, TestStatus)]) -> SurvivalReport {
    let nothing_to_verify = expected_red_names.is_empty();
    let mut survived = Vec::new();
    let mut regressed = Vec::new();
    let mut missing = Vec::new();
    let mut seen: Vec<&str> = Vec::new();

    for &name in expected_red_names {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let status = green_statuses
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| *s);
        match status {
            Some(TestStatus::Pass) => survived.push(name.to_string()),
            Some(TestStatus::Fail) => regressed.push(name.to_string()),
            Some(TestStatus::Unknown) | None => missing.push(name.to_string()),
        }
    }

    let ok = missing.is_empty() && regressed.is_empty() && !nothing_to_verify;
    SurvivalReport {
        ok,
        nothing_to_verify,
        survived,
        regressed,
        missing,
    }
}

#[derive(Debug)]
pub struct CollectByNameResult {
    pub units: Vec<NewUnit>,
    pub unmatched: Vec<String>,
}

/// Selects work units by an explicit name list, IGNORING each unit's `status` field.
pub fn collect_units_by_name(work_units_file: &Path, names: &[&str]) -> anyhow::Result<CollectByNameResult> {
    let v: serde_json::Value = read_json_file(work_units_file)?;
    let arr = parse_work_units_array(&v).unwrap_or(&[]);

    // Lookup of units by output_test_name; units missing that field are skipped.
    let mut by_name: std::collections::HashMap<&str, &serde_json::Value> =
        std::collections::HashMap::new();
    for u in arr {
        let test_name = match u.get("output_test_name").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => continue,
        };
        by_name.entry(test_name).or_insert(u);
    }

    let mut units = Vec::new();
    let mut unmatched = Vec::new();
    let mut seen: Vec<&str> = Vec::new();

    for &name in names {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        match by_name.get(name) {
            Some(u) => {
                let id = u
                    .get("id")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                let file_path = u
                    .get("output_file_path")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ext = Path::new(&file_path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase());
                let is_rust = ext.as_deref() == Some("rs");
                let is_csharp = ext.as_deref() == Some("cs");
                units.push(NewUnit {
                    id,
                    test_name: name.to_string(),
                    file_path,
                    is_rust,
                    is_csharp,
                });
            }
            None => unmatched.push(name.to_string()),
        }
    }

    Ok(CollectByNameResult { units, unmatched })
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

    // --- appended by unit-test-author (straightjacket, 2026-05-14) ---

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

    // --- B: nothing_to_run field tests ---

    /// RED: placeholder is `false`; contract requires `true` when no units are collected.
    #[test]
    fn test_nothing_to_run_is_true_when_no_units_collected() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-pending",
                "status": "pending",
                "output_test_name": "test_pending",
                "output_file_path": "src/foo.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let work_units_file = dir.path().join("work-units.json");
        std::fs::write(&work_units_file, work_units_json.to_string()).unwrap();
        let log_dir = dir.path().join("logs");

        let result = run_new_tests(
            dir.path(),
            &work_units_file,
            crate::common::Stack::Rust,
            &log_dir,
            3,
            Expect::Pass,
        )
        .unwrap();

        assert!(result.nothing_to_run, "nothing_to_run must be true when no written units are found");
        assert!(result.per_unit_results.is_empty());
    }

    /// GREEN against placeholder: one written unit + runs=0 → no cargo spawned; placeholder `false` matches.
    #[test]
    fn test_nothing_to_run_is_false_when_units_present() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-written",
                "status": "written",
                "output_test_name": "test_written",
                "output_file_path": "src/foo.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let work_units_file = dir.path().join("work-units.json");
        std::fs::write(&work_units_file, work_units_json.to_string()).unwrap();
        let log_dir = dir.path().join("logs");

        let result = run_new_tests(
            dir.path(),
            &work_units_file,
            crate::common::Stack::Rust,
            &log_dir,
            0, // runs=0: for round in 1..=0 iterates zero times, no cargo spawned
            Expect::Pass,
        )
        .unwrap();

        assert!(!result.nothing_to_run, "nothing_to_run must be false when written units are present");
        assert!(!result.per_unit_results.is_empty());
    }

    /// GREEN against placeholder: serialized JSON must contain a boolean key named "nothing_to_run".
    #[test]
    fn test_run_new_tests_result_serializes_nothing_to_run_key() {
        let log_dir = tempfile::tempdir().unwrap();
        let result = RunNewTestsResult {
            runs: 1,
            summary: "test".into(),
            per_unit_results: vec![],
            log_path: log_dir.path().join("run_new_tests.log"),
            nothing_to_run: false,
        };
        let value = serde_json::to_value(&result).unwrap();
        let field = value.get("nothing_to_run");
        assert!(field.is_some(), "serialized JSON must contain 'nothing_to_run' key");
        assert!(field.unwrap().is_boolean(), "'nothing_to_run' must serialize as a boolean");
    }

    // --- C: name_survival tests (all RED against unimplemented!()) ---

    #[test]
    fn test_name_survival_all_present_and_pass_are_survived() {
        let green = vec![
            ("a".to_string(), TestStatus::Pass),
            ("b".to_string(), TestStatus::Pass),
        ];
        let report = name_survival(&["a", "b"], &green);
        assert!(report.ok);
        assert!(!report.nothing_to_verify);
        let mut survived = report.survived.clone();
        survived.sort();
        assert_eq!(survived, vec!["a", "b"]);
        assert!(report.regressed.is_empty());
        assert!(report.missing.is_empty());
    }

    #[test]
    fn test_name_survival_present_but_fail_is_regressed_and_not_ok() {
        let green = vec![("a".to_string(), TestStatus::Fail)];
        let report = name_survival(&["a"], &green);
        assert!(!report.ok);
        assert!(report.survived.is_empty());
        assert_eq!(report.regressed, vec!["a"]);
        assert!(report.missing.is_empty());
    }

    #[test]
    fn test_name_survival_now_unknown_is_missing_and_not_ok() {
        let green = vec![("a".to_string(), TestStatus::Unknown)];
        let report = name_survival(&["a"], &green);
        assert!(!report.ok);
        assert!(report.survived.is_empty());
        assert!(report.regressed.is_empty());
        assert_eq!(report.missing, vec!["a"]);
    }

    #[test]
    fn test_name_survival_mixed_categories_partition_correctly_and_not_ok() {
        let green = vec![
            ("a".to_string(), TestStatus::Pass),
            ("b".to_string(), TestStatus::Fail),
            ("c".to_string(), TestStatus::Unknown),
        ];
        let report = name_survival(&["a", "b", "c"], &green);
        assert!(!report.ok);
        assert_eq!(report.survived, vec!["a"]);
        assert_eq!(report.regressed, vec!["b"]);
        assert_eq!(report.missing, vec!["c"]);
    }

    #[test]
    fn test_name_survival_empty_expected_set_is_nothing_to_verify_and_not_ok() {
        let green = vec![("a".to_string(), TestStatus::Pass)];
        let report = name_survival(&[], &green);
        assert!(report.nothing_to_verify);
        assert!(!report.ok);
        // An impl that sets nothing_to_verify but still fills the partitions must fail.
        assert!(report.survived.is_empty());
        assert!(report.regressed.is_empty());
        assert!(report.missing.is_empty());
    }

    #[test]
    fn test_name_survival_ignores_green_names_not_in_expected_set() {
        let green = vec![
            ("a".to_string(), TestStatus::Pass),
            ("z".to_string(), TestStatus::Fail),
        ];
        let report = name_survival(&["a"], &green);
        assert!(report.ok);
        assert_eq!(report.survived, vec!["a"]);
        assert!(report.regressed.is_empty());
        assert!(report.missing.is_empty());
        assert!(!report.survived.contains(&"z".to_string()));
        assert!(!report.regressed.contains(&"z".to_string()));
        assert!(!report.missing.contains(&"z".to_string()));
    }

    #[test]
    fn test_name_survival_deduplicates_repeated_expected_name() {
        let green = vec![("a".to_string(), TestStatus::Pass)];
        let report = name_survival(&["a", "a"], &green);
        assert_eq!(report.survived.len(), 1);
        assert_eq!(report.survived, vec!["a"]);
    }

    // --- D: collect_units_by_name tests (all RED against unimplemented!()) ---

    #[test]
    fn test_collect_units_by_name_collects_pending_status_unit() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-pending",
                "status": "pending",
                "output_test_name": "test_pending",
                "output_file_path": "src/foo.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let result = collect_units_by_name(&path, &["test_pending"]).unwrap();
        assert_eq!(result.units.len(), 1);
        assert_eq!(result.units[0].test_name, "test_pending");
        assert!(result.unmatched.is_empty());
    }

    #[test]
    fn test_collect_units_by_name_reports_unmatched_requested_names() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-a",
                "status": "written",
                "output_test_name": "test_a",
                "output_file_path": "src/foo.rs"
            },
            {
                "id": "unit-b",
                "status": "pending",
                "output_test_name": "test_b",
                "output_file_path": "src/foo.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let result = collect_units_by_name(&path, &["test_a", "test_b", "test_missing"]).unwrap();
        assert_eq!(result.units.len(), 2);
        assert_eq!(result.unmatched, vec!["test_missing"]);
        // Also assert WHICH two units were returned, not just the count.
        let mut returned_names: Vec<String> =
            result.units.iter().map(|u| u.test_name.clone()).collect();
        returned_names.sort();
        assert_eq!(returned_names, vec!["test_a", "test_b"]);
    }

    #[test]
    fn test_collect_units_by_name_empty_request_collects_nothing() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-a",
                "status": "written",
                "output_test_name": "test_a",
                "output_file_path": "src/foo.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let result = collect_units_by_name(&path, &[]).unwrap();
        assert!(result.units.is_empty());
        assert!(result.unmatched.is_empty());
    }

    /// NEW (1b): wrapper-object shape `{"work_units":[...], "scope_summary":"..."}` must
    /// extract units from the inner array, regardless of status filter (status is ignored here).
    #[test]
    fn test_collect_units_by_name_accepts_wrapper_object_shape() {
        let wrapper = serde_json::json!({
            "work_units": [
                {
                    "id": "wu-x",
                    "status": "pending",
                    "output_test_name": "test_x",
                    "output_file_path": "src/x.rs"
                }
            ],
            "scope_summary": "meta"
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, wrapper.to_string()).unwrap();

        let result = collect_units_by_name(&path, &["test_x"]).unwrap();
        assert_eq!(result.units.len(), 1);
        assert_eq!(result.units[0].test_name, "test_x");
        assert!(result.unmatched.is_empty());
    }

    /// NEW (1b): units lacking `output_test_name` must be silently skipped (not panic).
    #[test]
    fn test_collect_units_by_name_skips_unit_missing_output_test_name() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-malformed",
                "status": "pending",
                "output_file_path": "src/bad.rs"
                // output_test_name intentionally absent
            },
            {
                "id": "unit-good",
                "status": "written",
                "output_test_name": "test_good",
                "output_file_path": "src/good.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        // Must not panic; malformed unit is silently skipped.
        let result = collect_units_by_name(&path, &["test_good"]).unwrap();
        assert_eq!(result.units.len(), 1);
        assert_eq!(result.units[0].test_name, "test_good");
        assert!(result.unmatched.is_empty());
    }

    /// NEW (1b): a repeated name in the request must yield only one unit, not two.
    #[test]
    fn test_collect_units_by_name_deduplicates_repeated_requested_name() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-a",
                "status": "pending",
                "output_test_name": "test_a",
                "output_file_path": "src/a.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let result = collect_units_by_name(&path, &["test_a", "test_a"]).unwrap();
        assert_eq!(result.units.len(), 1, "duplicate request must not double-return the unit");
        assert!(result.unmatched.is_empty());
    }

    /// NEW (1b strength): dedup on the Fail branch — repeated expected name with a Fail
    /// result must appear exactly once in `regressed`, not twice.
    #[test]
    fn test_name_survival_deduplicates_repeated_expected_name_fail_path() {
        let green = vec![("a".to_string(), TestStatus::Fail)];
        let report = name_survival(&["a", "a"], &green);
        assert_eq!(report.regressed, vec!["a"], "regressed must contain 'a' exactly once");
        assert_eq!(report.regressed.len(), 1, "dedup must collapse repeated name to len 1");
        assert!(report.survived.is_empty());
        assert!(report.missing.is_empty());
    }

    #[test]
    fn test_collect_units_by_name_derives_is_rust_and_is_csharp_from_extension() {
        // Two units: one with a .rs output path and one with a .cs output path.
        // Asserts that is_rust / is_csharp flags are derived from the file extension,
        // killing the two surviving mutants at lines 437-438.
        let work_units_json = serde_json::json!([
            {
                "id": "u-rs",
                "status": "pending",
                "output_test_name": "test_rs",
                "output_file_path": "src/x.rs"
            },
            {
                "id": "u-cs",
                "status": "pending",
                "output_test_name": "test_cs",
                "output_file_path": "Foo.cs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let result = collect_units_by_name(&path, &["test_rs", "test_cs"]).unwrap();
        assert_eq!(result.units.len(), 2);
        assert!(result.unmatched.is_empty());

        let rs_unit = result.units.iter().find(|u| u.test_name == "test_rs").unwrap();
        assert!(rs_unit.is_rust, "unit with .rs path must have is_rust = true");
        assert!(!rs_unit.is_csharp, "unit with .rs path must have is_csharp = false");

        let cs_unit = result.units.iter().find(|u| u.test_name == "test_cs").unwrap();
        assert!(cs_unit.is_csharp, "unit with .cs path must have is_csharp = true");
        assert!(!cs_unit.is_rust, "unit with .cs path must have is_rust = false");
    }

    #[test]
    fn test_collect_units_by_name_single_match_returns_one_unit() {
        let work_units_json = serde_json::json!([
            {
                "id": "unit-sole",
                "status": "quarantined",
                "output_test_name": "test_sole",
                "output_file_path": "src/sole.rs"
            }
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work-units.json");
        std::fs::write(&path, work_units_json.to_string()).unwrap();

        let result = collect_units_by_name(&path, &["test_sole"]).unwrap();
        assert_eq!(result.units.len(), 1);
        assert!(result.unmatched.is_empty());
        assert_eq!(result.units[0].test_name, "test_sole");
    }
}
