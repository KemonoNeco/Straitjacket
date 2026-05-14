use crate::common::json_io::read_json_file;
use crate::common::subprocess::{run_with_timeout, RunResult};
use crate::common::Stack;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PerUnitResult {
    pub work_unit_id: String,
    pub output_file_path: String,
    pub passed: bool,
    pub diagnostics_excerpt: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyNewTestsCompileResult {
    pub all_passed: bool,
    pub per_unit_results: Vec<PerUnitResult>,
    pub log_path: PathBuf,
}

/// Walks up from `start` looking for `Cargo.toml`. Returns the directory containing it.
pub fn find_enclosing_cargo_project(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if cur.join("Cargo.toml").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Walks up from `start` looking for the first `*.csproj`. Returns the path to the csproj.
pub fn find_enclosing_csproj(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if let Ok(rd) = fs::read_dir(&cur) {
            for entry in rd.flatten() {
                let path = entry.path();
                // A directory named `Spurious.csproj/` would otherwise match by extension —
                // require an actual file before returning.
                let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
                if is_file && path.extension().and_then(|e| e.to_str()) == Some("csproj") {
                    return Some(path);
                }
            }
        }
        if !cur.pop() {
            return None;
        }
    }
}

#[derive(Debug, Clone)]
struct UnitInfo {
    id: String,
    output_file_path: String,
    abs_path: PathBuf,
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

fn first_n_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n")
}

pub fn verify_new_tests_compile(
    repo_root: &Path,
    work_units_file: &Path,
    stack: Stack,
    log_dir: &Path,
) -> anyhow::Result<VerifyNewTestsCompileResult> {
    fs::create_dir_all(log_dir)?;
    let log_path = log_dir.join("verify_new_tests_compile.log");
    fs::write(
        &log_path,
        format!(
            "verify_new_tests_compile started {}\n",
            chrono::Utc::now().to_rfc3339()
        ),
    )?;

    let work_units: serde_json::Value = read_json_file(work_units_file)
        .with_context(|| format!("read {}", work_units_file.display()))?;
    let units_array = work_units
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![work_units.clone()]);

    let candidates: Vec<UnitInfo> = units_array
        .iter()
        .filter_map(|u| {
            let status = u.get("status").and_then(|s| s.as_str()).unwrap_or("");
            if !matches!(status, "written" | "rejected_lint" | "pending") {
                return None;
            }
            let id = u.get("id")?.as_str()?.to_string();
            let ofp = u.get("output_file_path")?.as_str()?.to_string();
            let abs = if Path::new(&ofp).is_absolute() {
                PathBuf::from(&ofp)
            } else {
                repo_root.join(&ofp)
            };
            if !abs.is_file() {
                return None;
            }
            Some(UnitInfo {
                id,
                output_file_path: ofp,
                abs_path: abs,
            })
        })
        .collect();

    if candidates.is_empty() {
        return Ok(VerifyNewTestsCompileResult {
            all_passed: true,
            per_unit_results: vec![],
            log_path,
        });
    }

    let mut rust_groups: BTreeMap<PathBuf, Vec<UnitInfo>> = BTreeMap::new();
    let mut csharp_groups: BTreeMap<PathBuf, Vec<UnitInfo>> = BTreeMap::new();

    for unit in &candidates {
        match unit
            .abs_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
        {
            Some(ref ext) if ext == "rs" => {
                if let Some(project) = find_enclosing_cargo_project(&unit.abs_path) {
                    rust_groups.entry(project).or_default().push(unit.clone());
                }
            }
            Some(ref ext) if ext == "cs" => {
                if let Some(project) = find_enclosing_csproj(&unit.abs_path) {
                    csharp_groups.entry(project).or_default().push(unit.clone());
                }
            }
            _ => {}
        }
    }

    let mut per_unit_results: Vec<PerUnitResult> = Vec::new();
    let mut all_passed = true;

    if matches!(stack, Stack::Rust | Stack::Both) {
        for (project, units) in &rust_groups {
            let check_r = run_step(
                "cargo check --tests",
                "cargo",
                &["check", "--tests"],
                project,
                &log_path,
            )?;
            let clippy_r = run_step(
                "cargo clippy --tests -- -D warnings",
                "cargo",
                &["clippy", "--tests", "--", "-D", "warnings"],
                project,
                &log_path,
            )?;
            let project_passed = check_r.exit_code == 0 && clippy_r.exit_code == 0;
            if !project_passed {
                all_passed = false;
            }
            let excerpt = if project_passed {
                String::new()
            } else if check_r.exit_code != 0 {
                first_n_lines(&check_r.combined_output, 50)
            } else {
                first_n_lines(&clippy_r.combined_output, 50)
            };
            for u in units {
                per_unit_results.push(PerUnitResult {
                    work_unit_id: u.id.clone(),
                    output_file_path: u.output_file_path.clone(),
                    passed: project_passed,
                    diagnostics_excerpt: excerpt.clone(),
                });
            }
        }
    }

    if matches!(stack, Stack::Csharp | Stack::Both) {
        for (csproj, units) in &csharp_groups {
            let r = run_step(
                "dotnet build",
                "dotnet",
                &[
                    "build",
                    "--nologo",
                    "--verbosity",
                    "minimal",
                    csproj.to_str().unwrap_or(""),
                ],
                repo_root,
                &log_path,
            )?;
            let project_passed = r.exit_code == 0;
            if !project_passed {
                all_passed = false;
            }
            let excerpt = if project_passed {
                String::new()
            } else {
                first_n_lines(&r.combined_output, 50)
            };
            for u in units {
                per_unit_results.push(PerUnitResult {
                    work_unit_id: u.id.clone(),
                    output_file_path: u.output_file_path.clone(),
                    passed: project_passed,
                    diagnostics_excerpt: excerpt.clone(),
                });
            }
        }
    }

    Ok(VerifyNewTestsCompileResult {
        all_passed,
        per_unit_results,
        log_path,
    })
}

