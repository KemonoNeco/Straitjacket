use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Walks `root` and returns every file path whose extension matches one of `extensions`
/// (compared lowercase-insensitively, no leading `.`).
///
/// Excluded directory names listed in `excluded_dir_names` are pruned **at descent time**
/// — `WalkDir::filter_entry` skips descending into them entirely, never reading their
/// contents. This is load-bearing: post-walk filtering still descends into `target/`
/// and reads every file, which is the 4-minute snapshot bug the plan fixes.
///
/// Returns the matching paths sorted lexicographically by their full path.
pub fn walk_source_files(
    root: &Path,
    excluded_dir_names: &[&str],
    extensions: &[&str],
) -> std::io::Result<Vec<PathBuf>> {
    if !root.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("root path not found: {}", root.display()),
        ));
    }
    let exts_lower: Vec<String> = extensions.iter().map(|e| e.to_lowercase()).collect();

    let mut out: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_type().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    return !excluded_dir_names.contains(&name);
                }
            }
            true
        })
        .filter_map(|res| res.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| exts_lower.contains(&e.to_lowercase()))
                .unwrap_or(false)
        })
        .map(|entry| entry.into_path())
        .collect();

    out.sort();
    Ok(out)
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
    fn filters_by_extension() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        touch(&root.join("a.rs"));
        touch(&root.join("b.cs"));
        touch(&root.join("c.txt"));
        let result = walk_source_files(root, &[], &["rs"]).unwrap();
        assert_eq!(result.len(), 1, "only .rs files should match; got {:?}", result);
        assert!(result[0].file_name().and_then(|n| n.to_str()) == Some("a.rs"));
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        touch(&root.join("lower.rs"));
        touch(&root.join("upper.RS"));
        let result = walk_source_files(root, &[], &["rs"]).unwrap();
        assert_eq!(result.len(), 2, "both .rs and .RS should match: {:?}", result);
    }

    #[test]
    fn returns_paths_sorted_lexicographically() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        touch(&root.join("zeta.rs"));
        touch(&root.join("alpha.rs"));
        touch(&root.join("middle.rs"));
        let result = walk_source_files(root, &[], &["rs"]).unwrap();
        assert!(
            result.windows(2).all(|w| w[0] <= w[1]),
            "paths must be sorted: {:?}",
            result
        );
    }

    #[test]
    fn prunes_excluded_dirs_at_descent() {
        // Behavioral guarantee: files in excluded subtrees are not in the result.
        // (Adversarial note: this test passes equally for post-walk filter and
        // descent-time prune. The performance distinction can only be observed
        // via timing or via a side-channel like an invalid symlink. See
        // benchmarks_or_perf_tests if a stronger distinguisher is needed.)
        let td = TempDir::new().unwrap();
        let root = td.path();
        touch(&root.join("keep.rs"));
        touch(&root.join("target").join("nope.rs"));
        touch(&root.join("nested").join("target").join("nope2.rs"));
        touch(&root.join("nested").join("alsokeep.rs"));
        let result = walk_source_files(root, &["target"], &["rs"]).unwrap();
        let names: Vec<String> = result
            .iter()
            .filter_map(|p| p.strip_prefix(root).ok().map(|p| p.to_string_lossy().into_owned()))
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("keep.rs")),
            "keep.rs missing: {:?}",
            names
        );
        assert!(
            names.iter().any(|n| n.ends_with("alsokeep.rs")),
            "alsokeep.rs missing: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n.contains("nope")),
            "excluded files leaked: {:?}",
            names
        );
    }

    #[test]
    fn empty_dir_returns_empty_vec() {
        let td = TempDir::new().unwrap();
        let result = walk_source_files(td.path(), &[], &["rs"]).unwrap();
        assert!(result.is_empty(), "expected empty, got {:?}", result);
    }

    #[test]
    fn nonexistent_root_returns_err() {
        let td = TempDir::new().unwrap();
        let nonexistent = td.path().join("does_not_exist");
        let result = walk_source_files(&nonexistent, &[], &["rs"]);
        assert!(result.is_err(), "expected Err for missing root");
    }

    #[test]
    fn multiple_extensions_match() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        touch(&root.join("a.rs"));
        touch(&root.join("b.cs"));
        touch(&root.join("c.txt"));
        let result = walk_source_files(root, &[], &["rs", "cs"]).unwrap();
        assert_eq!(result.len(), 2, "rs+cs should match both: {:?}", result);
    }
}
