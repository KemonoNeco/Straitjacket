---
name: triage
description: "Close the loop on a captured bug: ingest it first if it is unrecorded locally (import a GitHub issue that has no ledger record), debug-verify it (root-cause from green — an imported/named bug is a CLAIM, not a local reproduction), then tdd FIX MODE (a failing test for the CORRECT behavior, then a fix to the real source) → flip the record to fixed on a green test that covers the bug. Use when the user says 'triage the bug backlog', 'triage issues on github', 'fix this bug end to end', 'turn this bug into a test and fix it', or 'work through the open bugs'. This drives a captured/imported bug to a green fix; use atlassian:triage-issue instead when the goal is interactive Jira duplicate-hunting and triage rather than driving a fix. Supports Rust (cargo + clippy) and C# (dotnet)."
---

# triage

## Cardinal Rule 0 — YOU ARE THE ROUTER + THE SINGLE LEDGER WRITER

You — the main session — **route a captured bug through debug and fix sub-flows and own every
write to the bug ledger** (`<repo>/.straitjacket/bugs.json`). The diagnosis and fix happen in the
sub-flows; you sequence them and record dispositions.

You mostly operate on *existing* records. There is **exactly one record you may create**: the
**reverse-mirror import of an already-tracked _remote_ issue** that has no local record yet (a
GitHub issue you were pointed at). That is not "inventing a bug" — the issue already exists and is
tracked; you are pulling it into the local ledger so the pipeline can run on it. A genuinely
*new, inline* bug (one you or the user just noticed, tracked nowhere) is still captured by
`straitjacket:report-bug`, not here — `report-bug` owns inline capture + remote creation; triage
must never create a *new* remote issue (that would duplicate). See **Preflight → ingest** below.

- **You never write test or implementation code yourself** — that's the multi-agent collapse this
  engine exists to prevent. The `coverage-reviewer`, the author teams, and the `implementation-author`
  do it. (Cardinal Rule 0 of the shared engine.)
- **Never weaken a test to clear a bug**, and never flip a record to `fixed` without a green test
  that actually covers the bug. The correctness gate is the whole point of triage.

The shared engine — the agent roster, the `fanout` / `adversarial` stages, the `tdd-cycle`
workflow, the dispatch convention, and the run-state layout — lives once in
**[`docs/STAGES.md`](../../docs/STAGES.md)**. This skill does not restate it.

> **Not `atlassian:triage-issue`.** That does interactive Jira *duplicate-hunting* against a remote
> tracker. **This** drives a tracked bug — already in the local ledger, or imported from its remote
> issue — to a verified diagnosis and, where the loop allows, a green fix.

## Args

- `--id <bug-id>` — triage one record.
- `--scope <path>` — triage all `open`/`mirrored` records whose `suspect_files` intersect `<path>`.
- `--github <n|url>[,…]` — triage GitHub issue(s). Each is matched against the ledger by
  `remote.github.number`; **any with no local record is reverse-mirror imported first** (Preflight
  → ingest). `--github all-open` selects every open issue labeled `bug` in the resolved repo.
- *(none)* — the **oldest open** record in the ledger.
- `--max N` — cap how many records to process this run (default 1; raise to drain a backlog).

## Preflight (this session)

1. Confirm a git repo (else abort); resolve `repo_root`.
2. Read `<repo>/.straitjacket/bugs.json` (create an empty `{ "bugs": [] }` if absent). Select the
   target `open`/`mirrored` record(s) per the args, capped at `--max`.
