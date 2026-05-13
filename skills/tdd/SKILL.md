---
name: tdd
description: "Drive new feature development with a practically-validated TDD flow: coverage planning from a spec → parallel test+stub authoring (tests compile-but-fail) → adversarial pre-validation → implementation by an implementation-author agent → adversarial + mutation passing-reason validation. Reuses the regression-tests plugin's eleven specialist agents. Use when the user wants to implement a new feature TDD-style, write failing tests first, drive a new module from a specification, generate stubs and implementations for a spec, or do test-driven development on a Rust or C# project. Supports Rust (cargo + clippy) and C# (dotnet + Stryker.NET)."
---

# tdd

## Cardinal Rule 0 — YOU ARE THE ORCHESTRATOR

**You — the main Claude session — execute every phase below yourself.** Specialist subagents are the ONLY things you spawn via the `Agent` tool. You never write test code or implementation code directly; that's the multi-agent collapse failure mode this skill exists to prevent.

The only files you (the main session) write directly are: `work-units.json`, `tooling.json`, `test-snapshot.json`, `state.json`, `.gitignore`, `diagnostics-*.txt`, scaffolded C# test projects, and the final markdown summary report. Everything else comes from specialists.

## Plugin-internal specialist agents

Same eleven agents as the `regression-tests` skill, plus `implementation-author` for the TDD green phase. See [the regression-tests SKILL.md](../regression-tests/SKILL.md#plugin-internal-specialist-agents) for the full table. The only additional role for tdd:

| `subagent_type` | Model | Effort | Tools | Notes |
|---|---|---|---|---|
| `implementation-author` | opus | xhigh | Read, Grep, Glob, Write, Edit | Phase 5. Replaces `unimplemented!()` / `NotImplementedException` stubs with real implementations. |

## Args

- `<spec-text>` — inline specification text (required for spec mode; file-path mode is a future enhancement).
- `--quick` — skip mutation testing in Phase 6.
- `--with-fuzz` — add a fuzz pass in Phase 6 (default off; fuzzing pre-implementation makes no sense, post-impl it's slow for iterative dev).
- `--max-rounds N` — override default 3.
- `--unattended` — skip the post-Phase-2 contract review confirmation.

## Cardinal rules

1. **You are the single writer** of `<repo_root>/.claude-regression/<run_id>/work-units.json`.
2. **Subagent prompts must be self-contained.**
3. **`intended_behavior` is immutable** after the Coverage Reviewer writes it in Phase 2.
4. **The adversarial specialists never see the diff or transcripts.** The PreToolUse hook scans prompts; defense-in-depth applies.
5. **Parallel spawns go in a single message.**
6. **JSON parse failures: retry once, then abort that unit.**
7. **Tests are read-only after Phase 3.** The implementation-author in Phase 5 must NOT modify tests; the post-impl hook re-runs verify-no-test-mutation to enforce this.

## Preflight

1. **Confirm working directory is a git repo.** If not, abort.
2. Resolve `repo_root` = absolute path to current working tree.
3. Capture `now_iso` and generate `run_id` = `<YYYYMMDDThhmmss>-<4-char hex>`.
4. Create `<repo_root>/.claude-regression/<run_id>/`.
5. Parse spec input from `<spec-text>` argument.

The plugin's `UserPromptExpansion` hook runs `regression-tests preflight` on invocation. If baseline is red, the skill does NOT run.

---

## Phase 1 — Baseline (subset of regression-tests Phase 1)

1. Run `regression-tests detect-stack --repo-root <repo_root>` for `stack`.
2. Run `regression-tests snapshot-tests --repo-root <repo_root> --out-file <run_id>/test-snapshot.json`.
3. Scaffold C# test projects if needed (same as regression-tests Phase 1 step 8).
4. Baseline must already be green (preflight hook enforced this).

---

## Phase 2 — Coverage Planning (`coverage-reviewer` in spec mode)

Build the spawn header:
- `mode: "spec"`.
- The user's `<spec-text>`.
- `stack`, `run_id`, scaffolded test project paths.
- The work-unit schema (`schemas/work-unit.schema.json`).
- Explicit instruction: "Populate `target_stub_path` for every work unit — the source file where the stub will live."

Spawn `subagent_type: "coverage-reviewer"`. Wait. Validate the returned WorkUnit list against the schema (each unit must have `target_stub_path` non-null).

Write to `<run_id>/work-units.json` as `round: 0`.

**Post-Phase-2 contract review (unless `--unattended`).** Print `intended_behavior` strings with their target paths and `target_stub_path`. Ask the user for one confirmation. If they reject any, halt.

---

## Phase 3 — Test + Stub Authoring (parallel author teams)

Partition work units by `kind`, chunk into ~3-5 units per agent, hard cap 6 parallel per kind.

For each chunk, build the prompt header:
- `mode: "tdd"` (so authors know to ALSO write stubs at `target_stub_path`).
- The assigned work units (each with `target_stub_path` populated).
- For each work unit, the contents of `target_stub_path` if it exists (may not yet).
- The test snapshot path.
- Explicit instruction: "For each work unit, (a) APPEND a `#[cfg(test)] mod tests` block or test method into `output_file_path`; (b) CREATE OR EXTEND `target_stub_path` with a stub function/method whose body is `unimplemented!()` / `throw new NotImplementedException()`. Stub must compile, must fail at runtime."

**Spawn all author chunks in a single message** with parallel `Agent` blocks. `subagent_type: "unit-test-author"` for unit-kind, `"integration-test-author"` for integration-kind.

The PostToolUse hook automatically runs:
1. `verify-no-test-mutation` against the snapshot.
2. `verify-new-tests-compile` — compile must succeed.

Then YOU run:
3. `regression-tests run-new-tests --repo-root <repo_root> --work-units-file <run_id>/work-units.json --stack <stack> --log-dir <run_id> --expect fail` — the TDD red-check. Each test should FAIL at runtime (stub panics). Tests that PASS at this stage are vacuous (passing without an implementation = bad test). Classification:
   - `red_ok` → keep.
   - `vacuous_pre_impl` → re-dispatch the author with a sharper prompt naming the vacuous test.

Merge author results into `work-units.json`. Successful units get `status: written`.

---

## Phase 4 — Practical Pre-Validation (adversarial team + synthesis)

Three Sonnet specialists in parallel, then Opus synthesis. Same shape as the regression-tests Phase 4a, BUT:
- `mode: "tdd-phase-4"`.
- There is no implementation yet — `source_under_test` contains only the stubs.
- The happy-path specialist's input shape: compare tests to the SPEC's edge-handling expectations, not the (nonexistent) implementation.
- No mutation runners at this phase (no impl to mutate).

If synthesis returns findings with severity ≥ medium, re-dispatch the relevant authors with the synthesized feedback. Allow one retry per unit, then continue.

---

## Phase 5 — Implementation (`implementation-author`, parallel team)

For each work unit with `status: written`, dispatch an `implementation-author` agent.

Chunk by file: stubs in the same `target_stub_path` get bundled into one agent (avoid concurrent writes to one file).

Cap: max 4 parallel implementation-author agents.

For each chunk, build the prompt:
- `assigned_work_units`: the units this agent owns.
- `failing_tests`: contents of every test file at the `output_file_path` of any assigned unit.
- `stubbed_sources`: contents of every file at `target_stub_path` for assigned units.
- `stack`.
- `test_snapshot_path`.
- `mode: "tdd-phase-5"`.

Spawn all chunks in a single message. `subagent_type: "implementation-author"`.

The PostToolUse hook automatically runs:
1. `verify-new-tests-compile`.
2. `regression-tests run-new-tests --expect pass` — tests must now pass.

If tests still fail, re-dispatch the failing implementation-author with the failing-test output for one retry. After that, mark the unit `status: surfaced_bug` (the implementation could not satisfy the contract — investigate manually).

Successful units get `status: implemented`.

---

## Phase 6 — Passing-Reason Validation (adversarial team + synthesis + mutation runners, parallel)

In a single message, parallel spawns:

- **Three adversarial specialists** (Sonnet 4.6): re-review tests + implementation. `mode: "tdd-phase-6"`. Same per-specialist isolation guarantee.
- **Mutation runner team** (Haiku, capped at 2-3 parallel): run mutation testing on the new source code. Surviving mutants prove the tests don't pin the contract.
- **Fuzz pipeline** (if `--with-fuzz`): same as regression-tests Phase 4b.

After specialists return, spawn one Opus `adversarial-synthesis` agent with the specialist reports. The mutation results aren't passed to synthesis directly — the orchestrator merges them into new work-unit proposals.

Surviving mutants → construct new work units describing the under-tested behavior class (NOT the mutant). Iterate back to Phase 3 if any new work units are accepted.

---

## Phase 7 — Verify & Finalize

1. **Run new tests 3x.** `regression-tests run-new-tests ... --expect pass`. All units should be `all_pass`. Quarantine flaky.

2. **Iteration check.** Trigger another round if:
   - Adversarial synthesis flagged unresolved high-severity findings, OR
   - Mutation runners reported surviving mutants, OR
   - (--with-fuzz) Fuzz runners produced crash artifacts not yet converted to tests.

3. **Termination.** Halt iteration when `round ≥ args.max_rounds` (default 3), or when no new mutants are killed this round, or when no new tests are produced.

4. **Final summary report.** Same shape as regression-tests, with an added section:
   - **Implementation written**: list of files + symbols touched by `implementation-author`.

Present the report verbatim to the user.

---

## Error handling

- Subagent timeout / failure → retry once, then quarantine.
- CLI subcommand failure → log and surface in summary.
- User interruption → best-effort `state.json` checkpoint.

## Notes

- The skill spawns Opus and Sonnet specialists and may iterate up to 3 rounds. A non-trivial spec can cost 15-25 Opus turns plus 8-15 Sonnet turns across specialists.
- All artifacts live under `<repo>/.claude-regression/<run-id>/`.
- The `regression-tests` CLI is on PATH via the plugin's `bin/`.
- `--quick` skips mutation testing in Phase 6. `--with-fuzz` adds the fuzz pass.
