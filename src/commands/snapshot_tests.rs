use crate::common::json_io::write_json_file;
use crate::common::walk::walk_source_files;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub out_file: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestFileEntry {
    pub path_absolute: PathBuf,
    pub path_relative: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestSnapshot {
    pub repo_root: PathBuf,
    pub captured_at: String,
    pub file_count: usize,
    pub files: Vec<TestFileEntry>,
}

/// Detects test files in `repo_root` (Rust `.rs` under `tests/` or containing
/// `#[test]` / `#[cfg(test)]`; C# `.cs` under `*.Tests*` dirs or containing
/// `[Fact` / `[Theory` / `[TestMethod`), SHA-256-hashes each, and returns a
/// manifest. Excluded dirs pruned at descent: `target`, `node_modules`,
/// `bin`, `obj`, `.git`, `.claude-regression`.
pub fn snapshot_test_files(repo_root: &Path) -> anyhow::Result<TestSnapshot> {
    let excluded = [
        "target",
        "node_modules",
        "bin",
        "obj",
        ".git",
        ".claude-regression",
    ];

    let rs_files = walk_source_files(repo_root, &excluded, &["rs"])
        .with_context(|| format!("walk .rs files under {}", repo_root.display()))?;
    let cs_files = walk_source_files(repo_root, &excluded, &["cs"])
        .with_context(|| format!("walk .cs files under {}", repo_root.display()))?;

    let mut detected: BTreeSet<PathBuf> = BTreeSet::new();
    for path in rs_files {
        if is_rust_test_file(&path) {
            detected.insert(path);
        }
    }
    for path in cs_files {
        if is_csharp_test_file(&path) {
            detected.insert(path);
        }
    }

    let mut files = Vec::with_capacity(detected.len());
    for abs in detected {
        let content = fs::read(&abs).with_context(|| format!("read {}", abs.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let hash = format!("{:X}", hasher.finalize());
        let rel = abs.strip_prefix(repo_root).unwrap_or(&abs).to_path_buf();
        files.push(TestFileEntry {
            path_absolute: abs,
            path_relative: rel,
            sha256: hash,
            size_bytes: content.len() as u64,
        });
    }

    Ok(TestSnapshot {
        repo_root: repo_root.to_path_buf(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        file_count: files.len(),
        files,
    })
}

fn is_rust_test_file(path: &Path) -> bool {
    if path
        .components()
        .any(|c| c.as_os_str().to_str() == Some("tests"))
    {
        return true;
    }
    let content = fs::read_to_string(path).unwrap_or_default();
    content.contains("#[test]") || content.contains("#[cfg(test)]")
}

fn is_csharp_test_file(path: &Path) -> bool {
    if path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(is_csharp_test_dir_name)
            .unwrap_or(false)
    }) {
        return true;
    }
    let content = fs::read_to_string(path).unwrap_or_default();
    content.contains("[Fact") || content.contains("[Theory") || content.contains("[TestMethod")
}

fn is_csharp_test_dir_name(name: &str) -> bool {
    name.ends_with(".Tests")
        || name.ends_with(".Test")
        || name.ends_with(".UnitTests")
        || name.ends_with(".UnitTest")
        || name.ends_with(".IntegrationTests")
        || name.ends_with(".IntegrationTest")
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let snapshot = snapshot_test_files(&args.repo_root)?;
    write_json_file(&args.out_file, &snapshot)?;
    let summary = serde_json::json!({
        "snapshot_path": args.out_file,
        "file_count": snapshot.file_count,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_repo_has_zero_files() {
        let td = TempDir::new().unwrap();
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 0);
    }

    #[test]
    fn rust_file_with_test_attr_is_detected() {
        let td = TempDir::new().unwrap();
        write(&td.path().join("src").join("lib.rs"), "#[test] fn t() {}");
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 1);
    }

    #[test]
    fn rust_file_with_cfg_test_attr_is_detected() {
        let td = TempDir::new().unwrap();
        write(
            &td.path().join("src").join("lib.rs"),
            "#[cfg(test)] mod tests {}",
        );
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 1);
    }

    #[test]
    fn rust_file_under_tests_dir_is_detected_without_attrs() {
        let td = TempDir::new().unwrap();
        write(
            &td.path().join("crate").join("tests").join("integration.rs"),
            "// no test attr",
        );
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 1);
    }

    #[test]
    fn rust_file_without_attrs_outside_tests_dir_is_not_detected() {
        let td = TempDir::new().unwrap();
        write(
            &td.path().join("src").join("lib.rs"),
            "pub fn ordinary_code() {}",
        );
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 0);
    }

    #[test]
    fn csharp_file_with_fact_is_detected() {
        let td = TempDir::new().unwrap();
        write(
            &td.path().join("src").join("App.cs"),
            "[Fact] public void T() {}",
        );
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 1);
    }

    #[test]
    fn csharp_file_under_tests_project_dir_is_detected() {
        let td = TempDir::new().unwrap();
        write(
            &td.path().join("MyApp.Tests").join("FooTests.cs"),
            "// no attribute",
        );
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 1);
    }

    #[test]
    fn csharp_file_without_attrs_outside_tests_dir_is_not_detected() {
        let td = TempDir::new().unwrap();
        write(&td.path().join("src").join("Lib.cs"), "public class C {}");
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 0);
    }

    #[test]
    fn files_under_excluded_dirs_are_ignored() {
        let td = TempDir::new().unwrap();
        for excluded in ["target", "node_modules", "bin", "obj", ".git", ".claude-regression"] {
            write(
                &td.path().join(excluded).join("trap.rs"),
                "#[test] fn t() {}",
            );
        }
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(s.file_count, 0, "excluded dirs leaked: {:?}", s.files);
    }

    #[test]
    fn sha256_of_empty_file_matches_known_value() {
        let td = TempDir::new().unwrap();
        write(&td.path().join("crate").join("tests").join("a.rs"), "");
        let s = snapshot_test_files(td.path()).unwrap();
        assert_eq!(
            s.files[0].sha256,
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
        assert_eq!(s.files[0].size_bytes, 0);
    }

    #[test]
    fn captured_at_is_rfc3339_format() {
        let td = TempDir::new().unwrap();
        let s = snapshot_test_files(td.path()).unwrap();
        assert!(s.captured_at.contains('T'));
        assert!(s.captured_at.starts_with(char::is_numeric));
    }

    #[test]
    fn relative_paths_are_relative_to_repo_root() {
        let td = TempDir::new().unwrap();
        write(&td.path().join("crate").join("tests").join("foo.rs"), "");
        let s = snapshot_test_files(td.path()).unwrap();
        assert!(!s.files[0].path_relative.is_absolute());
        assert!(s
            .files[0]
            .path_relative
            .to_string_lossy()
            .contains("foo.rs"));
    }
}
