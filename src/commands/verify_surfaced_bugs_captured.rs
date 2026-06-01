use crate::common::json_io::read_json_file;
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    pub suspect_symbol: Option<String>,
    #[serde(default)]
    pub intended_behavior_seed: Option<String>,
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
/// Capture is decided by **file AND identity on the SAME record** (not by a
/// ledger-wide file intersection — that errs loose and drops distinct bugs in
/// an already-recorded file). A finding `f` is captured iff there exists a
/// single ledger record `r` for which BOTH hold:
/// 1. **file membership**: `f.target_file` is an element of `r.suspect_files`
///    (per-element OR within the one record; an empty `suspect_files`
///    contributes no coverage).
/// 2. **identity AND**: every identity field the finding *has* matches `r`:
///    - a non-empty `f.target_symbol` must equal `r.suspect_symbol`
///      **exactly** (case-sensitive, no normalization);
///    - a non-empty `f.intended_behavior_seed` must equal
///      `r.intended_behavior_seed` after normalization (trim, lowercase,
///      collapse internal whitespace runs to one space — both sides).
///
/// A finding with NO identity field beyond `target_file` matches no record on
/// identity and is therefore UNCAPTURED, even if a record covers its file —
/// the gate cannot confirm the specific bug was filed, so it errs toward
/// re-filing. This is the safe direction: a false-uncaptured forces a harmless
/// re-file, while a false-captured silently drops a real bug. Record lifecycle
/// `status` is irrelevant (a resolved record still captures).
pub fn check_surfaced_bugs_captured(
    findings: &[SurfacedFinding],
    ledger: &[BugRecord],
) -> CaptureReport {
    let uncaptured: Vec<UncapturedEntry> = findings
        .iter()
        .filter(|finding| !is_captured(finding, ledger))
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

/// True iff a single ledger record covers the finding's `target_file` AND
/// matches every identity field the finding carries. The file-AND-identity
/// test lives inside one per-record predicate so that "file in record A,
/// identity in record B" never counts as a match.
fn is_captured(finding: &SurfacedFinding, ledger: &[BugRecord]) -> bool {
    let symbol = finding
        .target_symbol
        .as_deref()
        .filter(|s| !s.is_empty());
    // The finding's seed is loop-invariant, so normalize it ONCE here rather
    // than re-normalizing it for every ledger record below.
    let normalized_seed = finding
        .intended_behavior_seed
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(normalize_seed);

    // No present identity field ⇒ cannot confirm capture ⇒ uncaptured.
    if symbol.is_none() && normalized_seed.is_none() {
        return false;
    }

    ledger.iter().any(|record| {
        let file_member = record
            .suspect_files
            .iter()
            .any(|f| f == &finding.target_file);

        // Symbol: exact, case-sensitive. Seed: normalized. AND over every
        // present field — an absent finding field imposes no constraint.
        let symbol_ok = match symbol {
            Some(s) => record.suspect_symbol.as_deref() == Some(s),
            None => true,
        };
        let seed_ok = match &normalized_seed {
            Some(ns) => record
                .intended_behavior_seed
                .as_deref()
                .is_some_and(|r| &normalize_seed(r) == ns),
            None => true,
        };

        file_member && symbol_ok && seed_ok
    })
}

/// Normalizes a seed for comparison: trims leading/trailing whitespace,
/// lowercases, and collapses every internal run of whitespace to a single
/// space. `split_whitespace` trims and splits on any run of Unicode
/// whitespace; rejoining with one space collapses internal runs.
fn normalize_seed(seed: &str) -> String {
    seed.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

    /// Bare finding: no symbol, no seed.
    fn finding(id: &str, file: &str) -> SurfacedFinding {
        SurfacedFinding {
            work_unit_id: id.to_string(),
            target_file: file.to_string(),
            target_symbol: None,
            intended_behavior_seed: None,
            note: None,
        }
    }

    /// Finding with optional symbol and seed.
    fn finding_id(
        id: &str,
        file: &str,
        symbol: Option<&str>,
        seed: Option<&str>,
    ) -> SurfacedFinding {
        SurfacedFinding {
            work_unit_id: id.to_string(),
            target_file: file.to_string(),
            target_symbol: symbol.map(str::to_string),
            intended_behavior_seed: seed.map(str::to_string),
            note: None,
        }
    }

    /// Bare record: no symbol, no seed.
    fn record(files: &[&str], status: &str) -> BugRecord {
        BugRecord {
            suspect_files: files.iter().map(|s| s.to_string()).collect(),
            status: status.to_string(),
            suspect_symbol: None,
            intended_behavior_seed: None,
        }
    }

    /// Record with optional symbol and seed.
    fn record_full(
        files: &[&str],
        symbol: Option<&str>,
        seed: Option<&str>,
        status: &str,
    ) -> BugRecord {
        BugRecord {
            suspect_files: files.iter().map(|s| s.to_string()).collect(),
            status: status.to_string(),
            suspect_symbol: symbol.map(str::to_string),
            intended_behavior_seed: seed.map(str::to_string),
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
        // Finding with symbol=Foo::bar; record matches file AND symbol exactly.
        let findings = vec![finding_id("wu-1", "src/foo.rs", Some("Foo::bar"), None)];
        let ledger = vec![record_full(&["src/foo.rs"], Some("Foo::bar"), None, "open")];
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
        // File AND symbol match; record status is "fixed" (resolved).
        let findings = vec![finding_id("wu-1", "src/foo.rs", Some("Foo::bar"), None)];
        let ledger = vec![record_full(&["src/foo.rs"], Some("Foo::bar"), None, "fixed")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
    }

    // ── work unit f5d4e6a8 ────────────────────────────────────────────────────
    #[test]
    fn test_mixed_findings_uncaptured_list_holds_exactly_the_missing_ids() {
        // wu-a and wu-c: file+symbol match their records. wu-b: file in no record.
        let findings = vec![
            finding_id("wu-a", "src/a.rs", Some("A::f"), None),
            finding("wu-b", "src/b.rs"),
            finding_id("wu-c", "src/c.rs", Some("C::g"), None),
        ];
        let ledger = vec![
            record_full(&["src/a.rs"], Some("A::f"), None, "open"),
            record_full(&["src/c.rs"], Some("C::g"), None, "open"),
        ];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        let got: HashSet<&str> = report
            .uncaptured
            .iter()
            .map(|e| e.work_unit_id.as_str())
            .collect();
        assert_eq!(got, HashSet::from(["wu-b"]));
    }

    // ── work unit a6e5f7b9 ────────────────────────────────────────────────────
    #[test]
    fn test_exactly_one_uncaptured_finding_is_not_ok_and_listed_singly() {
        // wu-a: file+symbol match. wu-b: no covering record.
        let findings = vec![
            finding_id("wu-a", "src/a.rs", Some("A::f"), None),
            finding("wu-b", "src/b.rs"),
        ];
        let ledger = vec![record_full(&["src/a.rs"], Some("A::f"), None, "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert_eq!(report.uncaptured.len(), 1);
        assert_eq!(report.uncaptured[0].work_unit_id, "wu-b");
    }

    // ── work unit b7f6a8c0 ────────────────────────────────────────────────────
    #[test]
    fn test_two_or_more_uncaptured_findings_all_listed_and_not_ok() {
        // wu-a: file+symbol match. wu-b and wu-c: no covering record.
        let findings = vec![
            finding_id("wu-a", "src/a.rs", Some("A::f"), None),
            finding("wu-b", "src/b.rs"),
            finding("wu-c", "src/c.rs"),
        ];
        let ledger = vec![record_full(&["src/a.rs"], Some("A::f"), None, "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        let got: HashSet<&str> = report
            .uncaptured
            .iter()
            .map(|e| e.work_unit_id.as_str())
            .collect();
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
        // Both findings carry symbols matching their respective records.
        let findings = vec![
            finding_id("wu-a", "src/a.rs", Some("A::f"), None),
            finding_id("wu-b", "src/b.rs", Some("B::g"), None),
        ];
        let ledger = vec![
            record_full(&["src/a.rs"], Some("A::f"), None, "open"),
            record_full(&["src/b.rs"], Some("B::g"), None, "open"),
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
        let got: HashSet<&str> = report
            .uncaptured
            .iter()
            .map(|e| e.work_unit_id.as_str())
            .collect();
        assert_eq!(got, HashSet::from(["wu-a", "wu-b"]));
    }

    // ── work unit 0cebf315 ────────────────────────────────────────────────────
    #[test]
    fn test_capture_matches_when_target_file_is_any_element_of_suspect_files() {
        // Finding file is the 3rd element; record also carries matching symbol.
        let findings = vec![finding_id("wu-1", "src/c.rs", Some("C::g"), None)];
        let ledger = vec![record_full(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            Some("C::g"),
            None,
            "open",
        )];
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

        let nfc_field = v
            .get("no_findings_checked")
            .expect("key 'no_findings_checked' must exist");
        assert!(nfc_field.is_boolean(), "no_findings_checked must be a boolean");

        let uncaptured = v.get("uncaptured").expect("key 'uncaptured' must exist");
        assert!(uncaptured.is_array(), "uncaptured must be an array");

        let entry = &uncaptured[0];
        assert!(
            entry
                .get("work_unit_id")
                .map(|v| v.is_string())
                .unwrap_or(false),
            "uncaptured[0].work_unit_id must be a string"
        );
        assert!(
            entry
                .get("target_file")
                .map(|v| v.is_string())
                .unwrap_or(false),
            "uncaptured[0].target_file must be a string"
        );
    }

    // ── work unit 3a4b5c6d ────────────────────────────────────────────────────
    #[test]
    fn test_capture_matches_when_target_file_is_in_a_later_record() {
        // Only the third (last) record covers the file and matches the symbol.
        let findings = vec![finding_id("wu-1", "src/foo.rs", Some("Foo::bar"), None)];
        let ledger = vec![
            record(&["src/bar.rs"], "open"),
            record(&["src/baz.rs"], "open"),
            record_full(&["src/foo.rs"], Some("Foo::bar"), None, "open"),
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

    // ── work unit 7e1c9a02 ────────────────────────────────────────────────────
    /// RED: file matches but symbol differs → UNCAPTURED under tightened contract.
    #[test]
    fn test_same_file_different_symbol_is_uncaptured() {
        let findings = vec![finding_id("wu-distinct", "src/foo.rs", Some("Foo::bar"), None)];
        // Record covers the same file but has a different symbol.
        let ledger = vec![record_full(
            &["src/foo.rs"],
            Some("Other::thing"),
            None,
            "open",
        )];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(report
            .uncaptured
            .iter()
            .any(|e| e.work_unit_id == "wu-distinct"));
    }

    // ── work unit 8f2d0b13 ────────────────────────────────────────────────────
    /// RED: file+symbol match but seed differs → UNCAPTURED under tightened contract.
    #[test]
    fn test_same_file_same_symbol_different_seed_is_uncaptured() {
        let findings = vec![finding_id(
            "wu-seed",
            "src/foo.rs",
            Some("Foo::bar"),
            Some("bar returns Err on empty input"),
        )];
        // Record has matching file and symbol but a different seed.
        let ledger = vec![record_full(
            &["src/foo.rs"],
            Some("Foo::bar"),
            Some("bar returns Ok on empty input"),
            "open",
        )];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(report
            .uncaptured
            .iter()
            .any(|e| e.work_unit_id == "wu-seed"));
    }

    // ── work unit 9a3e1c24 ────────────────────────────────────────────────────
    /// RED: file on record A, symbol on record B — no single record satisfies both → UNCAPTURED.
    #[test]
    fn test_file_on_one_record_identity_on_another_is_uncaptured() {
        let findings = vec![finding_id("wu-split", "src/foo.rs", Some("Foo::bar"), None)];
        let ledger = vec![
            // Record A: covers the file but has a different symbol.
            record_full(&["src/foo.rs"], Some("Other::thing"), None, "open"),
            // Record B: has the matching symbol but a different file.
            record_full(&["src/elsewhere.rs"], Some("Foo::bar"), None, "open"),
        ];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(report
            .uncaptured
            .iter()
            .any(|e| e.work_unit_id == "wu-split"));
    }

    // ── work unit ab4f2d35 ────────────────────────────────────────────────────
    #[test]
    fn test_file_and_symbol_match_no_seed_is_captured() {
        // Finding: symbol present, seed absent. Record: matching file and symbol, no seed.
        let findings = vec![finding_id("wu-sym", "src/foo.rs", Some("Foo::bar"), None)];
        let ledger = vec![record_full(&["src/foo.rs"], Some("Foo::bar"), None, "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
        assert!(!report.no_findings_checked);
    }

    // ── work unit bc5a3e46 ────────────────────────────────────────────────────
    #[test]
    fn test_file_symbol_and_seed_all_match_is_captured() {
        // All three fields (file, symbol, seed) match verbatim on a single record.
        let findings = vec![finding_id(
            "wu-full",
            "src/foo.rs",
            Some("Foo::bar"),
            Some("bar returns Err on empty input"),
        )];
        let ledger = vec![record_full(
            &["src/foo.rs"],
            Some("Foo::bar"),
            Some("bar returns Err on empty input"),
            "open",
        )];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
        assert!(!report.no_findings_checked);
    }

    // ── work unit cd6b4f57 ────────────────────────────────────────────────────
    #[test]
    fn test_seed_matches_after_normalization_when_no_symbol() {
        // Finding has no symbol; seed differs from record only by whitespace/case.
        // Normalization: trim + lowercase + collapse internal whitespace runs to one space.
        let findings = vec![finding_id(
            "wu-norm",
            "src/foo.rs",
            None,
            Some("  Bar  Returns   Err on Empty  "),
        )];
        // Record seed is already the normalized form.
        let ledger = vec![record_full(
            &["src/foo.rs"],
            None,
            Some("bar returns err on empty"),
            "open",
        )];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(report.ok);
        assert!(report.uncaptured.is_empty());
        assert!(!report.no_findings_checked);
    }

    // ── work unit de7c5a68 ────────────────────────────────────────────────────
    /// RED: finding has NO identity beyond target_file → UNCAPTURED (safe direction).
    #[test]
    fn test_finding_with_only_target_file_is_uncaptured() {
        // Bare finding: no symbol, no seed. Record covers the file with some identity.
        let findings = vec![finding("wu-bare", "src/foo.rs")];
        let ledger = vec![record_full(
            &["src/foo.rs"],
            Some("Foo::bar"),
            Some("some seed"),
            "open",
        )];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(report
            .uncaptured
            .iter()
            .any(|e| e.work_unit_id == "wu-bare"));
    }

    // ── work unit ef8d6b79 ────────────────────────────────────────────────────
    /// RED: symbol differs only by case → UNCAPTURED (symbol matching is exact, case-sensitive).
    #[test]
    fn test_symbol_differing_only_by_case_is_uncaptured() {
        // Finding has "Foo::Bar" (capitalized); record has "foo::bar" (lowercase).
        let findings = vec![finding_id("wu-case", "src/foo.rs", Some("Foo::Bar"), None)];
        let ledger = vec![record_full(&["src/foo.rs"], Some("foo::bar"), None, "open")];
        let report = check_surfaced_bugs_captured(&findings, &ledger);
        assert!(!report.ok);
        assert!(!report.no_findings_checked);
        assert!(report
            .uncaptured
            .iter()
            .any(|e| e.work_unit_id == "wu-case"));
    }

    // ── work unit fa9e7c8a ────────────────────────────────────────────────────
    #[test]
    fn test_bug_record_deserializes_with_missing_identity_fields_as_none() {
        // Ledger record predates identity fields — only suspect_files and status present.
        let raw = json!({
            "suspect_files": ["src/foo.rs"],
            "status": "open"
        });
        let r: BugRecord = serde_json::from_value(raw).unwrap();
        assert_eq!(r.suspect_files, vec!["src/foo.rs"]);
        assert_eq!(r.status, "open");
        assert!(r.suspect_symbol.is_none());
        assert!(r.intended_behavior_seed.is_none());
    }
}