3. **Ingest any unrecorded target (reverse-mirror import).** For each `--github` issue (or each
   open `bug`-labeled issue under `all-open`) with **no** matching ledger record (matched on
   `remote.github.number`), pull it into the ledger — **you are the single writer**:
   - Fetch the issue: `gh issue view <n> --json number,title,body,labels,url` (github MCP fallback
     if `gh` is absent). Resolve the repo with `gh repo view --json nameWithOwner` — note this may
     differ from `git remote` if the repo was renamed (`gh` returns the canonical name).
   - **Parse the body — it is (almost always) the inverse of the `report-bug` remote template**
     (`schemas/bug-record.schema.json` / `report-bug`'s "Remote body template"): `## Summary`→`summary`,
     `## Expected`→`expected`, `## Actual`→`actual`, `## Steps to Reproduce`→`steps_to_reproduce`,
     `## Suspect location`→`suspect_files` + `suspect_symbol`, `**Severity:**`→`severity`, and the
     `## Fix direction` / `## Expected` text → `intended_behavior_seed` (a contract sentence for the
     *correct* behavior). **If the footer carries `Local id:` `bug-YYYY-MM-DD-NN`, REUSE it verbatim**
     (re-minting an id drifts the record from the issue body); else mint `bug-<today>-NN` per the
     report-bug id rule.
   - Write the record with `status: "mirrored"`, `remote.github = { repo, number, url }`,
     `discovered_during: "imported from GitHub #<n> by straitjacket:triage"`, and `labels` from the
     issue. Validate against `schemas/bug-record.schema.json`. **Do NOT call `report-bug`** for these
     (it would create a *duplicate* issue) and **do NOT `gh issue create`** — the issue already exists.
   - Selecting an issue that DOES already have a ledger record → just use that record (no import).
   - No target selected and nothing to import → report and stop.
4. **No green-baseline gate here** — triage is a router; the **debug** and **fix-mode** sub-flows
   each carry their own green preflight (`verify-tree-clean`), so the gate fires where the work
   happens, not at the router. (triage is deliberately excluded from the `UserPromptExpansion`
   preflight matcher.)

## Per record — ROUTE on verification, then on completeness

A record is only ready for fix mode once it has been **verified _in this tree_**. Provenance
decides that, not how full the fields look:

- **Imported from a remote issue this run, or any record missing `suspect_files` /
  `suspect_symbol` / `intended_behavior_seed` or with no prior local reproduction** → it is an
  **unverified claim**. A remote body's "Suspect location"/"Expected" are someone else's diagnosis;
  they may be stale, wrong, or un-reproducible here. **Run the DEBUG flow first** (below) regardless
  of how complete the fields look. This is the rule the `--github` import path always hits.
- **Already locally reproduced by a prior debug pass** (its `notes` carry a `root_cause` +
  `reproduction` from a `root-cause-analyst`) and complete → skip straight to **Fix mode**.

### DEBUG flow (verify / understand) — single direct `Agent` dispatch

Dispatch one `root-cause-analyst` from green (the same single direct `Agent` dispatch `skills/debug`
uses — reproduce, instrument-and-revert, leave the tree exactly green). Self-contained prompt: the
record's fields as *context not conclusions*, the `stack`, `repo_root`, and any `suspect_files` as a
scope hint.

**Tell the analyst what kind of code it is looking at.** If the target is **hand-authored
orchestration** — `workflows/*.js`, `skills/**`, `agents/*.md`, `hooks*.json`, docs — it is **not
standalone-runnable and the analyst cannot drive it** (it has no `Workflow`/`Agent` tool), so
reproduction-by-execution is unavailable: instruct it to **verify by code-trace** (read the cited
symbols/lines, confirm the defect statically, cite the issue's evidence lines), to set
`reproduced: false` honestly rather than fabricate a repro, and that `reproduced: false` with a
high-confidence code-trace is an **acceptable, expected outcome here** — not a failure. For
**testable code** (the Rust/C# crate), the usual "reproduce first" rule stands.

Write the analyst's `root_cause` + `reproduction` + the three bridge fields back into the record
(you are the single writer). Then **route on the result**:

| Debug result | Target | → Route |
|---|---|---|
| `reproduced: true` | testable code (Rust/C# crate) | **Fix mode** (below) |
| `reproduced: false` (verified by code-trace) | any | **Verified-by-analysis** (terminal) |
| reproduced or not | hand-authored orchestration (no test harness) | **Verified-by-analysis** (terminal) |

**Do NOT auto-fall-through into Fix mode after debug.** Fix mode is the tdd loop; it only applies to
a *reproduced defect in testable code*. A code-trace-only confirmation, or any orchestration target,
cannot be driven through it — see **Verified-by-analysis** below.

### Verified-by-analysis (terminal — fix is out-of-loop)

The defect is confirmed but cannot be driven to a green test through the tdd loop — either it was
verified by code-trace only (`reproduced: false`), or it lives in **hand-authored orchestration**
(`workflows/*.js`, `skills/**`, `agents/*.md`, `hooks*.json`), which **has no unit-test harness** and
is **live-run-guarded, not test-backed** (see [`CLAUDE.md` / `docs/STAGES.md`]). Here triage's honest
ceiling is *verification*, not a tested fix:

- Append the analyst's `root_cause` + code-trace evidence to the record's `notes`; refresh the
  bridge fields. **Leave `status` as `mirrored`/`open` — never flip to `fixed`** (there is no green
  test covering it). Do not enter Fix mode.
- Surface in the summary as **`verified-by-analysis`**, and state plainly that the fix is a
  **hand-authored / live-run-guarded** change outside the tdd loop — *that hand-authored fix is its
  own task, not part of this triage run* unless the user asks for it.

### Reproduced & complete in testable code — FIX MODE (the correctness pivot)

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

- **Bug id** + title, and its **disposition**: `imported` (newly reverse-mirrored from a remote
  issue this run) / `debugged → fixed` / `fixed` / `verified-by-analysis (fix out-of-loop)` /
  `wontfix` / `duplicate` / `escalated (could not fix)`.
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
