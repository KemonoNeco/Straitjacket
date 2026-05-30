---
name: triage
description: "Close the loop on a captured local bug: route a ledger record → debug it (root-cause from green if not yet reproduced) → tdd FIX MODE (a failing test for the CORRECT behavior, then a fix to the real source) → flip the record to fixed on a green test that covers the bug. Use when the user says 'triage the bug backlog', 'fix this bug end to end', 'turn this bug into a test and fix it', or 'work through the open bugs'. This drives a captured local bug to a green fix; use atlassian:triage-issue instead when the goal is interactive Jira duplicate-hunting and triage rather than driving a fix. Supports Rust (cargo + clippy) and C# (dotnet)."
---

# triage

## Cardinal Rule 0 — YOU ARE THE ROUTER + THE SINGLE LEDGER WRITER

You — the main session — **route a captured bug through debug and fix sub-flows and own every
write to the bug ledger** (`<repo>/.straitjacket/bugs.json`). You operate on *existing* records;
you do not create them (a newly-found inline bug is captured by `straitjacket:report-bug`, not
here). The diagnosis and fix happen in the sub-flows; you sequence them and record dispositions.

- **You never write test or implementation code yourself** — that's the multi-agent collapse this
  engine exists to prevent. The `coverage-reviewer`, the author teams, and the `implementation-author`
  do it. (Cardinal Rule 0 of the shared engine.)
- **Never weaken a test to clear a bug**, and never flip a record to `fixed` without a green test
  that actually covers the bug. The correctness gate is the whole point of triage.

The shared engine — the agent roster, the `fanout` / `adversarial` stages, the `tdd-cycle`
workflow, the dispatch convention, and the run-state layout — lives once in
**[`docs/STAGES.md`](../../docs/STAGES.md)**. This skill does not restate it.

> **Not `atlassian:triage-issue`.** That does interactive Jira *duplicate-hunting* against a remote
> tracker. **This** drives a record that already exists in the local ledger to a green fix.

## Args

- `--id <bug-id>` — triage one record.
- `--scope <path>` — triage all `open`/`mirrored` records whose `suspect_files` intersect `<path>`.
- *(none)* — the **oldest open** record in the ledger.
- `--max N` — cap how many records to process this run (default 1; raise to drain a backlog).

## Preflight (this session)

1. Confirm a git repo (else abort); resolve `repo_root`.
2. Read `<repo>/.straitjacket/bugs.json`; select the target `open`/`mirrored` record(s) per the
   args, capped at `--max`. No record selected → report and stop.
3. **No green-baseline gate here** — triage is a router; the **debug** and **fix-mode** sub-flows
   each carry their own green preflight (`verify-tree-clean`), so the gate fires where the work
   happens, not at the router. (triage is deliberately excluded from the `UserPromptExpansion`
   preflight matcher.)

## Per record — ROUTE on completeness

For each selected record, branch on whether it is reproduced and the bridge fields are filled:

### Incomplete / unreproduced

Missing any of `suspect_files` / `suspect_symbol` / `intended_behavior_seed`, or no reproduction →
**run the DEBUG flow first**: dispatch one `root-cause-analyst` from green (the same single direct
`Agent` dispatch `skills/debug` uses — reproduce, instrument-and-revert, leave the tree exactly
green). Write its `root_cause` + the three bridge fields back into the record (you are the single
writer). Then fall through to **Fix mode** with the now-complete record.

### Reproduced & complete — FIX MODE (the correctness pivot)

