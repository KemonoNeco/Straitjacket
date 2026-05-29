---
name: tdd
description: "Drive new feature development with a practically-validated TDD flow: coverage planning from a spec → parallel test+stub authoring (tests compile-but-fail) → adversarial pre-validation on the RED tests → implementation by an implementation-author agent → adversarial + mutation passing-reason validation, under a savepoint red/green discipline. Reuses the straightjacket plugin's eleven specialist agents. Use when the user wants to implement a new feature TDD-style, write failing tests first, drive a new module from a specification, or do test-driven development on a Rust or C# project. Supports Rust (cargo + clippy + cargo-mutants) and C# (dotnet + Stryker.NET)."
---

# tdd

## Cardinal Rule 0 — YOU ARE THE LAUNCHER, NOT THE AUTHOR

**You — the main Claude session — own the *checkpoints* and the *state*; the fan-out work runs in specialist subagents.** You NEVER write test code or implementation code yourself; that's the multi-agent collapse failure mode this skill exists to prevent. The only files you write directly are `work-units.json`, `tooling.json`, `test-snapshot.json`, `state.json`, `.gitignore`, scaffolded C# test projects, and the final summary. Everything else comes from specialists.

This skill is **workflow-first**: the deterministic fan-out *phases* (authoring, adversarial review, implementation, mutation) run as **dynamic-Workflow stages** when the `Workflow` tool is available, and fall back to direct `Agent` dispatch when it is not. The *judgment* phases (contract review, the red/green gates, iterate-or-finalize) always run in this main session — a workflow cannot pause for you, so each fan-out stage is its own invocation and you regain control between them.

## Dispatch convention (read once, applies to every fan-out stage below)

**Capability check:** inspect your own available tools for one named `Workflow`.
- **Present** → run the stage as a workflow: `straightjacket workflow-script <stage>` (via Bash) emits the stage script to stdout; capture it verbatim and call `Workflow({script: <captured>, args: {...bindings}})`. The bindings are listed per stage; **never put the diff or author transcripts in the bindings** — adversarial agents Read the source themselves. When the workflow returns, parse its structured result and merge into `work-units.json` (you remain the single writer).
- **Absent** → legacy path: dispatch the same specialist agents directly via the `Agent` tool, **all parallel spawns in one message**, and merge their JSON yourself.

Either way the agents, prompts, schemas, and per-team caps are identical — the workflow only changes the dispatch substrate. The stage scripts are `adversarial` (the 3 specialists → synthesis [+ post-green mutation]) and `fanout` (generic capped parallel authoring/implementation).

## Plugin-internal specialist agents

