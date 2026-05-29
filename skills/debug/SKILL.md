---
name: debug
description: "Root-cause ONE bug from a green tree — understand the defect and its cause WITHOUT fixing it. Dispatches a single root-cause-analyst agent that reproduces, instruments via Bash, reverts, and leaves the tree exactly green, returning a root_cause + reproduction + the three test-bridge fields (suspect_files / suspect_symbol / intended_behavior_seed). Use when the user says 'debug this', 'why does X happen', 'find the root cause', or 'investigate this bug/crash/failure'. Diagnosis-only: it does not write a fix or a test — hand the diagnosis to straitjacket:triage (or tdd fix-mode) to turn it into a failing test + fix. Supports Rust and C#."
---

# debug

## Cardinal Rule 0 — YOU DIAGNOSE, YOU DO NOT FIX

This skill **understands a bug and its cause; it never fixes it and never authors a test.**
Fixing is `straitjacket:triage` (or `tdd` fix-mode); test-writing is the multi-agent engine.
Your residual role is thin: start from a known-green tree, dispatch the investigator, and **own
the savepoint** — the tree must end exactly as green as it started.

- The investigation is done by **one `root-cause-analyst` agent**, not by you and not by a
  fan-out team. You do not read-and-reason your way to the root cause in the main session.
- The analyst may instrument via Bash, but it **reverts** (or works in a throwaway worktree). If
  it leaves anything dirty, you restore the tree — the savepoint guarantee is mechanical, not a
  promise.

The shared engine — the agent roster (incl. `root-cause-analyst`), the dispatch convention, and
the run-state layout — lives once in **[`docs/STAGES.md`](../../docs/STAGES.md)**. This skill does
not restate it.

## Args

- `<bug>` — an inline description of the defect (the failing behavior, the symptom). OR:
- `--id <bug-id>` — pull an existing record from the bug ledger (`<repo>/.straitjacket/bugs.json`)
  and investigate it; its `suspect_files` / `suspect_symbol` seed the scope.
- `--files a,b` — repo-relative path(s) the user already suspects; narrows the analyst's scope.

At least one of `<bug>` / `--id` is required; `--files` is an optional scope hint either way.

## Preflight (this session)

1. Confirm the working dir is a git repo (else abort); resolve `repo_root`.
2. **Confirm the tree is green/clean** — `straitjacket verify-tree-clean --repo-root <repo_root>`
   → `{clean, dirty_files}`. If `clean` is false, stop and report `dirty_files`: debug operates
   **from a green state** (it is in the `UserPromptExpansion` green-baseline preflight matcher, so
   the gate also fires on invocation). A diagnosis from a dirty tree can't promise a clean savepoint.
3. Generate `run_id` = `<YYYYMMDDThhmmss>-<4hex>`; create `<repo_root>/.claude-regression/<run_id>/`;
   append `.claude-regression/` to `.gitignore` if absent.
4. `straitjacket detect-stack --repo-root <repo_root>` → `stack`.
5. If `--id` was given, read the record and carry its `suspect_files` / `suspect_symbol` /
   `summary` / `expected` / `actual` into the analyst's prompt as context (not as conclusions).

## Investigate (single direct Agent dispatch — NOT a workflow stage)

`root-cause-analyst` is a single intra-turn iterative agent (reproduce → hypothesize → instrument
→ re-run, internally), so it is dispatched **directly via `Agent`** — never a `workflow-script`
stage (same precedent as `coverage-reviewer`; see the substrate classifier in
[`docs/STAGES.md`](../../docs/STAGES.md#the-substrate-classifier---stage-vs-agent-vs-main-session)).

Dispatch **one** `root-cause-analyst`. Self-contained prompt (the agent has no memory of this
session): the bug (inline text and/or the ledger record's fields), the `--files` scope hint, the
`stack`, and `repo_root`. Instruct it to:

- **Reproduce** the failure first (the analyst owns the reproduction; an unreproduced bug is a
  hypothesis, not a diagnosis).
- **Instrument via Bash and REVERT** — add prints/asserts/extra logging as needed to localize the
  cause, then undo every edit (or do it all in a throwaway worktree). It leaves the tree exactly green.
- **Not fix anything** — no source edit survives the turn; this is diagnosis, not repair.
- Return `root_cause`, a `reproduction` (the minimal steps/command that triggers the bug), and the
  three bridge fields `suspect_files` / `suspect_symbol` / `intended_behavior_seed` (the contract
  sentence for the *correct* behavior — the seed a future fix-mode run locks).

## Restore the savepoint (this session)

After the analyst returns, re-run `straitjacket verify-tree-clean --repo-root <repo_root>`. If
`clean` is false, **restore the tree** (`git checkout -- <dirty_files>` / `git reset --hard` to
the pre-run HEAD) before reporting — the analyst is supposed to revert, but the savepoint is yours
to guarantee mechanically. Never present a diagnosis over a dirty tree.

## Handle the result (this session)

1. **`--id` record given** → write back the analyst's findings to that record (you may use the
   ledger directly here): refresh `suspect_files` / `suspect_symbol` / `intended_behavior_seed`,
   append the `root_cause` + `reproduction` to `notes`. Leave `status` as-is (debug doesn't fix).
2. **Inline bug, real defect** → offer to capture it via `straitjacket:report-bug` (map the
   analyst's `suspect_files` / `suspect_symbol` / `intended_behavior_seed` straight onto the
   BugRecord fields) so the diagnosis becomes a durable, liftable ledger record.
3. **Reproduction failed** → say so plainly. An unreproduced symptom is reported as a hypothesis
   with what was tried, not as a root cause.

## Final summary (present verbatim)

- **Run metadata**: run_id, stack, the bug investigated (id if from the ledger).
- **Reproduction**: the minimal steps/command that triggers the failure.
- **Root cause**: the analyst's `root_cause` — the actual defect and why the symptom follows.
- **Suspect location**: `suspect_files` + `suspect_symbol`.
- **Intended behavior seed**: the contract sentence for the *correct* behavior (the fix-mode seed).
- **Savepoint**: confirmation the tree is green (and whether you had to restore it).
- **Hand-off**: `straitjacket:triage` (or `tdd` fix-mode seeded with `intended_behavior_seed`)
  turns this diagnosis into a failing test for the correct behavior + a fix. **debug does not fix.**

## Notes

- One Opus turn (the `root-cause-analyst`) plus a couple of CLI calls; no fan-out, no workflow.
- All run artifacts live under `<repo>/.claude-regression/<run_id>/`; the bug ledger at
  `<repo>/.straitjacket/bugs.json` is tracked/committed. The `straitjacket` CLI is on `PATH`
  via the plugin's `bin/`.
