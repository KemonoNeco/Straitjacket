use crate::common::json_io::read_json_file;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// A single surfaced-bug finding from a tdd-cycle run.
#[derive(Debug, Deserialize)]
pub struct SurfacedFinding {
    pub work_unit_id: String,
    pub target_file: String,
    #[serde(default)]
    pub target_symbol: Option<String>,
    #[serde(default)]
    pub intended_behavior_seed: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Minimal ledger record — reused from .straitjacket/bugs.json.
/// The crate treats the ledger as untyped Value in bug_status.rs; we define
/// a typed projection here with only the fields capture logic requires.
#[derive(Debug, Deserialize)]
pub struct BugRecord {
    #[serde(default)]
    pub suspect_files: Vec<String>,
    #[serde(default)]
    pub status: String,
}

/// One entry in the uncaptured list: identifies the unfiled finding.
#[derive(Debug, Serialize)]
pub struct UncapturedEntry {
    pub work_unit_id: String,
    pub target_file: String,
}

/// The gate's output — mirrors the `no_findings_checked` / `ok` / `uncaptured`
/// wire contract that the orchestrator parses.
#[derive(Debug, Serialize)]
pub struct CaptureReport {
    pub ok: bool,
    pub no_findings_checked: bool,
    pub uncaptured: Vec<UncapturedEntry>,
}

/// Given the run's structured surfaced-bug findings and the ledger, returns
/// not-ok whenever any surfaced finding is absent from the ledger, so the
/// orchestrator cannot reach ready_to_commit with an unfiled surfaced bug.
///
/// Capture is decided by file intersection: a finding is captured if and only
/// if its `target_file` is present in any ledger record's `suspect_files`.
pub fn check_surfaced_bugs_captured(
    findings: &[SurfacedFinding],
    ledger: &[BugRecord],
) -> CaptureReport {
    // The capture set is the union of every record's suspect_files across the
    // whole ledger — record position and lifecycle status are irrelevant; an
    // empty suspect_files array contributes nothing. Matching is exact equality.
    let captured_files: HashSet<&str> = ledger
        .iter()
        .flat_map(|record| record.suspect_files.iter())
        .map(String::as_str)
        .collect();

    let uncaptured: Vec<UncapturedEntry> = findings
        .iter()
        .filter(|finding| !captured_files.contains(finding.target_file.as_str()))
        .map(|finding| UncapturedEntry {
            work_unit_id: finding.work_unit_id.clone(),
            target_file: finding.target_file.clone(),
        })
        .collect();

    CaptureReport {
        // Zero findings ⇒ zero uncaptured ⇒ ok is vacuously true.
        ok: uncaptured.is_empty(),
        no_findings_checked: findings.is_empty(),
        uncaptured,
    }
}

/// CLI args for the `verify-surfaced-bugs-captured` gate.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    /// JSON file holding the run's surfaced-bug findings: a bare array of
    /// `{work_unit_id, target_file, ...}` objects.
    #[arg(long)]
    pub findings_file: PathBuf,
}

/// Typed projection over `.straitjacket/bugs.json`'s `{"bugs":[...]}` wrapper.
/// serde ignores every real bug-record field the capture decision doesn't use.
#[derive(Debug, Deserialize)]
struct Ledger {
    #[serde(default)]
    bugs: Vec<BugRecord>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let findings: Vec<SurfacedFinding> = read_json_file(&args.findings_file)?;

    let ledger_path = args.repo_root.join(".straitjacket").join("bugs.json");
    // An absent ledger is the empty-ledger case (every finding uncaptured),
    // not an error — only a present-but-malformed ledger should fail.
    let ledger: Vec<BugRecord> = if ledger_path.exists() {
        read_json_file::<Ledger>(&ledger_path)?.bugs
    } else {
        Vec::new()
    };

