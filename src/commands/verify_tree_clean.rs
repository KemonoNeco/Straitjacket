use crate::common::subprocess::run_with_timeout;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct TreeStatus {
    pub clean: bool,
    pub dirty_files: Vec<String>,
}

/// Parses lines of `git status --porcelain` output and returns the list of
/// file paths. Each non-blank line has the form `XY <path>` (two status chars
/// then a space then the path). Returns an empty Vec for empty/whitespace input.
pub fn parse_porcelain(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.get(3..).unwrap_or("").trim_start().to_string())
        .collect()
}

/// Derives a `TreeStatus` from a list of dirty file paths. When `dirty_files`
/// is empty the tree is clean; otherwise it is dirty.
pub fn derive_tree_status(dirty_files: Vec<String>) -> TreeStatus {
    TreeStatus {
        clean: dirty_files.is_empty(),
        dirty_files,
    }
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let result = run_with_timeout(
        "git",
        &["status", "--porcelain"],
        &args.repo_root,
        Duration::from_secs(30),
    )?;
    let dirty_files = parse_porcelain(&result.combined_output);
    let status = derive_tree_status(dirty_files);
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_porcelain ---

    #[test]
    fn parse_porcelain_extracts_paths_from_mixed_status_lines() {
        // Input is in non-alphabetic order to catch any sorting side-effect.
        let result = parse_porcelain(" M src/foo.rs\n?? new.txt\n");
        assert_eq!(result, vec!["src/foo.rs", "new.txt"]);
    }

    #[test]
    fn parse_porcelain_returns_empty_vec_for_empty_input() {
        let result = parse_porcelain("");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_porcelain_returns_empty_vec_for_whitespace_only_input() {
        let result = parse_porcelain("   \n");
        assert!(result.is_empty());
    }

    // --- derive_tree_status ---

    #[test]
    fn derive_tree_status_is_clean_when_dirty_files_is_empty() {
        let status = derive_tree_status(vec![]);
        assert!(status.clean);
        assert!(status.dirty_files.is_empty());
    }

    #[test]
    fn derive_tree_status_is_dirty_when_files_are_present() {
        let status = derive_tree_status(vec!["src/main.rs".to_string()]);
        assert!(!status.clean);
        assert_eq!(status.dirty_files, vec!["src/main.rs"]);
    }
}