The eleven `straightjacket` agents plus `implementation-author` for the green phase. See [the straightjacket SKILL.md](../regression/SKILL.md#plugin-internal-specialist-agents) for the full table. Additional role:

| `subagent_type` | Model | Effort | Tools | Notes |
|---|---|---|---|---|
| `implementation-author` | opus | xhigh | Read, Grep, Glob, Write, Edit | Stage D. Replaces `unimplemented!()` / `NotImplementedException` stubs with real implementations. Never modifies tests. |

The three `adversarial-*` specialists are **opus / high**; `adversarial-synthesis` is **opus / xhigh**.

## Args

- `<spec-text>` — inline specification text (required).
- `--quick` — skip mutation testing in stage E.
- `--with-fuzz` — add a fuzz pass in stage E (post-green only; fuzzing stubs is pointless).
- `--max-rounds N` — iteration cap (default 3).
- `--unattended` — skip the contract-review confirmation (the other gates still run).

## Cardinal rules

1. **You are the single writer** of `<repo_root>/.claude-regression/<run_id>/work-units.json`. Subagents/stages return JSON; you merge.
2. **Subagent prompts must be self-contained.** Pass work-unit data + paths inline; never rely on agent memory.
3. **`intended_behavior` is immutable** after the Coverage Reviewer writes it in stage A.
4. **The adversarial specialists never see the diff or transcripts.** Their `tools: Read, Grep, Glob` (no Bash) is the load-bearing isolation guarantee — verified to hold for workflow-spawned agents (spike `wf_060d27f3`). Never inline a diff into a binding/prompt; they Read the current source themselves. (The `PreToolUse` hook scans prompts in the legacy Agent path; it does NOT fire for workflow-spawned agents, so isolation rests on the tool restriction + you never passing the diff.)
5. **Parallel spawns go in a single message** (legacy path) / one `parallel()` batch (workflow path).
6. **JSON parse failures:** retry once, then abort that unit.
7. **Tests are read-only after they lock (end of stage C) — reviewer + name-survival.** Enforced by: (a) the explicit "never modify tests" rule passed to `implementation-author` with the test-snapshot; (b) the `adversarial-misalignment` specialist re-checking tests against the locked `intended_behavior` in stage E; (c) **behavioral name-survival** — `straightjacket run-new-tests` records the RED-phase test-name set, and at green every one of those names must still exist and now PASS (a deleted/renamed/`#[ignore]`-d test is caught); (d) an end-of-stage-E `verify-no-test-mutation` audit. No per-author SHA hook (false-positives on in-source Rust tests).

## Savepoint red/green discipline (inspired by [savepoint](https://github.com/NamtaoProductions/savepoint))

- **RED** (tests failing): never modify a test. Write or improve *code* to pass it when the test aligns with the locked `intended_behavior`; if it cannot pass honestly, or the test is misaligned, **restore to the last savepoint** (`git reset`/checkout to the last green commit) — never weaken a test, never leave a red tree.
- **GREEN** (tests passing): write/improve tests for new behavior, refactor freely.
- **A savepoint is a COMMIT, and you commit ONLY on QA'd green** — all tests pass *and* the new behavior is validated. The red-check is a *gate*, not a savepoint; **never commit the `unimplemented!()` skeleton** ("don't commit the skeleton"). The first savepoint is the first module's QA'd green.

## Preflight

1. Confirm the working dir is a git repo (else abort). Resolve `repo_root`.
2. Generate `run_id` = `<YYYYMMDDThhmmss>-<4hex>`; create `<repo_root>/.claude-regression/<run_id>/`.
3. Parse the spec from `<spec-text>`.

---

## Phase 1 — Baseline (this session)

1. `straightjacket detect-stack --repo-root <repo_root>` → `stack` (+ the `cargo_target` field; the nested-crate-aware resolver means **no hand-added root workspace is needed**).
2. `straightjacket snapshot-tests --repo-root <repo_root> --out-file <run_id>/test-snapshot.json`.
3. Probe tooling (cargo-mutants / cargo-fuzz / dotnet-stryker) → `tooling.json`.
4. Scaffold C# `*.Tests` projects if needed.
5. Baseline must be green (preflight enforces).

## Stage A — Coverage planning (single agent — no workflow)

Spawn one `coverage-reviewer` (`mode: "spec"`) with the spec, `stack`, the work-unit schema, and the instruction to populate `target_stub_path` for every unit. (A single agent is not a fan-out — dispatch it directly via `Agent` even when `Workflow` is present.) Validate the returned units against the schema with `straightjacket validate-work-units --work-units-file <run_id>/work-units.json`. Write `work-units.json` at `round: 0`.

**▸ CHECKPOINT — contract review (unless `--unattended`).** Print each `intended_behavior` with its `target_file`/`target_stub_path`. Ask the user for one confirmation. If they reject any unit, halt.

## Stage B — Test + stub authoring (fanout workflow) + red-check

Chunk units by `kind` (cap 6 per kind). Build a self-contained prompt per chunk (`mode: "tdd"`: APPEND a `#[cfg(test)]`/test-method into `output_file_path`; CREATE/EXTEND a stub at `target_stub_path` with an `unimplemented!()`/`NotImplementedException` body — compiles, fails at runtime). **Serialize writes:** one author owns one `output_file_path` (chunk so no two agents write the same file); add any shared deps once yourself before fan-out.

Dispatch via the **`fanout`** stage (`tasks: [{agentType, prompt, label}]`, `cap: 6`) per the dispatch convention. Then:
- `straightjacket verify-new-tests-compile …` (or the PostToolUse hook in legacy mode) — compile must pass; retry a failing chunk once, then quarantine.
- **Red-check:** `straightjacket run-new-tests --work-units-file <run_id>/work-units.json --stack <stack> --log-dir <run_id> --expect fail`. Every new test must FAIL (`RedOk`). A test that PASSES now is **vacuous** (`VacuousPreImpl`) — re-dispatch its author with a sharper prompt. **Branch on `nothing_to_run`** (loud-on-zero) — if it checked nothing, that's a failure, not a pass. Record the RED-phase test-name set (for name-survival).

**▸ GATE — red-check (NOT a commit).** Proceed only when every new test fails honestly. The skeleton is never committed.

## Stage C — Pre-implementation adversarial validation ON THE RED TESTS (adversarial workflow)

Run the **`adversarial`** stage with `args: { workUnits, stack, mode: "pre_impl", toolingAvailable, repoRoot }`. The three specialists (Read the tests + stubs themselves; no diff) review the RED tests; `adversarial-synthesis` dedupes/ranks and emits **test additions/strengthenings** (not mutation tasks — there's no impl yet). This catches vacuous / happy-path / misaligned tests **before** code is written to satisfy them (the implementation-author is told to *satisfy* the tests, so a bad test gets baked in as "correct").

Apply warranted additions/strengthenings (dispatch `unit-test-author` again), re-run the red-check, confirm still RED. **Tests LOCK here** — read-only from now on.

## Stage D — Implementation (fanout workflow, green)

Chunk units by `target_stub_path` (cap 4). Build each `implementation-author` prompt: `assigned_work_units`, `failing_tests` (contents), `stubbed_sources`, `stack`, `test_snapshot_path`, and the explicit **"you may NOT modify any test, period"** rule. Dispatch via the `fanout` stage (`agentType: "implementation-author"`, `cap: 4`).

Then `verify-new-tests-compile` + `straightjacket run-new-tests … --expect pass`. Tests must now PASS. If a unit can't pass, re-dispatch once with the failing output; then mark `surfaced_bug` and surface it (do NOT weaken the test).

**▸ GATE — green check.** All locked tests pass + compile/clippy clean.

## Stage E — Passing-reason validation (adversarial workflow, post-green) + name-survival

Run the **`adversarial`** stage with `mode: "post_green"`: the three specialists re-review tests + the real implementation, `adversarial-synthesis` emits ranked findings + **`mutation_runner_tasks`**, and (unless `--quick`) the capped mutation-runner team runs and reports surviving mutants. If `--with-fuzz`, also run the fuzz pass (straightjacket Phase 4b shape) against the real impl.

Then in this session:
- **Name-survival:** `straightjacket run-new-tests … --expect pass` and compare the green-phase test-name set to the RED-phase set — every RED test must still exist and pass (the `run_new_tests::name_survival` backstop). Any `missing`/`regressed` → fail loudly.
- **End-of-stage audit:** `straightjacket verify-no-test-mutation … --snapshot-file <run_id>/test-snapshot.json` (branch on `no_files_checked` — a 0-file audit is not a pass).
- **Surviving mutants → S8 auto-cover:** construct new work units describing the *under-tested behavior class* (never the mutant), and iterate back to stage B for them. The coverage-reviewer should have flagged any thin wrapper it intentionally left to this backstop.

**▸ SAVEPOINT — commit (only here, on QA'd green).** Commit the green snapshot. This is the first real savepoint.

## Phase 7 — Verify & finalize / iterate

1. Run new tests 3× (`--expect pass`); quarantine flaky.
2. **Iterate** (back to stage B) if: synthesis flagged unresolved ≥medium findings, OR mutation reported survivors, OR (`--with-fuzz`) crashes not yet converted to tests.
3. **Halt** when `round ≥ max-rounds` (default 3), or no new mutants killed, or no new tests produced.
4. **Final summary** — same shape as straightjacket plus an **"Implementation written"** section (files + symbols touched by `implementation-author`). Present verbatim.

---

## Error handling

- Subagent/stage timeout or failure → retry once, then quarantine; on a workflow error, fall back to the legacy Agent path for that stage (the two paths are equivalent).
- CLI subcommand failure → log + surface in summary.
- User interruption → best-effort `state.json` checkpoint.

## Notes

- The adversarial trio + synthesis + (post-green) mutation run as the `adversarial` workflow stage; authoring + implementation as the `fanout` stage. Single agents (coverage-reviewer) and all checkpoints/merges stay in this session.
- A non-trivial spec is mostly Opus turns (coverage, integration authors, the adversarial stack, implementation) plus Haiku mutation/fuzz runners; it may iterate up to `--max-rounds`.
- All artifacts live under `<repo>/.claude-regression/<run_id>/`. The `straightjacket` CLI is on `PATH` via the plugin's `bin/`.
