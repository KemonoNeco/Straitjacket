---
name: tdd
description: "Drive new feature development test-first: coverage planning from a spec → parallel test+stub authoring (tests compile-but-fail) → red-check → adversarial pre-validation on the RED tests → implementation → green-check → mutation testing, then a session-owned post-green hardening loop (audit the finished code → send behavior gaps + surviving mutants back through test-first fix mode, apply quality/refactor findings under green), under a savepoint red/green/refactor discipline. The red→green→mutation pass runs as one resumable `tdd-cycle` dynamic-Workflow (gates branch in-script on runner verdicts; no interactive contract-review — contracts are surfaced non-blocking instead), falling back to staged Agent dispatch when the Workflow tool is absent. Use when the user wants to implement a new feature TDD-style, write failing tests first, drive a new module from a specification, or do test-driven development on a Rust or C# project. Supports Rust (cargo + clippy + cargo-mutants) and C# (dotnet + Stryker.NET)."
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
red→green→mutation pass (coverage → author → red → pre-impl adversarial → impl → green → mutation)
now runs as **one resumable `tdd-cycle` workflow** with the gates as in-script branches on
`gate-runner` verdicts. The locked `intended_behavior` contracts are **surfaced non-blocking** in the
final summary (audit-after, not pre-empt).

**Post-green pivot.** The post-green phase no longer re-grades the (now-locked) tests with the
adversarial test-validity specialists — that was **redundant** with the pre-impl pass, which checks
the same frozen tests. The workflow now makes one honest **red→green→mutation** pass; *you* run the
**post-green hardening loop** (below): audit the **finished implementation**, send **behavior gaps +
surviving mutants** back through **test-first fix mode** (a RED test for the correct behavior, then a
fix), and apply **quality/refactor** findings **under green** (reverting any that break green). This
loop is yours, not the workflow's, because it owns the git savepoint — it can commit each accepted
improvement and revert a green-breaking refactor, which the workflow runtime cannot.

Your residual role: start from a known-green tree, **commit the savepoint on QA'd green**, run the
hardening loop, handle surfaced bugs, present the summary.

## Args

- `<spec-text>` — inline specification (required).
- `--quick` — skip the post-green mutation team.
- `--max-rounds N` — in-workflow cap (default 3). Now largely vestigial: since the post-green pivot
  the workflow makes a single red→green→mutation pass; post-green iteration is `--max-harden-rounds`.