    let report = check_surfaced_bugs_captured(&findings, &ledger);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.ok {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn finding(id: &str, file: &str) -> SurfacedFinding {
        SurfacedFinding {
            work_unit_id: id.to_string(),
            target_file: file.to_string(),
            target_symbol: None,
            intended_behavior_seed: None,
            note: None,
        }
    }

    fn record(files: &[&str], status: &str) -> BugRecord {
        BugRecord {
            suspect_files: files.iter().map(|s| s.to_string()).collect(),
            status: status.to_string(),
        }
    }

    // ── work unit b1f0a2c4 ────────────────────────────────────────────────────
    #[test]
    fn test_uncaptured_finding_makes_report_not_ok() {
        let findings = vec![finding("wu-x", "src/missing.rs")];
        let ledger: Vec<BugRecord> = vec![];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.uncaptured.is_empty());
        assert!(report.uncaptured.iter().any(|e| e.work_unit_id == "wu-x"));
    }

    // ── work unit c2a1b3d5 ────────────────────────────────────────────────────
    #[test]
    fn test_single_finding_with_matching_suspect_file_is_captured_and_ok() {
        let findings = vec![finding("wu-1", "src/foo.rs")];
        let ledger = vec![record(&["src/foo.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
        assert!(!report.no_findings_checked);
    }

    // ── work unit d3b2c4e6 ────────────────────────────────────────────────────
    #[test]
    fn test_file_intersection_is_the_capture_rule_unrelated_record_does_not_capture() {
        let findings = vec![finding("wu-1", "src/foo.rs")];
        let ledger = vec![record(&["src/bar.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(report.uncaptured.iter().any(|e| e.work_unit_id == "wu-1"));
        assert!(!report.no_findings_checked);
    }

    // ── work unit e4c3d5f7 ────────────────────────────────────────────────────
    #[test]
    fn test_resolved_status_record_still_counts_as_captured() {
        let findings = vec![finding("wu-1", "src/foo.rs")];
        let ledger = vec![record(&["src/foo.rs"], "fixed")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
    }

    // ── work unit f5d4e6a8 ────────────────────────────────────────────────────
    #[test]
    fn test_mixed_findings_uncaptured_list_holds_exactly_the_missing_ids() {
        let findings = vec![
            finding("wu-a", "src/a.rs"),
            finding("wu-b", "src/b.rs"),
            finding("wu-c", "src/c.rs"),
        ];
        let ledger = vec![
            record(&["src/a.rs"], "open"),
            record(&["src/c.rs"], "open"),
        ];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        let got: HashSet<&str> = report.uncaptured.iter().map(|e| e.work_unit_id.as_str()).collect();
        assert_eq!(got, HashSet::from(["wu-b"]));
    }

    // ── work unit a6e5f7b9 ────────────────────────────────────────────────────
    #[test]
    fn test_exactly_one_uncaptured_finding_is_not_ok_and_listed_singly() {
        let findings = vec![
            finding("wu-a", "src/a.rs"),
            finding("wu-b", "src/b.rs"),
        ];
        let ledger = vec![record(&["src/a.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert_eq!(report.uncaptured.len(), 1);
        assert_eq!(report.uncaptured[0].work_unit_id, "wu-b");
    }

    // ── work unit b7f6a8c0 ────────────────────────────────────────────────────
    #[test]
    fn test_two_or_more_uncaptured_findings_all_listed_and_not_ok() {
        let findings = vec![
            finding("wu-a", "src/a.rs"),
            finding("wu-b", "src/b.rs"),
            finding("wu-c", "src/c.rs"),
        ];
        let ledger = vec![record(&["src/a.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        let got: HashSet<&str> = report.uncaptured.iter().map(|e| e.work_unit_id.as_str()).collect();
        assert_eq!(got, HashSet::from(["wu-b", "wu-c"]));
    }

    // ── work unit c8a7b9d1 ────────────────────────────────────────────────────
    #[test]
    fn test_zero_findings_sets_no_findings_checked_true_and_ok_true() {
        let findings: Vec<SurfacedFinding> = vec![];
        let ledger: Vec<BugRecord> = vec![];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.no_findings_checked);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
    }

    // ── work unit d9b8c0e2 ────────────────────────────────────────────────────
    #[test]
    fn test_all_captured_findings_set_no_findings_checked_false_and_ok_true() {
        let findings = vec![
            finding("wu-a", "src/a.rs"),
            finding("wu-b", "src/b.rs"),
        ];
        let ledger = vec![
            record(&["src/a.rs"], "open"),
            record(&["src/b.rs"], "open"),
        ];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.no_findings_checked);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
    }

    // ── work unit eac9d1f3 ────────────────────────────────────────────────────
    #[test]
    fn test_uncaptured_with_nonempty_ledger_keeps_no_findings_checked_false() {
        let findings = vec![finding("wu-x", "src/foo.rs")];
        let ledger = vec![record(&["src/unrelated.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(!report.uncaptured.is_empty());
    }

    // ── work unit fbdae204 ────────────────────────────────────────────────────
    #[test]
    fn test_empty_ledger_leaves_all_findings_uncaptured() {
        let findings = vec![
            finding("wu-a", "src/a.rs"),
            finding("wu-b", "src/b.rs"),
        ];
        let ledger: Vec<BugRecord> = vec![];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        let got: HashSet<&str> = report.uncaptured.iter().map(|e| e.work_unit_id.as_str()).collect();
        assert_eq!(got, HashSet::from(["wu-a", "wu-b"]));
    }

    // ── work unit 0cebf315 ────────────────────────────────────────────────────
    #[test]
    fn test_capture_matches_when_target_file_is_any_element_of_suspect_files() {
        let findings = vec![finding("wu-1", "src/c.rs")];
        let ledger = vec![record(&["src/a.rs", "src/b.rs", "src/c.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
    }

    // ── work unit 1dfca426 ────────────────────────────────────────────────────
    #[test]
    fn test_capture_report_serializes_ok_no_findings_checked_and_uncaptured_entry_shape() {
        let report = CaptureReport {
            ok: false,
            no_findings_checked: false,
            uncaptured: vec![UncapturedEntry {
                work_unit_id: "wu-z".to_string(),
                target_file: "src/z.rs".to_string(),
            }],
        };
        let v = serde_json::to_value(&report).unwrap();

        let ok_field = v.get("ok").expect("key 'ok' must exist");
        assert!(ok_field.is_boolean(), "ok must be a boolean");

        let nfc_field = v.get("no_findings_checked").expect("key 'no_findings_checked' must exist");
        assert!(nfc_field.is_boolean(), "no_findings_checked must be a boolean");

        let uncaptured = v.get("uncaptured").expect("key 'uncaptured' must exist");
        assert!(uncaptured.is_array(), "uncaptured must be an array");

        let entry = &uncaptured[0];
        assert!(entry.get("work_unit_id").map(|v| v.is_string()).unwrap_or(false),
            "uncaptured[0].work_unit_id must be a string");
        assert!(entry.get("target_file").map(|v| v.is_string()).unwrap_or(false),
            "uncaptured[0].target_file must be a string");
    }

    // ── work unit 3a4b5c6d ────────────────────────────────────────────────────
    #[test]
    fn test_capture_matches_when_target_file_is_in_a_later_record() {
        let findings = vec![finding("wu-1", "src/foo.rs")];
        let ledger = vec![
            record(&["src/bar.rs"], "open"),
            record(&["src/baz.rs"], "open"),
            record(&["src/foo.rs"], "open"),
        ];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
        assert!(!report.no_findings_checked);
    }

    // ── work unit 4b5c6d7e ────────────────────────────────────────────────────
    #[test]
    fn test_empty_suspect_files_record_does_not_capture() {
        let findings = vec![finding("wu-empty", "src/foo.rs")];
        let ledger = vec![record(&[], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(report.uncaptured.iter().any(|e| e.work_unit_id == "wu-empty"));
    }

    // ── work unit 5c6d7e8f ────────────────────────────────────────────────────
    #[test]
    fn test_zero_findings_with_nonempty_ledger_still_no_findings_checked_true_and_ok() {
        let findings: Vec<SurfacedFinding> = vec![];
        let ledger = vec![record(&["src/foo.rs"], "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.no_findings_checked);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
    }

    // ── work unit 2e0db537 ────────────────────────────────────────────────────
    #[test]
    fn test_surfaced_finding_deserializes_from_tdd_cycle_surfaced_bugs_entry() {
        let raw = json!({
            "work_unit_id": "wu-a",
            "target_file": "src/foo.rs",
            "target_symbol": "foo::bar",
            "intended_behavior_seed": "bar returns Err on empty",
            "note": "found in stage D"
        });
        let f: SurfacedFinding = serde_json::from_value(raw).unwrap();
        assert_eq!(f.work_unit_id, "wu-a");
        assert_eq!(f.target_file, "src/foo.rs");
    }
}
