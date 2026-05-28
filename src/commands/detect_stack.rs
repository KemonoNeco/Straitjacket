use crate::common::cargo_target::{resolve_cargo_target, CargoTarget};
use crate::common::walk::{keep_entry, walk_source_files, SOURCE_TREE_EXCLUDES};
use crate::common::Stack;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct DetectStackResult {
    pub stack: Stack,
    pub rust_manifests: Vec<PathBuf>,
    pub csharp_projects: Vec<PathBuf>,
    pub csharp_solutions: Vec<PathBuf>,
    pub cargo_target: CargoTarget,
}

/// Returns which stacks (rust, csharp, both, none) are present in `repo_root` along
/// with the manifests found. All walks prune the canonical `SOURCE_TREE_EXCLUDES` set
/// (build outputs, VCS, Claude Code per-project state).
pub fn detect_stack(repo_root: &Path) -> std::io::Result<DetectStackResult> {
    if !repo_root.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("repo_root not found: {}", repo_root.display()),
        ));
    }
    let rust_manifests = find_named_files(repo_root, "Cargo.toml", SOURCE_TREE_EXCLUDES)?;
    let csharp_projects = walk_source_files(repo_root, SOURCE_TREE_EXCLUDES, &["csproj"])?;
    let csharp_solutions = walk_source_files(repo_root, SOURCE_TREE_EXCLUDES, &["sln"])?;

    let has_rust = !rust_manifests.is_empty();
    let has_csharp = !csharp_projects.is_empty() || !csharp_solutions.is_empty();

    let stack = match (has_rust, has_csharp) {
        (true, true) => Stack::Both,
        (true, false) => Stack::Rust,
        (false, true) => Stack::Csharp,
        (false, false) => Stack::None,
    };

    let cargo_target = resolve_cargo_target(&rust_manifests, repo_root);

    Ok(DetectStackResult {
        stack,
        rust_manifests,
        csharp_projects,
        csharp_solutions,
        cargo_target,
    })
}