fn run_step(
    label: &str,
    cmd: &str,
    args: &[&str],
    cwd: &Path,
    log_path: &Path,
) -> anyhow::Result<RunResult> {
    let r = run_with_timeout(cmd, args, cwd, Duration::from_secs(900))?;
    append_section(log_path, &format!("{} (exit {})", label, r.exit_code), &r.combined_output)?;
    Ok(r)
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let r = verify_new_tests_compile(&args.repo_root, &args.work_units_file, args.stack, &args.log_dir)?;
    println!("{}", serde_json::to_string_pretty(&r)?);
    if r.all_passed {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn find_cargo_project_walks_up() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let nested = root.join("src").join("lib.rs");
        fs::write(&nested, "").unwrap();
        assert_eq!(find_enclosing_cargo_project(&nested), Some(root.to_path_buf()));
    }

    #[test]
    fn find_cargo_project_returns_none_when_absent() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("orphan");
        fs::create_dir_all(&dir).unwrap();
        assert!(find_enclosing_cargo_project(&dir).is_none());
    }

    #[test]
    fn find_csproj_walks_up() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        fs::write(root.join("MyApp.Tests.csproj"), "<Project></Project>").unwrap();
        let nested = root.join("FooTests.cs");
        fs::write(&nested, "").unwrap();
        let found = find_enclosing_csproj(&nested).unwrap();
        assert_eq!(found, root.join("MyApp.Tests.csproj"));
    }

    #[test]
    fn first_n_lines_returns_compact_excerpt() {
        let s = "a\nb\nc\nd\ne\nf";
        assert_eq!(first_n_lines(s, 3), "a\nb\nc");
        assert_eq!(first_n_lines(s, 10), s);
        assert_eq!(first_n_lines("", 5), "");
    }

    /// find_enclosing_csproj must only return paths to actual files with the .csproj
    /// extension, not directories that happen to have a .csproj name suffix.
    ///
    /// Precondition: td/Spurious.csproj/ is a DIRECTORY (not a file), and there are
    /// no real *.csproj files anywhere in the temp tree. Expected: None.
    ///
    /// NOTE: The current implementation does NOT call is_file() on matched entries,
    /// only checks the extension. This test is expected to FAIL against the current
    /// source, thereby surfacing the bug.
    #[test]
    fn test_find_enclosing_csproj_ignores_directories_with_csproj_suffix() {
        let td = TempDir::new().unwrap();
        let root = td.path();

        // Create a directory whose name ends in .csproj (not a file).
        let spurious_dir = root.join("Spurious.csproj");
        fs::create_dir_all(&spurious_dir).unwrap();

        // Plant a dummy file inside so we have a real path to start the walk from.
        let dummy = spurious_dir.join("dummy.txt");
        fs::write(&dummy, b"").unwrap();

        // Expect None because Spurious.csproj is a directory, not a file.
        assert_eq!(
            find_enclosing_csproj(&dummy),
            None,
            "find_enclosing_csproj must not return a directory path — only real .csproj files"
        );
    }

    /// When a work_units.json entry references an output_file_path that does not exist
    /// on disk, verify_new_tests_compile must silently skip it (filtered at the
    /// abs.is_file() check) rather than panicking or erroring.
    ///
    /// Expected: returns Ok, all_passed == true, per_unit_results is empty (no candidates).
    #[test]
    fn test_verify_new_tests_compile_skips_units_whose_output_file_does_not_exist() {
        let td = TempDir::new().unwrap();
        let repo_root = td.path();

        // Work-units.json with status "written" so the entry passes the status filter,
        // but output_file_path points to a file that does not exist on disk.
        let work_units_json = serde_json::json!([
            {
                "id": "deadbeef-0000-0000-0000-000000000001",
                "status": "written",
                "output_file_path": "src/does_not_exist.rs"
            }
        ]);
        let work_units_file = repo_root.join("work-units.json");
        fs::write(&work_units_file, serde_json::to_string_pretty(&work_units_json).unwrap()).unwrap();

        let log_dir = repo_root.join("logs");

        let result = verify_new_tests_compile(
            repo_root,
            &work_units_file,
            Stack::Rust,
            &log_dir,
        )
        .expect("verify_new_tests_compile must return Ok even when output_file_path is missing");

        assert!(
            result.all_passed,
            "all_passed must be true when there are no valid candidates"
        );
        assert!(
            result.per_unit_results.is_empty(),
            "per_unit_results must be empty when the missing-file entry is filtered out; got {:?}",
            result.per_unit_results
        );
    }
}
