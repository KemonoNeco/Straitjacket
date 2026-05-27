use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Decides how to invoke cargo given the Rust manifests discovered under `repo_root`.
/// PURE: no filesystem I/O, reads no manifest contents — only path comparisons over
/// the manifest paths detect_stack already collected. All manifest paths are assumed
/// to be under `repo_root`; behavior is unspecified (and untested) otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CargoTarget {
    /// Run cargo from `working_dir`; pass `--workspace` iff `workspace`.
    Resolved { working_dir: PathBuf, workspace: bool },
    /// Multiple candidate crate dirs, no root manifest — refuse and let the caller report.
    Ambiguous { candidates: Vec<PathBuf> },
    /// No Rust manifests at all.
    NoRustTarget,
}

pub fn resolve_cargo_target(manifests: &[PathBuf], repo_root: &Path) -> CargoTarget {
    // Rule 1: no manifests at all.
    if manifests.is_empty() {
        return CargoTarget::NoRustTarget;
    }

    // Rule 2: any manifest sitting directly at repo_root is decisive — workspace.
    // Use repo_root directly so working_dir compares exactly equal (no normalization).
    let has_root = manifests
        .iter()
        .any(|m| m.parent() == Some(repo_root));
    if has_root {
        return CargoTarget::Resolved {
            working_dir: repo_root.to_path_buf(),
            workspace: true,
        };
    }

    // Candidate dirs are each manifest's parent directory (not the .toml path).
    let candidates: Vec<PathBuf> = manifests
        .iter()
        .map(|m| {
            m.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| m.clone())
        })
        .collect();

    // Rule 3: exactly one nested manifest → resolve at its crate dir, not a workspace.
    if candidates.len() == 1 {
        return CargoTarget::Resolved {
            working_dir: candidates.into_iter().next().expect("len == 1"),
            workspace: false,
        };
    }

    // Rule 4: two or more nested manifests, none at root → ambiguous.
    CargoTarget::Ambiguous { candidates }
}

/// What to actually run for the Rust stack, derived from a resolved CargoTarget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CargoInvocation {
    /// Run cargo from `cwd` with these full args.
    Run { cwd: PathBuf, args: Vec<String> },
    /// No Rust target — nothing to run.
    Skip,
    /// Multiple candidate crates with no root — the caller must surface this as a loud error.
    Ambiguous { candidates: Vec<PathBuf> },
}