fn find_named_files(
    root: &Path,
    name: &str,
    excluded_dir_names: &[&str],
) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| keep_entry(e, excluded_dir_names))
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.file_name().to_str() == Some(name))
        .map(|e| e.into_path())
        .collect();
    out.sort();
    Ok(out)
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let result = detect_stack(&args.repo_root)?;
    let json = serde_json::to_string_pretty(&result)?;
    println!("{}", json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn empty_repo_returns_none() {
        let td = TempDir::new().unwrap();
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.stack, Stack::None);
        assert!(r.rust_manifests.is_empty());
        assert!(r.csharp_projects.is_empty());
        assert!(r.csharp_solutions.is_empty());
    }

    #[test]
    fn rust_only_detects_rust() {
        let td = TempDir::new().unwrap();
        touch(&td.path().join("Cargo.toml"));
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.stack, Stack::Rust);
        assert_eq!(r.rust_manifests.len(), 1);
    }

    #[test]
    fn csharp_csproj_only_detects_csharp() {
        let td = TempDir::new().unwrap();
        touch(&td.path().join("MyApp.csproj"));
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.stack, Stack::Csharp);
        assert_eq!(r.csharp_projects.len(), 1);
        assert!(r.csharp_solutions.is_empty());
    }

    #[test]
    fn csharp_sln_only_detects_csharp() {
        let td = TempDir::new().unwrap();
        touch(&td.path().join("App.sln"));
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.stack, Stack::Csharp);
        assert!(r.csharp_projects.is_empty());
        assert_eq!(r.csharp_solutions.len(), 1);
    }

    #[test]
    fn both_stacks_detected() {
        let td = TempDir::new().unwrap();
        touch(&td.path().join("Cargo.toml"));
        touch(&td.path().join("App.csproj"));
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.stack, Stack::Both);
    }

    #[test]
    fn cargo_toml_under_target_is_excluded() {
        let td = TempDir::new().unwrap();
        // Only a Cargo.toml inside target/ — should be invisible.
        touch(&td.path().join("target").join("Cargo.toml"));
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.stack, Stack::None, "target/Cargo.toml must be ignored");
    }

    #[test]
    fn csproj_under_bin_obj_node_modules_is_excluded() {
        let td = TempDir::new().unwrap();
        touch(&td.path().join("bin").join("Hidden.csproj"));
        touch(&td.path().join("obj").join("Generated.csproj"));
        touch(&td.path().join("node_modules").join("Foo.csproj"));
        let r = detect_stack(td.path()).unwrap();
        assert!(
            r.csharp_projects.is_empty(),
            "bin/obj/node_modules csproj files must be ignored: {:?}",
            r.csharp_projects
        );
        assert_eq!(r.stack, Stack::None);
    }

    #[test]
    fn cargo_toml_in_nested_workspace_is_found() {
        let td = TempDir::new().unwrap();
        touch(&td.path().join("Cargo.toml"));
        touch(&td.path().join("crates").join("foo").join("Cargo.toml"));
        touch(&td.path().join("crates").join("bar").join("Cargo.toml"));
        let r = detect_stack(td.path()).unwrap();
        assert_eq!(r.rust_manifests.len(), 3);
    }

    #[test]
    fn nonexistent_root_returns_err() {
        let td = TempDir::new().unwrap();
        let nonexistent = td.path().join("does_not_exist");
        assert!(detect_stack(&nonexistent).is_err());
    }

    #[test]
    fn stack_serializes_as_lowercase_string() {
        // Defends the JSON output contract against the PowerShell consumer expectations.
        let r = DetectStackResult {
            stack: Stack::Both,
            rust_manifests: vec![],
            csharp_projects: vec![],
            csharp_solutions: vec![],
            cargo_target: CargoTarget::NoRustTarget,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"stack\":\"both\""),
            "stack must serialize as lowercase string: {}",
            json
        );
    }

    // ─── cargo_target integration tests (TDD stubs — RED for 1/2/3, GREEN for 4/5) ─

    #[test]
    fn test_root_cargo_toml_yields_resolved_workspace_cargo_target() {
        // RED: placeholder is NoRustTarget; expects Resolved { workspace: true }.
        let td = TempDir::new().unwrap();
        touch(&td.path().join("Cargo.toml"));
        let result = detect_stack(td.path()).unwrap();
        assert_eq!(
            result.cargo_target,
            CargoTarget::Resolved {
                working_dir: td.path().to_path_buf(),
                workspace: true,
            },
            "root Cargo.toml must produce Resolved {{ working_dir == repo_root, workspace == true }}"
        );
    }

    #[test]
    fn test_single_nested_crate_yields_resolved_non_workspace_cargo_target() {
        // RED: placeholder is NoRustTarget; expects Resolved { workspace: false }.
        let td = TempDir::new().unwrap();
        touch(&td.path().join("cli").join("Cargo.toml"));
        let result = detect_stack(td.path()).unwrap();
        assert_eq!(
            result.cargo_target,
            CargoTarget::Resolved {
                working_dir: td.path().join("cli"),
                workspace: false,
            },
            "single nested Cargo.toml (no root) must produce Resolved {{ working_dir == cli dir, workspace == false }}"
        );
    }

    #[test]
    fn test_multiple_nested_crates_no_root_yields_ambiguous_cargo_target() {
        // RED: placeholder is NoRustTarget; expects Ambiguous.
        let td = TempDir::new().unwrap();
        touch(&td.path().join("cli").join("Cargo.toml"));
        touch(&td.path().join("tools").join("Cargo.toml"));
        let result = detect_stack(td.path()).unwrap();
        match result.cargo_target {
            CargoTarget::Ambiguous { mut candidates } => {
                candidates.sort();
                let mut expected = vec![td.path().join("cli"), td.path().join("tools")];
                expected.sort();
                assert_eq!(
                    candidates, expected,
                    "Ambiguous candidates must be the parent dirs of the manifests"
                );
            }
            other => panic!(
                "expected Ambiguous for multiple nested crates, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_no_rust_manifests_yields_no_rust_target_cargo_target() {
        // GREEN from placeholder: both placeholder and expected value are NoRustTarget.
        let td = TempDir::new().unwrap();
        let result = detect_stack(td.path()).unwrap();
        assert_eq!(
            result.cargo_target,
            CargoTarget::NoRustTarget,
            "empty repo must produce NoRustTarget"
        );
    }

    #[test]
    fn test_detect_stack_result_serializes_cargo_target_with_tagged_shape() {
        // GREEN from placeholder: field + serde shape exist regardless of resolve logic.
        use serde_json::Value;

        // ── Resolved ──
        let resolved = DetectStackResult {
            stack: Stack::Rust,
            rust_manifests: vec![],
            csharp_projects: vec![],
            csharp_solutions: vec![],
            cargo_target: CargoTarget::Resolved {
                working_dir: PathBuf::from("/some/project"),
                workspace: true,
            },
        };
        let v: Value = serde_json::to_value(&resolved).expect("serialize must not fail");
        assert!(
            v.get("cargo_target").is_some(),
            "DetectStackResult must have a cargo_target key; got: {v}"
        );
        assert_eq!(
            v["cargo_target"]["kind"].as_str(),
            Some("resolved"),
            "Resolved must serialize with kind == \"resolved\"; got: {v}"
        );

        // ── Ambiguous ──
        let ambiguous = DetectStackResult {
            stack: Stack::Rust,
            rust_manifests: vec![],
            csharp_projects: vec![],
            csharp_solutions: vec![],
            cargo_target: CargoTarget::Ambiguous {
                candidates: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            },
        };
        let v2: Value = serde_json::to_value(&ambiguous).expect("serialize must not fail");
        assert_eq!(
            v2["cargo_target"]["kind"].as_str(),
            Some("ambiguous"),
            "Ambiguous must serialize with kind == \"ambiguous\"; got: {v2}"
        );

        // ── NoRustTarget ──
        let no_rust = DetectStackResult {
            stack: Stack::None,
            rust_manifests: vec![],
            csharp_projects: vec![],
            csharp_solutions: vec![],
            cargo_target: CargoTarget::NoRustTarget,
        };
        let v3: Value = serde_json::to_value(&no_rust).expect("serialize must not fail");
        assert_eq!(
            v3["cargo_target"]["kind"].as_str(),
            Some("no_rust_target"),
            "NoRustTarget must serialize with kind == \"no_rust_target\"; got: {v3}"
        );
    }
}
