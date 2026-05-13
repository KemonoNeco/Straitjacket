use crate::common::walk::walk_source_files;
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
}

/// Returns which stacks (rust, csharp, both, none) are present in `repo_root` along
/// with the manifests found.
///   - Rust: `Cargo.toml` at any depth, pruning `target/`.
///   - C# projects: `*.csproj`, pruning `bin/`, `obj/`, `node_modules/`.
///   - C# solutions: `*.sln`, no exclusions.
pub fn detect_stack(repo_root: &Path) -> std::io::Result<DetectStackResult> {
    if !repo_root.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("repo_root not found: {}", repo_root.display()),
        ));
    }
    let rust_manifests = find_named_files(repo_root, "Cargo.toml", &["target"])?;
    let csharp_projects =
        walk_source_files(repo_root, &["bin", "obj", "node_modules"], &["csproj"])?;
    let csharp_solutions = walk_source_files(repo_root, &[], &["sln"])?;

    let has_rust = !rust_manifests.is_empty();
    let has_csharp = !csharp_projects.is_empty() || !csharp_solutions.is_empty();

    let stack = match (has_rust, has_csharp) {
        (true, true) => Stack::Both,
        (true, false) => Stack::Rust,
        (false, true) => Stack::Csharp,
        (false, false) => Stack::None,
    };

    Ok(DetectStackResult {
        stack,
        rust_manifests,
        csharp_projects,
        csharp_solutions,
    })
}

fn find_named_files(
    root: &Path,
    name: &str,
    excluded_dir_names: &[&str],
) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            if e.file_type().is_dir() {
                if let Some(n) = e.file_name().to_str() {
                    return !excluded_dir_names.contains(&n);
                }
            }
            true
        })
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
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"stack\":\"both\""),
            "stack must serialize as lowercase string: {}",
            json
        );
    }
}