- `--max-harden-rounds N` — post-green hardening-loop cap (default 2; `0` = skip the loop).
- `--no-harden` — commit the green baseline but skip the post-green audit / refactor / fix-mode loop.
- `--no-commit` — run the cycle and report, but do NOT commit (you'll commit by hand; implies no harden).

## Savepoint red/green discipline

- **RED** (tests fail): never weaken a test; if a test can't pass honestly or is misaligned, the
  cycle surfaces it (`surfaced_bug`) rather than weakening it.
- **GREEN** (tests pass): a savepoint is a **commit, made ONLY on QA'd green**. Never commit the
  `unimplemented!()` skeleton. Start the run from a known-green commit.

## Preflight (this session)

1. Confirm the working dir is a git repo (else abort); resolve `repo_root`. The tree should be
   **green** (the `UserPromptExpansion` preflight gate fires for `tdd`).
2. Generate `run_id` = `<YYYYMMDDThhmmss>-<4hex>`; create `<repo_root>/.straitjacket/<run_id>/`;
   append `.straitjacket/*/` to `.gitignore` if absent (scoped to subdirs so `.straitjacket/bugs.json` stays committable).
3. `straitjacket detect-stack --repo-root <repo_root>` → `stack` (+ `cargo_target`).
4. `straitjacket snapshot-tests --repo-root <repo_root> --out-file <run_id>/test-snapshot.json`.
5. Probe tooling (cargo-mutants / dotnet-stryker) → `<run_id>/tooling.json`.
6. Scaffold C# `*.Tests` projects if needed.

## Run the cycle

**Capability check:** inspect your own tools for one named `Workflow`.

- **Present →** `straitjacket workflow-script tdd-cycle` (Bash) emits the script; capture it
  verbatim and call `Workflow({script: <captured>, args})` with:
  - `spec`, `stack`, `repoRoot`, `outputDir` (`<repo_root>/.straitjacket/<run_id>`),
    `workUnitsPath` (`outputDir + "/work-units.json"`), `testSnapshotPath`,
    `toolingAvailable` (from `tooling.json`), `maxRounds`, `quick`.
  - **Never** put a diff or author transcript in `args` — agents Read the spec + source themselves.
  The workflow runs the full cycle and returns a compact structured result (below).
- **Absent →** staged fallback per [`docs/STAGES.md`](../../docs/STAGES.md#dispatch-convention):
  dispatch `coverage-reviewer` (spec mode) directly, then the `fanout` and the **pre-impl**
  `adversarial` pass by hand, running the `run-new-tests` / `verify-new-tests-compile` gates in this
  session between stages and branching on the same verdicts the workflow uses. **Post-green mirrors
  the workflow's pivot:** no post-green adversarial re-grade — run the mutation team over the impl
  files mechanically, then run the post-green hardening loop (step 5) yourself. The two paths are
  equivalent.

## Handle the result (this session)

The `tdd-cycle` result is `{ rounds_run, locked_contracts, surfaced_bugs, surviving_mutants,
no_mutation_audit, ready_to_commit, error }`. The workflow made ONE red→green→mutation savepoint
pass; the **post-green hardening loop (step 5) is yours** — it owns the git savepoint, so it commits
each accepted improvement and reverts a green-breaking refactor (the workflow runtime cannot).

1. **`error` set** (e.g. a `nothing_to_run` gate, name-survival break) → do NOT commit; surface the
   error verbatim and stop. A zero-check is a failure, not a pass.
2. **`surfaced_bugs` non-empty** → ESCALATE each in the summary. For any you will not fix in this
   run, invoke `straitjacket:report-bug` (map `target_file`/`target_symbol`/`intended_behavior_seed`
   → the bug's `suspect_files`/`suspect_symbol`/`intended_behavior_seed`). Never weaken a test to
   clear one.
3. **Capture gate (MANDATORY, blocking — issue #15; runs only when `surfaced_bugs` is non-empty)**
   → a hard gate, not advice: after filing (step 2), confirm none was dropped before declaring done.
   (a) Write `surfaced_bugs` (already `{work_unit_id, target_file, …}`-shaped) to
   `<repo>/.straitjacket/<run_id>/surfaced-findings.json`; (b) run
   `straitjacket verify-surfaced-bugs-captured --repo-root <repo_root> --findings-file <that file>`;
   (c) if it exits non-zero (`uncaptured` non-empty — a surfaced bug never reached the ledger), file
   it (re-runs are dedupe-safe) and return to (b). **Do NOT report the run complete or treat
   `ready_to_commit` as final until the gate exits 0.**
4. **`ready_to_commit` true and not `--no-commit`** → run the new tests once more to confirm green,
   then **commit the baseline savepoint** (QA'd green). This is the green baseline the hardening loop
   builds on — each accepted hardening change (step 5) becomes its own savepoint commit on top.
5. **Post-green hardening loop** (unless `--no-harden` / `--no-commit`) → interrogate the **finished
   implementation** and iterate, bounded by `--max-harden-rounds` (default 2). Per round:
   - **(a) Audit the impl.** Run the `audit` stage (`straitjacket workflow-script audit` → `Workflow`)
     scoped to the impl files the `implementation-author` touched (the `target_file`s in
     `locked_contracts`), with the **full 7-lens set** + the available mechanical tools. Reuse the
     `audit` skill's preflight (detect-stack, probe tools, `skeptics` default 3) and its **refute
     spine** — action only `confirmed_findings`.
   - **(b) Route each confirmed finding by class:**
     - **Behavior gap** — lenses `latent-bug` / `error-handling` / `security` / `concurrency`, or
       disposition `bug_record` / `work_unit_proposal` (asserts WRONG or MISSING behavior) — **plus
       every surviving mutant** (a survived mutant = a behavior the tests don't pin) → **go back to
       the start of TDD**: relaunch `tdd-cycle` in **fix mode** (`mode:'target'`), mapping the
       finding's bridge fields **verbatim** — `suspect_files`→`targetFile`, `suspect_symbol`→
       `targetSymbol`, `intended_behavior_seed`→`intendedBehaviorSeed` (for a mutant:
       `intendedBehaviorSeed` = "the behavior the surviving mutant at `<file:line / operator>`
       violates"; `targetFile`/`targetSymbol` = the mutant's location). That run writes a RED test for
       the **correct** behavior, then a fix, then green → **commit savepoint**. NEVER hand-patch
       (Cardinal Rule 0 / "no fix without a failing test first").
     - **Quality** — lenses `dead-code` / `performance` / `doc-drift`, disposition `report` →
       **refactor under green**: dispatch `implementation-author` with `mode:'refactor'`,
       `refactor_findings` = these findings, `target_files` = the impl files. Then **re-run the green
       gate** (`run-new-tests --expect pass`) **+ `verify-no-test-mutation`** (CLI, this session). If
       **still green AND tests unmutated** → **commit the refactor savepoint**. Otherwise
       `git checkout -- <impl files>` to revert to the last savepoint and record the **rejected
       refactor** in the summary (a refactor that can't stay green is a behavior gap, not a cleanup).
   - **(c) Stop** when a round yields no actionable `confirmed_findings`, or `--max-harden-rounds` is
     reached. Re-audit between rounds only if changes landed.
   - **(d)** Behavior gaps / surviving mutants left **unfixed** at the cap → `straitjacket:report-bug`
     (then the capture gate, step 3). Behavior gaps you **did** fix this run → note in the
     commit/summary; do **not** open an issue (a same-change fix needs no ticket).
   This loop is also the verification basis ([STAGES.md](../../docs/STAGES.md) rule 8) for any slice of
   the change that had no test seam — state it as *audit-checked + live-run-guarded* where applicable.
6. **`surviving_mutants`** → consumed by step 5 (lifted into fix-mode coverage). Report any that
   remain after the cap as known coverage gaps.

## Final summary (present verbatim)

- **Run metadata**: run_id, stack, rounds_run, harden rounds run.
- **Locked contracts** (the non-blocking contract surfacing): each `intended_behavior` with its
  `target_file`/`target_symbol` — audit these; the tests faithfully enforce them.
- **Implementation written**: files + symbols the `implementation-author` touched.
- **🚨 Surfaced bugs** (if any): ESCALATE.
- **🔬 Post-green audit** (if the hardening loop ran): lenses + tools run, and confirmed findings by
  class — behavior gaps fixed test-first (commits), quality findings applied as refactors.
- **🔧 Refactors applied / ↩️ reverted** (if any): each rejected refactor (broke green / mutated a
  test) named, so the would-be cleanup is visible as a known limitation rather than silently dropped.
- **Surviving mutants**: killed this run (lifted into fix-mode coverage) vs. remaining as known gaps.
- **Degraded steps** (tooling-absent paths; `--no-harden` / `--quick` skips).
- **Known-limitation note** (always): "`intended_behavior` was inferred by an LLM and is no longer
  gated by a human contract-review; tests faithfully enforce it — if the inference was wrong, the
  tests enforce the wrong contract. The surfaced contracts above are your audit hook."

## Error handling

- Workflow error → fall back to the staged Agent path for the remainder (the two paths are equivalent).
- CLI subcommand failure → log + surface in the summary.
- User interruption → best-effort `state.json` checkpoint.

## Notes

- The cycle is mostly Opus turns (coverage, authors, the pre-impl adversarial stack, implementation)
  plus Haiku runners (mutation, gate-runner). The workflow makes a single red→green→mutation pass
  (in-workflow iteration was retired with the post-green pivot); the **post-green hardening loop**
  (this session) is what iterates, up to `--max-harden-rounds`.
- All run artifacts live under `<repo>/.straitjacket/<run_id>/`; the bug ledger at
  `<repo>/.straitjacket/bugs.json` is tracked/committed. The `straitjacket` CLI is on `PATH`
  via the plugin's `bin/`.
