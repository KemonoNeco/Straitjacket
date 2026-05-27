use crate::commands::snapshot_tests::TestSnapshot;
use crate::common::json_io::read_json_file;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub snapshot_file: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Violation {
    pub path_absolute: PathBuf,
    pub path_relative: PathBuf,
    pub kind: ViolationKind,
    pub original_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_sha256: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ViolationKind {
    Modified,
    Deleted,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyResult {
    pub clean: bool,
    pub violations: Vec<Violation>,
    pub deletions: Vec<Violation>,
    pub checked_count: usize,
    pub no_files_checked: bool,
}

/// Re-hashes every file in `snapshot` and reports mismatches.
/// Modified files go into `violations`; missing files go into `deletions`.
/// `clean` is true iff both lists are empty.
pub fn verify_no_test_mutation(snapshot: &TestSnapshot) -> anyhow::Result<VerifyResult> {
    let mut violations = Vec::new();
    let mut deletions = Vec::new();

    for entry in &snapshot.files {
        let abs = &entry.path_absolute;
        if !abs.exists() {
            deletions.push(Violation {
                path_absolute: abs.clone(),
                path_relative: entry.path_relative.clone(),
                kind: ViolationKind::Deleted,
                original_sha256: entry.sha256.clone(),
                current_sha256: None,
            });
            continue;
        }
        let content = fs::read(abs).with_context(|| format!("re-read {}", abs.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let current = format!("{:X}", hasher.finalize());
        if current != entry.sha256 {
            violations.push(Violation {
                path_absolute: abs.clone(),
                path_relative: entry.path_relative.clone(),
                kind: ViolationKind::Modified,
                original_sha256: entry.sha256.clone(),
                current_sha256: Some(current),
            });
        }
    }

    Ok(VerifyResult {
        clean: violations.is_empty() && deletions.is_empty(),
        checked_count: snapshot.files.len(),
        violations,
        deletions,
        no_files_checked: snapshot.files.is_empty(),
    })
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let snapshot: TestSnapshot = read_json_file(&args.snapshot_file)?;
    let result = verify_no_test_mutation(&snapshot)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    if result.clean {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::snapshot_tests::snapshot_test_files;
    use tempfile::TempDir;

    fn write_file(td: &TempDir, rel: &str, content: &str) -> PathBuf {
        let p = td.path().join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn empty_snapshot_is_clean() {
        let td = TempDir::new().unwrap();
        let snapshot = snapshot_test_files(td.path()).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert!(r.clean);
        assert_eq!(r.checked_count, 0);
    }

    #[test]
    fn untouched_files_are_clean() {
        let td = TempDir::new().unwrap();
        write_file(&td, "tests/foo.rs", "#[test] fn t() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert!(r.clean, "{:?}", r);
        assert_eq!(r.checked_count, 1);
    }

    #[test]
    fn modified_file_is_a_violation() {
        let td = TempDir::new().unwrap();
        let path = write_file(&td, "tests/foo.rs", "#[test] fn t() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        fs::write(&path, "#[test] fn t() { assert!(false); }").unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert!(!r.clean);
        assert_eq!(r.violations.len(), 1);
        assert_eq!(r.deletions.len(), 0);
        let v = &r.violations[0];
        assert_eq!(v.kind, ViolationKind::Modified);
        assert!(v.current_sha256.is_some());
        assert_ne!(v.original_sha256, v.current_sha256.as_deref().unwrap());
    }

    #[test]
    fn deleted_file_is_in_deletions() {
        let td = TempDir::new().unwrap();
        let path = write_file(&td, "tests/foo.rs", "#[test] fn t() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        fs::remove_file(&path).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert!(!r.clean);
        assert_eq!(r.violations.len(), 0);
        assert_eq!(r.deletions.len(), 1);
        assert_eq!(r.deletions[0].kind, ViolationKind::Deleted);
        assert!(r.deletions[0].current_sha256.is_none());
    }

    #[test]
    fn one_modified_one_deleted_one_clean_classified_correctly() {
        let td = TempDir::new().unwrap();
        let _ = write_file(&td, "tests/keep.rs", "#[test] fn a() {}");
        let modify = write_file(&td, "tests/modify.rs", "#[test] fn b() {}");
        let delete = write_file(&td, "tests/delete.rs", "#[test] fn c() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        fs::write(&modify, "#[test] fn b() { panic!(); }").unwrap();
        fs::remove_file(&delete).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert_eq!(r.checked_count, 3);
        assert_eq!(r.violations.len(), 1);
        assert_eq!(r.deletions.len(), 1);
        assert!(!r.clean);
    }

    #[test]
    fn snapshot_round_trip_through_disk_works() {
        let td = TempDir::new().unwrap();
        write_file(&td, "tests/foo.rs", "#[test] fn t() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        let snap_path = td.path().join(".claude-regression").join("snap.json");
        crate::common::json_io::write_json_file(&snap_path, &snapshot).unwrap();
        let restored: TestSnapshot =
            crate::common::json_io::read_json_file(&snap_path).unwrap();
        let r = verify_no_test_mutation(&restored).unwrap();
        assert!(r.clean, "{:?}", r);
    }

    // --- new tests for no_files_checked field ---

    #[test]
    fn test_no_files_checked_signal_is_true_when_snapshot_is_empty() {
        let td = TempDir::new().unwrap();
        let snapshot = snapshot_test_files(td.path()).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert_eq!(r.checked_count, 0);
        assert!(r.no_files_checked);
    }

    #[test]
    fn test_no_files_checked_signal_is_false_with_one_file() {
        let td = TempDir::new().unwrap();
        write_file(&td, "tests/a.rs", "#[test] fn a() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert_eq!(r.checked_count, 1);
        assert!(!r.no_files_checked);
    }

    #[test]
    fn test_no_files_checked_signal_is_false_with_two_files() {
        let td = TempDir::new().unwrap();
        write_file(&td, "tests/a.rs", "#[test] fn a() {}");
        write_file(&td, "tests/b.rs", "#[test] fn b() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert_eq!(r.checked_count, 2);
        assert!(!r.no_files_checked);
    }

    #[test]
    fn test_no_files_checked_is_independent_of_clean_when_violation_present() {
        let td = TempDir::new().unwrap();
        let path = write_file(&td, "tests/a.rs", "#[test] fn a() {}");
        let snapshot = snapshot_test_files(td.path()).unwrap();
        fs::write(&path, "#[test] fn a() { panic!(); }").unwrap();
        let r = verify_no_test_mutation(&snapshot).unwrap();
        assert!(!r.no_files_checked);
        assert!(!r.clean);
        assert!(!r.violations.is_empty());
    }

    #[test]
    fn test_verify_result_serializes_no_files_checked_key() {
        let result = VerifyResult {
            clean: true,
            violations: vec![],
            deletions: vec![],
            checked_count: 0,
            no_files_checked: true,
        };
        let v = serde_json::to_value(&result).unwrap();
        let field = v.get("no_files_checked").expect("key 'no_files_checked' must exist");
        assert!(field.is_boolean(), "no_files_checked must serialize as a boolean, got: {field}");
    }
}
