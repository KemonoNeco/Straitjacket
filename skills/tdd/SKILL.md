---
name: tdd
description: "Drive new feature development test-first: coverage planning from a spec → parallel test+stub authoring (tests compile-but-fail) → red-check → adversarial pre-validation on the RED tests → implementation → adversarial + mutation passing-reason validation, iterating to a cap, under a savepoint red/green discipline. Runs as one resumable `tdd-cycle` dynamic-Workflow (gates branch in-script on runner verdicts; no interactive contract-review — contracts are surfaced non-blocking instead), falling back to staged Agent dispatch when the Workflow tool is absent. Use when the user wants to implement a new feature TDD-style, write failing tests first, drive a new module from a specification, or do test-driven development on a Rust or C# project. Supports Rust (cargo + clippy + cargo-mutants) and C# (dotnet + Stryker.NET)."
---

# tdd

## Cardinal Rule 0 — YOU ARE THE LAUNCHER, NOT THE AUTHOR

**You — the main session — own the savepoint and the final commit; the cycle runs in the
`tdd-cycle` workflow.** You NEVER write test or implementation code yourself (the multi-agent
collapse failure mode this skill exists to prevent). The only files you write directly are
`tooling.json`, `test-snapshot.json`, `state.json`, `.gitignore`, scaffolded C# test projects,
and the final summary. Everything else comes from the workflow's specialist agents.

The shared engine — the specialist agent roster, the dispatch convention, the `fanout` /
`adversarial` stages, the cardinal rules, and the run-state layout — lives once in
**[`docs/STAGES.md`](../../docs/STAGES.md)**. This skill does not restate it.

## What changed from the staged design

The interactive **contract-review gate is gone** — it was the only human-input stop, so the
whole cycle (coverage → author → red → adversarial → impl → green → mutation → iterate) now runs
as **one resumable `tdd-cycle` workflow** with the gates as in-script branches on `gate-runner`
verdicts. The locked `intended_behavior` contracts are **surfaced non-blocking** in the final
summary (audit-after, not pre-empt). Your residual role is thin: start from a known-green tree,
**commit the savepoint on QA'd green**, handle surfaced bugs, present the summary.

## Args

- `<spec-text>` — inline specification (required).
- `--quick` — skip the post-green mutation team.
- `--max-rounds N` — iteration cap (default 3).
- `--no-commit` — run the cycle and report, but do NOT commit (you'll commit by hand).

## Savepoint red/green discipline

- **RED** (tests fail): never weaken a test; if a test can't pass honestly or is misaligned, the
  cycle surfaces it (`surfaced_bug`) rather than weakening it.
- **GREEN** (tests pass): a savepoint is a **commit, made ONLY on QA'd green**. Never commit the
  `unimplemented!()` skeleton. Start the run from a known-green commit.

## Preflight (this session)

1. Confirm the working dir is a git repo (else abort); resolve `repo_root`. The tree should be
   **green** (the `UserPromptExpansion` preflight gate fires for `tdd`).
2. Generate `run_id` = `<YYYYMMDDThhmmss>-<4hex>`; create `<repo_root>/.claude-regression/<run_id>/`;
   append `.claude-regression/` to `.gitignore` if absent.
3. `straitjacket detect-stack --repo-root <repo_root>` → `stack` (+ `cargo_target`).
4. `straitjacket snapshot-tests --repo-root <repo_root> --out-file <run_id>/test-snapshot.json`.
5. Probe tooling (cargo-mutants / dotnet-stryker) → `<run_id>/tooling.json`.
6. Scaffold C# `*.Tests` projects if needed.

## Run the cycle

**Capability check:** inspect your own tools for one named `Workflow`.

- **Present →** `straitjacket workflow-script tdd-cycle` (Bash) emits the script; capture it
  verbatim and call `Workflow({script: <captured>, args})` with:
  - `spec`, `stack`, `repoRoot`, `outputDir` (`<repo_root>/.claude-regression/<run_id>`),
    `workUnitsPath` (`outputDir + "/work-units.json"`), `testSnapshotPath`,
    `toolingAvailable` (from `tooling.json`), `maxRounds`, `quick`.
  - **Never** put a diff or author transcript in `args` — agents Read the spec + source themselves.
  The workflow runs the full cycle and returns a compact structured result (below).
- **Absent →** staged fallback per [`docs/STAGES.md`](../../docs/STAGES.md#dispatch-convention):
  dispatch `coverage-reviewer` (spec mode) directly, then the `fanout` and `adversarial` stages
  by hand, running the `run-new-tests` / `verify-new-tests-compile` gates in this session between
  stages and branching on the same verdicts the workflow uses.

## Handle the result (this session)

The `tdd-cycle` result is `{ rounds_run, locked_contracts, surfaced_bugs, surviving_mutants,
no_mutation_audit, ready_to_commit, error }`.

1. **`error` set** (e.g. a `nothing_to_run` gate, name-survival break) → do NOT commit; surface the
   error verbatim and stop. A zero-check is a failure, not a pass.
2. **`surfaced_bugs` non-empty** → ESCALATE each in the summary. For any you will not fix in this
   run, invoke `straitjacket:report-bug` (map `target_file`/`target_symbol`/`intended_behavior_seed`
   → the bug's `suspect_files`/`suspect_symbol`/`intended_behavior_seed`). Never weaken a test to
   clear one.
3. **`ready_to_commit` true and not `--no-commit`** → run the new tests once more to confirm green,
   then **commit the savepoint** (QA'd green). This is the only commit point.
4. **`surviving_mutants`** → already fed back into the cycle's own iteration up to `--max-rounds`;
   report any that remain as known coverage gaps.
5. **Any part of the work not TDD-verifiable** (a piece that landed in non-unit-tested orchestration
   — `workflows/*.js`, `skills/*/SKILL.md`, `agents/*.md`, `hooks.json` — or otherwise has no test
   seam) → before declaring done, **verify it via `straitjacket:audit`** scoped to those file(s)
   ([STAGES.md](../../docs/STAGES.md) rule 8) and state the basis (*audit-checked + live-run-guarded*,
   not test-backed). Do not let an untestable slice ride out on "live-run-guarded" alone.

## Final summary (present verbatim)

- **Run metadata**: run_id, stack, rounds_run.
- **Locked contracts** (the non-blocking contract surfacing): each `intended_behavior` with its
  `target_file`/`target_symbol` — audit these; the tests faithfully enforce them.
- **Implementation written**: files + symbols the `implementation-author` touched.
- **🚨 Surfaced bugs** (if any): ESCALATE.
- **Surviving mutants** (if any).
- **Degraded steps** (tooling-absent paths).
- **Known-limitation note** (always): "`intended_behavior` was inferred by an LLM and is no longer
  gated by a human contract-review; tests faithfully enforce it — if the inference was wrong, the
  tests enforce the wrong contract. The surfaced contracts above are your audit hook."

## Error handling

- Workflow error → fall back to the staged Agent path for the remainder (the two paths are equivalent).
- CLI subcommand failure → log + surface in the summary.
- User interruption → best-effort `state.json` checkpoint.

## Notes

- The cycle is mostly Opus turns (coverage, authors, the adversarial stack, implementation) plus
  Haiku runners (mutation, gate-runner); it iterates up to `--max-rounds`.
- All run artifacts live under `<repo>/.claude-regression/<run_id>/`; the bug ledger at
  `<repo>/.straitjacket/bugs.json` is tracked/committed. The `straitjacket` CLI is on `PATH`
  via the plugin's `bin/`.