/// Maps a resolved `CargoTarget` + the base cargo args (e.g. `["test", "--no-fail-fast"]`)
/// into a concrete invocation. Inserts `"--workspace"` immediately after the subcommand
/// (index 1) iff the target is a workspace. Pure: no I/O.
pub fn cargo_invocation(target: &CargoTarget, base_args: &[&str]) -> CargoInvocation {
    match target {
        CargoTarget::NoRustTarget => CargoInvocation::Skip,
        CargoTarget::Ambiguous { candidates } => CargoInvocation::Ambiguous {
            candidates: candidates.clone(),
        },
        CargoTarget::Resolved { working_dir, workspace } => {
            let mut args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
            if *workspace {
                // Insert immediately after the subcommand (index 1). On a single-element
                // base_args this clamps to a push (index 1 == len), so ["test"] becomes
                // ["test", "--workspace"].
                let idx = args.len().min(1);
                args.insert(idx, "--workspace".to_string());
            }
            CargoInvocation::Run {
                cwd: working_dir.clone(),
                args,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper: build a synthetic root path that is absolute and OS-correct without
    /// touching the filesystem. On Windows this produces e.g. `C:\fake\repo`;
    /// on Unix `/fake/repo`.
    fn synthetic_root() -> PathBuf {
        #[cfg(windows)]
        { PathBuf::from(r"C:\fake\repo") }
        #[cfg(not(windows))]
        { PathBuf::from("/fake/repo") }
    }

    // ─── Test 1 ──────────────────────────────────────────────────────────────────
    /// Root manifest at repo root → `Resolved { working_dir == repo_root, workspace == true }`.
    #[test]
    fn test_root_manifest_resolves_at_repo_root_with_workspace() {
        let root = synthetic_root();
        let manifests = vec![root.join("Cargo.toml")];
        let result = resolve_cargo_target(&manifests, &root);
        assert_eq!(
            result,
            CargoTarget::Resolved { working_dir: root.clone(), workspace: true },
            "single root Cargo.toml must resolve to repo_root with workspace=true"
        );
    }

    // ─── Test 2 ──────────────────────────────────────────────────────────────────
    /// Single nested manifest, no root → `Resolved { working_dir == crate_dir, workspace == false }`.
    #[test]
    fn test_single_nested_manifest_resolves_at_crate_dir_without_workspace() {
        let root = synthetic_root();
        let manifests = vec![root.join("cli").join("Cargo.toml")];
        let result = resolve_cargo_target(&manifests, &root);
        assert_eq!(
            result,
            CargoTarget::Resolved { working_dir: root.join("cli"), workspace: false },
            "single nested Cargo.toml must resolve to its parent dir with workspace=false"
        );
    }

    // ─── Test 3 ──────────────────────────────────────────────────────────────────
    /// Single deeply-nested manifest, no root → `Resolved { working_dir == its dir, workspace == false }`.
    #[test]
    fn test_single_deeply_nested_manifest_resolves_at_its_dir_without_workspace() {
        let root = synthetic_root();
        let manifests = vec![root.join("cli").join("sub").join("Cargo.toml")];
        let result = resolve_cargo_target(&manifests, &root);
        assert_eq!(
            result,
            CargoTarget::Resolved {
                working_dir: root.join("cli").join("sub"),
                workspace: false,
            },
            "deeply-nested Cargo.toml must resolve to its immediate parent dir with workspace=false"
        );
    }

    // ─── Test 4 ──────────────────────────────────────────────────────────────────
    /// Multiple nested manifests, no root → `Ambiguous` with candidate DIRS (not .toml paths).
    #[test]
    fn test_multiple_nested_manifests_without_root_resolves_ambiguous() {
        let root = synthetic_root();
        let manifests = vec![
            root.join("cli").join("Cargo.toml"),
            root.join("tools").join("Cargo.toml"),
        ];
        let result = resolve_cargo_target(&manifests, &root);
        match result {
            CargoTarget::Ambiguous { mut candidates } => {
                candidates.sort();
                let mut expected = vec![root.join("cli"), root.join("tools")];
                expected.sort();
                assert_eq!(
                    candidates, expected,
                    "Ambiguous candidates must be the parent DIRS of the manifests, not the .toml paths"
                );
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }

    // ─── Test 5 ──────────────────────────────────────────────────────────────────
    /// Root manifest + nested workspace members → root wins; `Resolved { repo_root, true }`.
    #[test]
    fn test_root_plus_nested_members_root_wins_with_workspace() {
        let root = synthetic_root();
        let manifests = vec![
            root.join("Cargo.toml"),
            root.join("crates").join("foo").join("Cargo.toml"),
            root.join("crates").join("bar").join("Cargo.toml"),
        ];
        let result = resolve_cargo_target(&manifests, &root);
        assert_eq!(
            result,
            CargoTarget::Resolved { working_dir: root.clone(), workspace: true },
            "root manifest presence must be decisive; nested members must NOT downgrade to Ambiguous"
        );
    }

    // ─── Test 5b ─────────────────────────────────────────────────────────────────
    /// Root manifest + EXACTLY ONE nested member → root still wins; `Resolved { repo_root, true }`.
    /// Boundary of test 5 (root+2 members) and test 1 (root+0 members).
    #[test]
    fn test_root_plus_single_nested_member_root_still_wins() {
        let root = synthetic_root();
        let manifests = vec![
            root.join("Cargo.toml"),
            root.join("crates").join("foo").join("Cargo.toml"),
        ];
        let result = resolve_cargo_target(&manifests, &root);
        assert_eq!(
            result,
            CargoTarget::Resolved { working_dir: root.clone(), workspace: true },
            "root manifest + one nested member must resolve to repo_root with workspace=true"
        );
    }

    // ─── Test 6 ──────────────────────────────────────────────────────────────────
    /// Empty manifest slice → `NoRustTarget`.
    #[test]
    fn test_empty_manifests_resolves_no_rust_target() {
        let root = synthetic_root();
        let manifests: Vec<PathBuf> = vec![];
        let result = resolve_cargo_target(&manifests, &root);
        assert_eq!(
            result,
            CargoTarget::NoRustTarget,
            "empty manifest list must resolve to NoRustTarget"
        );
    }

    // ─── Test 7 ──────────────────────────────────────────────────────────────────
    /// Resolution is order-independent for both multi-nested (Ambiguous) and root-plus-members (Resolved).
    #[test]
    fn test_resolution_is_independent_of_manifest_order() {
        let root = synthetic_root();

        // Part A: multi-nested (no root) — forward and reversed order must yield equal Ambiguous sets.
        let manifests_fwd = vec![
            root.join("cli").join("Cargo.toml"),
            root.join("tools").join("Cargo.toml"),
        ];
        let mut manifests_rev = manifests_fwd.clone();
        manifests_rev.reverse();

        let result_fwd = resolve_cargo_target(&manifests_fwd, &root);
        let result_rev = resolve_cargo_target(&manifests_rev, &root);

        let mut candidates_fwd = match result_fwd {
            CargoTarget::Ambiguous { candidates } => candidates,
            other => panic!("expected Ambiguous (fwd), got {:?}", other),
        };
        let mut candidates_rev = match result_rev {
            CargoTarget::Ambiguous { candidates } => candidates,
            other => panic!("expected Ambiguous (rev), got {:?}", other),
        };
        candidates_fwd.sort();
        candidates_rev.sort();
        assert_eq!(
            candidates_fwd, candidates_rev,
            "Ambiguous candidate sets must be equal regardless of input order"
        );

        // Part B: root-plus-members — forward and reversed must yield identical Resolved.
        let root_manifests_fwd = vec![
            root.join("Cargo.toml"),
            root.join("crates").join("foo").join("Cargo.toml"),
            root.join("crates").join("bar").join("Cargo.toml"),
        ];
        let mut root_manifests_rev = root_manifests_fwd.clone();
        root_manifests_rev.reverse();

        let resolved_fwd = resolve_cargo_target(&root_manifests_fwd, &root);
        let resolved_rev = resolve_cargo_target(&root_manifests_rev, &root);
        assert_eq!(
            resolved_fwd,
            CargoTarget::Resolved { working_dir: root.clone(), workspace: true },
            "root-plus-members (fwd) must resolve at repo_root with workspace=true"
        );
        assert_eq!(
            resolved_fwd, resolved_rev,
            "root-plus-members result must be identical regardless of input order"
        );
    }

    // ─── Test 8 ──────────────────────────────────────────────────────────────────
    /// Root manifest → `working_dir` equals `repo_root` EXACTLY (no trailing `.`, no suffix, no normalization).
    #[test]
    fn test_root_manifest_working_dir_equals_repo_root_exactly() {
        let root = synthetic_root();
        let manifests = vec![root.join("Cargo.toml")];
        let result = resolve_cargo_target(&manifests, &root);
        match result {
            CargoTarget::Resolved { working_dir, workspace: _ } => {
                assert_eq!(
                    working_dir, root,
                    "working_dir must equal repo_root exactly — no trailing '.', no 'Cargo.toml' suffix, no normalization artifact"
                );
                // Extra guard: the last component must NOT be "Cargo.toml" or "."
                let last = working_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                assert_ne!(last, "Cargo.toml", "working_dir must not end in Cargo.toml");
                assert_ne!(last, ".", "working_dir must not end in '.'");
            }
            other => panic!("expected Resolved, got {:?}", other),
        }
    }

    // ─── Test 9 ──────────────────────────────────────────────────────────────────
    /// Schema-shape guard: CargoTarget serializes with the stable tagged-union JSON shape.
    /// This test constructs values DIRECTLY (no call to `resolve_cargo_target`) so it
    /// exercises the serde derive independently of the fn under test — it passes from the
    /// stub by design (mirrors `stack_serializes_as_lowercase_string`).
    #[test]
    fn test_cargo_target_serializes_with_stable_tagged_shape() {
        use serde_json::Value;

        // ── Resolved { workspace: true } ──
        let resolved = CargoTarget::Resolved {
            working_dir: PathBuf::from("/some/project"),
            workspace: true,
        };
        let v: Value = serde_json::to_value(&resolved)
            .expect("Resolved must serialize without error");
        assert_eq!(
            v["kind"].as_str(),
            Some("resolved"),
            "Resolved must serialize with kind == \"resolved\"; got: {v}"
        );
        assert!(
            v.get("working_dir").is_some(),
            "Resolved must have a working_dir field; got: {v}"
        );
        assert_eq!(
            v["workspace"].as_bool(),
            Some(true),
            "Resolved must have workspace == true; got: {v}"
        );

        // ── NoRustTarget ──
        let no_rust = CargoTarget::NoRustTarget;
        let v2: Value = serde_json::to_value(&no_rust)
            .expect("NoRustTarget must serialize without error");
        assert_eq!(
            v2["kind"].as_str(),
            Some("no_rust_target"),
            "NoRustTarget must serialize with kind == \"no_rust_target\"; got: {v2}"
        );
        // NoRustTarget carries no other fields
        let obj = v2.as_object().expect("serialized value must be a JSON object");
        assert_eq!(
            obj.len(),
            1,
            "NoRustTarget must serialize to exactly one field (kind); got: {v2}"
        );

        // ── Ambiguous ──
        let ambiguous = CargoTarget::Ambiguous {
            candidates: vec![PathBuf::from("/a/one"), PathBuf::from("/b/two")],
        };
        let v3: Value = serde_json::to_value(&ambiguous)
            .expect("Ambiguous must serialize without error");
        assert_eq!(
            v3["kind"].as_str(),
            Some("ambiguous"),
            "Ambiguous must serialize with kind == \"ambiguous\"; got: {v3}"
        );
        assert_eq!(
            v3["candidates"].as_array().map(|a| a.len()),
            Some(2),
            "Ambiguous must serialize candidates as a JSON array of length 2; got: {v3}"
        );
    }

    // ─── cargo_invocation tests ───────────────────────────────────────────────────

    // ─── Test CI-1 ───────────────────────────────────────────────────────────────
    /// Workspace target → `--workspace` inserted at index 1 (immediately after subcommand).
    #[test]
    fn test_cargo_invocation_workspace_inserts_flag_after_subcommand() {
        let root = synthetic_root();
        let target = CargoTarget::Resolved { working_dir: root.clone(), workspace: true };
        let result = cargo_invocation(&target, &["test", "--no-fail-fast"]);
        let expected_args: Vec<String> = vec![
            "test".to_string(),
            "--workspace".to_string(),
            "--no-fail-fast".to_string(),
        ];
        assert_eq!(
            result,
            CargoInvocation::Run { cwd: root, args: expected_args },
            "--workspace must be inserted at index 1 (after the subcommand)"
        );
    }

    // ─── Test CI-2 ───────────────────────────────────────────────────────────────
    /// Non-workspace target → no `--workspace` flag inserted.
    #[test]
    fn test_cargo_invocation_non_workspace_omits_flag() {
        let root = synthetic_root();
        let dir = root.join("cli");
        let target = CargoTarget::Resolved { working_dir: dir.clone(), workspace: false };
        let result = cargo_invocation(&target, &["test", "--no-fail-fast"]);
        let expected_args: Vec<String> = vec![
            "test".to_string(),
            "--no-fail-fast".to_string(),
        ];
        assert_eq!(
            result,
            CargoInvocation::Run { cwd: dir, args: expected_args },
            "non-workspace target must not insert --workspace into the args"
        );
    }

    // ─── Test CI-3 ───────────────────────────────────────────────────────────────
    /// Workspace target with `--` separator: `--workspace` is still inserted at index 1,
    /// before `--all-targets` and the `--` separator.
    #[test]
    fn test_cargo_invocation_inserts_workspace_before_double_dash_separator() {
        let root = synthetic_root();
        let dir = root.join("lib");
        let target = CargoTarget::Resolved { working_dir: dir.clone(), workspace: true };
        let result = cargo_invocation(
            &target,
            &["clippy", "--all-targets", "--", "-D", "warnings"],
        );
        let expected_args: Vec<String> = vec![
            "clippy".to_string(),
            "--workspace".to_string(),
            "--all-targets".to_string(),
            "--".to_string(),
            "-D".to_string(),
            "warnings".to_string(),
        ];
        assert_eq!(
            result,
            CargoInvocation::Run { cwd: dir, args: expected_args },
            "--workspace must be at index 1, before --all-targets and the -- separator"
        );
    }

    // ─── Test CI-4 ───────────────────────────────────────────────────────────────
    /// `NoRustTarget` → `Skip`, regardless of base_args.
    #[test]
    fn test_cargo_invocation_no_rust_target_is_skip() {
        let result = cargo_invocation(&CargoTarget::NoRustTarget, &["test", "--no-fail-fast"]);
        assert_eq!(
            result,
            CargoInvocation::Skip,
            "NoRustTarget must produce Skip"
        );
    }

    // ─── Test CI-5 ───────────────────────────────────────────────────────────────
    /// `CargoTarget::Ambiguous` passes candidates through as `CargoInvocation::Ambiguous`
    /// with the same dirs in the same order.
    #[test]
    fn test_cargo_invocation_ambiguous_passes_through_candidates() {
        let root = synthetic_root();
        let candidates = vec![root.join("cli"), root.join("tools")];
        let target = CargoTarget::Ambiguous { candidates: candidates.clone() };
        let result = cargo_invocation(&target, &["test"]);
        assert_eq!(
            result,
            CargoInvocation::Ambiguous { candidates },
            "Ambiguous must pass candidates through unchanged (same dirs, same order)"
        );
    }

    // ─── Test CI-6 ───────────────────────────────────────────────────────────────
    /// The `cwd` in the returned `Run` equals exactly the `working_dir` supplied in `Resolved`.
    #[test]
    fn test_cargo_invocation_run_preserves_cwd_exactly() {
        let root = synthetic_root();
        let distinct_dir = root.join("some_crate");
        let target = CargoTarget::Resolved {
            working_dir: distinct_dir.clone(),
            workspace: false,
        };
        let result = cargo_invocation(&target, &["test"]);
        match result {
            CargoInvocation::Run { cwd, .. } => {
                assert_eq!(
                    cwd, distinct_dir,
                    "Run cwd must equal the working_dir supplied in Resolved exactly"
                );
            }
            other => panic!("expected Run, got {:?}", other),
        }
    }

    // ─── Test CI-7 ───────────────────────────────────────────────────────────────
    /// `Ambiguous` passthrough preserves the ORIGINAL order — NOT re-sorted.
    /// Closes a coincidental-pass gap: CI-5 uses `[cli, tools]` (alphabetical).
    /// Here the input is `[tools, cli]` (reverse-alpha); a sort-on-passthrough impl
    /// would produce `[cli, tools]` and fail this assertion.
    #[test]
    fn test_cargo_invocation_ambiguous_preserves_nonalphabetical_order() {
        let root = synthetic_root();
        let candidates: Vec<PathBuf> = vec![root.join("tools"), root.join("cli")];
        let target = CargoTarget::Ambiguous { candidates: candidates.clone() };
        let result = cargo_invocation(&target, &["test"]);
        assert_eq!(
            result,
            CargoInvocation::Ambiguous { candidates },
            "Ambiguous must preserve candidate order exactly — must NOT sort [tools, cli] to [cli, tools]"
        );
    }

    // ─── Test CI-8 ───────────────────────────────────────────────────────────────
    /// Boundary: workspace Resolved with a single base arg (`["test"]`).
    /// Insert-at-index-1 degenerates to append — the result must be `["test", "--workspace"]`.
    #[test]
    fn test_cargo_invocation_workspace_single_element_base_args() {
        let root = synthetic_root();
        let target = CargoTarget::Resolved { working_dir: root.clone(), workspace: true };
        let result = cargo_invocation(&target, &["test"]);
        let expected_args: Vec<String> = vec![
            "test".to_string(),
            "--workspace".to_string(),
        ];
        assert_eq!(
            result,
            CargoInvocation::Run { cwd: root, args: expected_args },
            "workspace + single base arg: --workspace must follow the subcommand (index 1 == append)"
        );
    }
}
