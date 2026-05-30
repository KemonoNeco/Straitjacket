use crate::common::Stack;
use crate::common::walk::{keep_entry, walk_source_files, SOURCE_TREE_EXCLUDES};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long, value_enum)]
    pub stack: Stack,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RustCrateInfo {
    pub crate_root: PathBuf,
    pub manifest: PathBuf,
    pub has_fuzz_dir: bool,
    pub existing_fuzz_targets: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RustFuzzInfo {
    pub cargo_fuzz_available: bool,
    pub crates: Vec<RustCrateInfo>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CsharpFuzzInfo {
    pub sharpfuzz_available: bool,
    pub csproj_paths: Vec<PathBuf>,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FuzzSetupResult {
    pub rust: Option<RustFuzzInfo>,
    pub csharp: Option<CsharpFuzzInfo>,
}

fn probe_tool(cmd: &str, arg: &str) -> bool {
    Command::new(cmd)
        .arg(arg)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn find_rust_crates(repo_root: &Path) -> std::io::Result<Vec<RustCrateInfo>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(repo_root)
        .into_iter()
        .filter_entry(|e| keep_entry(e, SOURCE_TREE_EXCLUDES))
        .filter_map(|r| r.ok())
    {
        if entry.file_type().is_file() && entry.file_name().to_str() == Some("Cargo.toml") {
            let crate_root = entry.path().parent().unwrap().to_path_buf();
            let fuzz_dir = crate_root.join("fuzz");
            let has_fuzz_dir = fuzz_dir.is_dir();
            let mut existing = Vec::new();
            if has_fuzz_dir {
                let targets_dir = fuzz_dir.join("fuzz_targets");
                if targets_dir.is_dir() {
                    if let Ok(rd) = std::fs::read_dir(&targets_dir) {
                        for f in rd.flatten() {
                            if f.path().extension().and_then(|e| e.to_str()) == Some("rs") {
                                if let Some(stem) =
                                    f.path().file_stem().and_then(|s| s.to_str())
                                {
                                    existing.push(stem.to_string());
                                }
                            }
                        }
                    }
                }
            }
            existing.sort();
            out.push(RustCrateInfo {
                crate_root,
                manifest: entry.path().to_path_buf(),
                has_fuzz_dir,
                existing_fuzz_targets: existing,
            });
        }
    }
    out.sort_by(|a, b| a.manifest.cmp(&b.manifest));
    Ok(out)
}

pub fn probe_fuzz_setup(repo_root: &Path, stack: Stack) -> anyhow::Result<FuzzSetupResult> {
    let rust = if matches!(stack, Stack::Rust | Stack::Both) {
        // Two-arm probe, decided by `decide_cargo_fuzz_available`. Availability hinges
        // solely on the `--version` arm: cargo-fuzz always demands a subcommand and
        // exits 2 when none is given, so the bare `cargo fuzz` probe is informational
        // only and must NOT gate the decision (a naive `&&` would short-circuit a real
        // install to "unavailable"). Stdout/stderr are nulled on both probes to avoid
        // noisy output AND to dodge the cargo-fuzz v0.13.1 / is-terminal v0.4.1
        // terminal-width panic on some Windows consoles (the redirected handles aren't
        // classified as terminals, so the probe is skipped).
        let no_subcommand_ok = probe_tool("cargo", "fuzz");
        let version_ok = Command::new("cargo")
            .args(["fuzz", "--version"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let cargo_fuzz_available = decide_cargo_fuzz_available(no_subcommand_ok, version_ok);
        let crates = find_rust_crates(repo_root)?;
        Some(RustFuzzInfo {
            cargo_fuzz_available,
            crates,
        })
    } else {
        None
    };

    let csharp = if matches!(stack, Stack::Csharp | Stack::Both) {
        let sharpfuzz_available = probe_tool("sharpfuzz", "--version");
        let csproj_paths =
            walk_source_files(repo_root, SOURCE_TREE_EXCLUDES, &["csproj"]).unwrap_or_default();
        let note = if sharpfuzz_available {
            "SharpFuzz detected; harness author may proceed.".to_string()
        } else {
            "SharpFuzz not installed. Install with: dotnet tool install --global SharpFuzz.CommandLine. \
             Fuzzing on C# is materially less mature than Rust; expected absence."
                .to_string()
        };
        Some(CsharpFuzzInfo {
            sharpfuzz_available,
            csproj_paths,
            note,
        })
    } else {
        None
    };

    Ok(FuzzSetupResult { rust, csharp })
}

/// Decides whether cargo-fuzz is available based on injected probe results.
/// `no_subcommand_ok`: whether `cargo fuzz` (no subcommand) exited zero.
/// `version_ok`: whether `cargo fuzz --version` exited zero.
/// Availability is determined solely by `version_ok`; cargo-fuzz always demands a
/// subcommand and exits 2 when none is given, so `no_subcommand_ok` is irrelevant.
pub fn decide_cargo_fuzz_available(no_subcommand_ok: bool, version_ok: bool) -> bool {
    let _ = no_subcommand_ok;
    version_ok
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let result = probe_fuzz_setup(&args.repo_root, args.stack)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_repo_returns_no_crates() {
        let td = TempDir::new().unwrap();
        let crates = find_rust_crates(td.path()).unwrap();
        assert!(crates.is_empty());
    }

    #[test]
    fn single_crate_without_fuzz_dir() {
        let td = TempDir::new().unwrap();
        write_file(&td.path().join("Cargo.toml"), "[package]\nname = \"x\"\n");
        let crates = find_rust_crates(td.path()).unwrap();
        assert_eq!(crates.len(), 1);
        assert!(!crates[0].has_fuzz_dir);
        assert!(crates[0].existing_fuzz_targets.is_empty());
    }

    #[test]
    fn single_crate_with_fuzz_dir_lists_existing_targets() {
        let td = TempDir::new().unwrap();
        write_file(&td.path().join("Cargo.toml"), "[package]\nname = \"x\"\n");
        write_file(
            &td.path()
                .join("fuzz")
                .join("fuzz_targets")
                .join("parse_header.rs"),
            "// harness",
        );
        write_file(
            &td.path()
                .join("fuzz")
                .join("fuzz_targets")
                .join("decode_packet.rs"),
            "// harness",
        );
        let crates = find_rust_crates(td.path()).unwrap();
        assert_eq!(crates.len(), 1);
        assert!(crates[0].has_fuzz_dir);
        assert_eq!(
            crates[0].existing_fuzz_targets,
            vec!["decode_packet".to_string(), "parse_header".to_string()]
        );
    }

    #[test]
    fn cargo_toml_under_target_is_excluded() {
        let td = TempDir::new().unwrap();
        write_file(
            &td.path().join("target").join("Cargo.toml"),
            "[package]\nname = \"trap\"\n",
        );
        let crates = find_rust_crates(td.path()).unwrap();
        assert!(crates.is_empty());
    }

    #[test]
    fn workspace_with_multiple_crates_lists_all() {
        let td = TempDir::new().unwrap();
        write_file(&td.path().join("Cargo.toml"), "[workspace]\n");
        write_file(
            &td.path().join("crates").join("foo").join("Cargo.toml"),
            "[package]\nname = \"foo\"\n",
        );
        write_file(
            &td.path().join("crates").join("bar").join("Cargo.toml"),
            "[package]\nname = \"bar\"\n",
        );
        let crates = find_rust_crates(td.path()).unwrap();
        assert_eq!(crates.len(), 3, "root workspace + 2 members");
    }

    #[test]
    fn probe_fuzz_setup_for_rust_stack_does_not_panic() {
        let td = TempDir::new().unwrap();
        write_file(&td.path().join("Cargo.toml"), "[package]\nname = \"x\"\n");
        let r = probe_fuzz_setup(td.path(), Stack::Rust).unwrap();
        assert!(r.rust.is_some());
        assert!(r.csharp.is_none());
    }

    #[test]
    fn probe_fuzz_setup_for_csharp_stack_returns_csharp_info_only() {
        let td = TempDir::new().unwrap();
        let r = probe_fuzz_setup(td.path(), Stack::Csharp).unwrap();
        assert!(r.rust.is_none());
        assert!(r.csharp.is_some());
        // sharpfuzz_available depends on system; just verify the note field is populated
        assert!(!r.csharp.unwrap().note.is_empty());
    }

    #[test]
    fn probe_fuzz_setup_for_both_returns_both_blocks() {
        let td = TempDir::new().unwrap();
        let r = probe_fuzz_setup(td.path(), Stack::Both).unwrap();
        assert!(r.rust.is_some());
        assert!(r.csharp.is_some());
    }

    // Truth table: cargo-fuzz availability depends solely on version_ok, not
    // no_subcommand_ok. cargo-fuzz always demands a subcommand and exits 2
    // without one, so the no_subcommand_ok arm is irrelevant to availability.
    #[test]
    fn decide_cargo_fuzz_available_is_version_ok_regardless_of_no_subcommand_arm() {
        // Discriminating case: cargo-fuzz installed (version_ok=true) but the
        // no-subcommand probe exits 2 (no_subcommand_ok=false). Must be true.
        assert!(decide_cargo_fuzz_available(false, true));
        // Both probes succeed — also available.
        assert!(decide_cargo_fuzz_available(true, true));
        // Neither probe succeeds — not available.
        assert!(!decide_cargo_fuzz_available(false, false));
        // No-subcommand probe succeeds but version probe fails — not available.
        assert!(!decide_cargo_fuzz_available(true, false));
    }
}