Run the test-first cycle **seeded from the ledger** rather than from a spec. The choreography is
the engine's — reuse the **`tdd-cycle`** workflow (capability-check for `Workflow`, else staged
`fanout` + `adversarial` Agent dispatch per the
[dispatch convention](../../docs/STAGES.md#dispatch-convention---workflow-first-with-agent-fallback));
**do not restate it here.** Two seams are unique to fix mode and must hold:

1. **Seed `coverage-reviewer` in TARGET mode with the record's `intended_behavior_seed` as the
   AUTHORITATIVE, verbatim locked contract.** Pass `suspect_files`→`target_file`,
   `suspect_symbol`→`target_symbol`, and the seed as the `intended_behavior` it must NOT re-infer.
   The reviewer's fix-mode clause means it writes a test for the **correct** behavior — it must
   **not** characterize current (buggy) behavior, or the test would lock the bug instead of the fix
   (see [`docs/STAGES.md` → target mode / fix mode](../../docs/STAGES.md#target-mode--the-report-bug-ledger-seed-the-fix-mode-seam)).

2. **`bug-status --set fixed` is GATED on a green test that covers the bug.** Only after the new
   test goes RED against the buggy code and GREEN after the fix do you flip the record.

Everything between those seams is *the same as a `tdd` run, seeded from the ledger*: authors write
the failing test → `straitjacket run-new-tests --expect fail` (it must FAIL against the buggy
source; branch loudly on `nothing_to_run` — a zero-check is a failure, not a pass) → adversarial
`pre_impl` on the RED test → `implementation-author` **fixes the real source, never the test** →
`run-new-tests --expect pass` + name-survival + `verify-no-test-mutation` → adversarial `post_green`
(+ optional mutation). On QA'd green:

- `straitjacket bug-status --repo-root <repo_root> --id <id> --set fixed [--note <test names>]`
  — only with a green test that references/covers the bug.
- **Commit the savepoint** (QA'd green only — never the `unimplemented!()`/red state). **If the record
  was `mirrored` with a `remote.github`, the commit message MUST carry a GitHub closing keyword —
  `Closes <remote.github.url>`** (URL form so it resolves cross-repo; `#<remote.github.number>` works same-repo).
  GitHub then auto-closes the linked issue **when the PR merges to the default branch** — that, not a
  fix-time `gh issue close`, is how triage closes the issue: triage's lifecycle ends at the commit and
  it must not close an issue whose fix could still be rejected in review. Gate it strictly: only on
  `fixed`, only when `remote.github` exists — a local-only `open` record has no issue to close, and
  `wontfix`/`duplicate` are out of scope.

### Won't-fix / duplicate

Not a real defect or already tracked → `straitjacket bug-status --repo-root <repo_root> --id <id>
--set wontfix|duplicate --note "<why>"`. No code change, no test, no fix-mode run.

## Handle the result (this session)

- **Fix-mode `error`** (a `nothing_to_run` gate, a name-survival break, the test won't go red, the
  fix can't pass without weakening the test) → do NOT flip the record to `fixed`; leave it `open`,
  surface the error verbatim, and ESCALATE in the summary. A bug that can't be fixed honestly stays open.
- **Surfaced bugs** (a fix-mode run uncovers a *different* defect) → capture via
  `straitjacket:report-bug` as a new record; never fold it into the one you're triaging.
- **You remain the single ledger writer** for the whole run — all status transitions and
  bridge-field writebacks go through this session (`bug-status` / direct edits), never a sub-agent.

## Final summary (present verbatim)

Per record:

- **Bug id** + title, and its **disposition**: `debugged → fixed` / `fixed` / `wontfix` /
  `duplicate` / `escalated (could not fix)`.
- **Test(s) added**: the new test name(s) that lock the correct behavior.
- **Fix**: files + symbols the `implementation-author` touched.
- **New ledger status** (and the `bug-status` set you applied).
- **Issue close-out** — for each record flipped to `fixed` that had a remote:
  - **GitHub**: state that the savepoint commit carries `Closes <remote.github.url>` and the issue
    auto-closes on PR merge. **Surface `Closes <remote.github.url>` (or the repo-qualified
    `Closes <remote.github.repo>#<remote.github.number>`) for the PR body too** — triage owns the
    commit but not the PR, and a squash/rebase merge can drop a commit-message keyword, so the PR
    body is the strategy-independent close. Use the URL or repo-qualified form, not a bare
    `#<remote.github.number>`, so it still closes when the PR is in a different repo than the issue.
    (triage must not create the PR itself.)
  - **Jira**: a GitHub merge does **not** close a Jira ticket unless a DVCS/smart-commit integration
    is wired — do not imply it does. Surface `remote.jira.key` so the owner can transition it (and,
    like GitHub, only post-merge — never a fix-time transition that could outrun review).
- **🚨 Escalations**: any record that could not be driven to green — surfaced loudly, left `open`.

## Notes

- Mostly the `tdd-cycle` engine's turns (coverage, authors, adversarial stack, implementation) plus
  the `root-cause-analyst` when a record needs debugging first; iterates the cycle to its cap.
- All run artifacts live under `<repo>/.claude-regression/<run_id>/`; the bug ledger at
  `<repo>/.straitjacket/bugs.json` is **tracked/committed** — it is the durable record this skill
  reads, transitions, and never gitignores. The `straitjacket` CLI is on `PATH` via the plugin's `bin/`.
