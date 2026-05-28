---
name: regression-tests
description: "Generate regression tests for recent changes or a target module using a multi-agent workflow (coverage review → parallel test authoring → adversarial specialist team + synthesis → real mutation testing → fuzzing with reproducer mining). Enforces compile/lint baselines before and after authoring via plugin hooks. Use when the user asks to write regression tests, generate tests for a diff or PR, lock in current behavior, harden a module against regressions, mine fuzz crashes into deterministic tests, add tests for under-tested code, write tests for changes, increase test coverage, or add edge-case tests. Supports Rust (cargo + clippy + cargo-mutants + cargo-fuzz) and C# (dotnet + Stryker.NET + SharpFuzz). Writes tests directly into the repo."
---

# regression-tests

## Cardinal Rule 0 — YOU ARE THE ORCHESTRATOR

**You — the main Claude session the user is talking to — execute every phase below yourself.** Specialist subagents (the eleven plugin-internal agents shipped under this plugin's `agents/`) are the ONLY things you spawn via the `Agent` tool. There is no nested "orchestrator agent." If you find yourself writing one, stop.

**You never write test code yourself.** If you find yourself reaching for `Write` or `Edit` to create a `_test.rs`, `Tests.cs`, or any file whose path matches a `WorkUnit.output_file_path`, that is the multi-agent collapse failure mode this skill exists to prevent. Stop and dispatch the appropriate specialist instead. Test source files come exclusively from specialist `Agent` results — never from your own `Write`/`Edit` calls.

The only files you (the main session) write directly are: `work-units.json`, `tooling.json`, `test-snapshot.json`, `coverage-baseline.json`, `scaffolded.json`, `state.json`, `.gitignore`, `diagnostics-*.txt`, scaffolded C# test projects (via `dotnet new`), and the final markdown summary report. Everything else comes from specialists.

## Plugin-internal specialist agents

You spawn these by `subagent_type` (bare name — the plugin namespace is implicit). Do NOT pass `model:` — each agent's frontmatter locks its tier and tool list.

| `subagent_type` | Model | Effort | Tools | Notes |
|---|---|---|---|---|
| `coverage-reviewer` | opus | xhigh | Read, Grep, Glob | Phase 2. Single agent. Locks `intended_behavior`. |
| `unit-test-author` | sonnet | high | Read, Grep, Glob, Write, Edit | Phase 3. Parallel team (chunk ~3-5 units/agent). |
| `integration-test-author` | opus | xhigh | Read, Grep, Glob, Write, Edit | Phase 3. Parallel team. |
| `adversarial-vacuousness` | opus | xhigh | Read, Grep, Glob | Phase 4a specialist (vacuous assertions + test-mutation patterns). |
| `adversarial-happy-path` | opus | xhigh | Read, Grep, Glob | Phase 4a specialist (happy-path bias + edge-case enumeration). |
| `adversarial-misalignment` | opus | xhigh | Read, Grep, Glob | Phase 4a specialist (test ↔ contract alignment). |
| `adversarial-synthesis` | opus | xhigh | Read, Grep, Glob | Phase 4a synthesis over the three specialists' reports. |
| `mutation-runner` | haiku | — | Read, Bash, PowerShell | Phase 4a. Parallel team capped at 2-3. Mechanical. |
| `fuzz-harness-author` | opus | xhigh | Read, Grep, Glob, Write, Edit, Bash, PowerShell | Phase 4b. Single. |
| `fuzz-runner` | haiku | — | Read, Glob, Bash, PowerShell | Phase 4b. Parallel team capped at 2. Mechanical. |

## Cardinal rules

1. **You are the single writer** of `<repo_root>/.claude-regression/<run_id>/work-units.json`. Subagents return results in their `Agent` payload; you parse and merge.
2. **Subagent prompts must be self-contained.** Agents have no memory of prior runs. Always pass work-unit data and source paths inline (or via file paths the agent must read).
3. **`intended_behavior` is immutable** after the Coverage Reviewer writes it. Reject any subagent output that rewrites it.
4. **The Adversarial specialists never see the diff** (or "what changed" framing, or author transcripts). The plugin's PreToolUse hook scans the constructed prompt for forbidden strings (`--- a/`, `+++ b/`, `git diff`) and blocks the spawn if found. As defense-in-depth, also avoid inlining diff text yourself.
5. **Parallel spawns go in a single message** with multiple `Agent` tool-use blocks. Sequential messages defeat the concurrency. This applies to chunked author teams in Phase 3, the three adversarial specialists in Phase 4a, and the adversarial+fuzz pair in Phase 4.
6. **Subagent response JSON parse failures**: retry once with a "your previous response was not valid JSON — return only valid JSON matching <schema>" prefix. After one retry, abort that work unit and continue.

## Args

Parse from the user's invocation. Recognized flags:

- `<path>` — explicit target (file, directory, or `crate::module` symbol). Absent → diff mode.
- `--quick` — skip mutation and fuzzing.
- `--no-fuzz` — skip only fuzzing.
- `--fuzz-time <seconds>` — per-target fuzz budget (default 60).
- `--max-rounds N` — override default 3.
- `--dry-run` — write tests to staging instead of into the repo.
- `--unattended` — skip the post-Phase-2 contract review confirmation.

## Preflight

1. **Confirm working directory is a git repo.** If `git rev-parse --is-inside-work-tree` returns false, abort with a message asking the user to run from a git working tree.
2. Resolve `repo_root` = absolute path to the current working tree.
3. Capture `now_iso` = current ISO-8601 timestamp.

The plugin's `UserPromptExpansion` hook fires on the slash-command invocation and runs `regression-tests preflight` (combined detect-stack + baseline-check + lint-check). If the hook blocks with `decision: "block"`, the skill does NOT run — investigate the failing checks first.

---

## Phase 1 — Detect & Baseline

0. **Generate `run_id`** = `<now_iso compacted: YYYYMMDDThhmmss>-<4-char hex>`. Create `<repo_root>/.claude-regression/<run_id>/`. Append `.claude-regression/` to `<repo_root>/.gitignore` if not already there.

1. **Scope detection.**
   - If `args.target` is set → target mode; scope = the user's path/symbol.
   - Otherwise → diff mode:
     - `default_branch` = `git symbolic-ref refs/remotes/origin/HEAD` (stripped of `refs/remotes/origin/`). Fallback: try `main`, `master`, `develop` via `git rev-parse --verify`. If none exist, abort with "no default branch found — diff mode requires a comparison point."
     - `merge_base` = `git merge-base HEAD origin/<default_branch>`.
     - `diff` = `git diff <merge_base>...HEAD`.
     - `untracked` = `git status --porcelain` (entries starting with `??`).
     - `scope` = union of files in `diff` + `untracked`.

2. **Stack detection.** Run `regression-tests detect-stack --repo-root <repo_root>`. Output: `rust` | `csharp` | `both` | `none`. If `none`, abort with "no supported stack found."

3. **Tooling check.** For each detected stack, probe for tools:
   - Rust: `cargo-mutants` (`cargo mutants --version`), `cargo-fuzz` (probe via `cargo fuzz --version 2>&1 | <discard>` or `cargo fuzz list 2>&1`; **never** call `cargo fuzz --version` with a live stdout to a terminal — cargo-fuzz v0.13.1 pulls in `is-terminal v0.4.1` which panics on certain Windows console widths during terminal-width probing, so use redirected output or treat the "could not read fuzz/Cargo.toml" error from `cargo fuzz list` as "installed"), `cargo-llvm-cov` (`cargo llvm-cov --version`).
   - C#: `dotnet stryker --version`, `sharpfuzz --version`, `reportgenerator`.
   - Record presence/absence in `<run_id>/tooling.json`. Missing tools → degrade gracefully:
     - Mutation tooling absent → adversarial Phase 4a runs static-only.
     - Fuzz tooling absent → skip Phase 4b entirely.
     - Coverage tooling absent → skip coverage delta in Phase 5.

4. **Baseline green.** The plugin's preflight hook already ran `baseline-check`; you don't need to run it again unless it was skipped.

5. **Baseline compile/lint clean.** Same — preflight already ran `lint-check`.

6. **Test snapshot.** Run `regression-tests snapshot-tests --repo-root <repo_root> --out-file <run_id>/test-snapshot.json`. SHA-256 every pre-existing test file.

7. **Baseline coverage snapshot.** If coverage tool is present, run it now and save to `<run_id>/coverage-baseline.json`. If absent, skip.

8. **Test project scaffolding (C# only).** For each `*.csproj` in scope without a sibling `*.Tests` project, run:
   ```
   dotnet new xunit -n <Project>.Tests
   dotnet sln add <Project>.Tests/<Project>.Tests.csproj
   dotnet add <Project>.Tests reference <Project>.csproj
   ```
   Record scaffolded projects in `<run_id>/scaffolded.json`.

9. **Budget estimate.** Print to user: `"Estimated 15-45 minutes; use --quick to skip mutation+fuzzing, --no-fuzz to skip only fuzzing. Run-id: <run_id>"`.

---

## Phase 2 — Coverage Planning (`coverage-reviewer`)

Build the spawn header containing:
- `mode`: `diff` or `target`.
- In diff mode: the full `diff` text and the list of changed files.
- In target mode: the resolved file/symbol paths, plus contents of any `CLAUDE.md` in or above those paths.
- `stack`: `rust` | `csharp` | `both`.
- `run_id` and `output_dir`.
- The work-unit JSON schema (`schemas/work-unit.schema.json`).
- For C#, the scaffolded `*.Tests` project paths from step 8.

Spawn the coverage reviewer via `Agent` with `subagent_type: "coverage-reviewer"` and the constructed prompt. Wait for the response.

Parse the response. Expected: a JSON list of WorkUnit records. Validate against `work-unit.schema.json`. Reject any entry missing required fields. Write the validated list to `<run_id>/work-units.json` as `round: 0`.

**`--dry-run` path rewrite (BEFORE Phase 3 dispatch).** If `args.dry_run` is set, mutate every `output_file_path` to point under `<run_id>/staged-tests/`. Mirror the directory structure. Persist the mutation in `work-units.json`.

**Post-Phase-2 contract review (unless `--unattended`).** Print the list of `intended_behavior` strings (with their `target_file` and `target_symbol`) to the user and ask for one confirmation. If they reject any, halt.

---

## Phase 3 — Parallel Author Teams (`unit-test-author`, `integration-test-author`)

Partition work units by `kind`, then **chunk into agent teams**:

- Group by `kind`: `unit` → unit-test-author team, `integration` → integration-test-author team.
- Within each kind, chunk work units so each Agent gets ~3-5 units. Prefer to keep work units targeting the same source file in the same chunk.
- Hard cap: max 6 parallel author Agents per kind.

For each chunk, build a prompt header containing:
- `mode: "regression-tests"` (so authors do NOT generate stubs at `target_stub_path`).
- The assigned work units for this chunk as a JSON array.
- For each work unit, the source-under-test file contents.
- The pre-existing test snapshot file path so the author can avoid modifying existing files.
- The locked `intended_behavior` per unit, plus the explicit rule: "You may CREATE `output_file_path`. You may NOT modify any test file listed in test-snapshot.json. You may NOT rewrite `intended_behavior`."

**Spawn every author chunk in a single message** — one `Agent` tool-use block per chunk, all in parallel.

The plugin's PostToolUse hook automatically runs `verify-new-tests-compile` after each author returns. If the hook blocks (compile failure), the diagnostic comes back to you for retry; re-dispatch the failing units with the diagnostic inlined. Allow one retry per unit; after that, mark `status: quarantined`.

After all author chunks have returned, run `regression-tests verify-no-test-mutation --repo-root <repo_root> --snapshot-file <run_id>/test-snapshot.json` ONCE as an end-of-phase audit. Surface any reported violations in the run summary — these are pre-existing test files that an author touched against the prompt rule. They do not block iteration (the adversarial-vacuousness specialist re-checks the test corpus in Phase 4a), but they are noteworthy.

Merge author results into `work-units.json`. For each successful unit, set `status: written` and confirm `output_test_name`.

---

## Phase 4 — Adversarial Validation (parallel specialists + fuzz)

In a single message, spawn the three Phase 4a specialists in parallel. Then (after they return) run the synthesis pass, and in parallel with that the fuzz pipeline.

### 4a. Adversarial specialist team + synthesis

**Three Opus specialists, single message, parallel:**

Build a shared "no-diff" header containing:
- `mode: "regression-tests-phase-4a"`.
- Post-change source code for every file touched by work units. **NOT THE DIFF.**
- The locked `intended_behavior` for each test (from `work-units.json`).
- The test code as written (read from disk at the `output_file_path` values).
- `stack`: `rust` | `csharp`.

**Guard:** the plugin's PreToolUse hook scans the prompt for forbidden strings (`--- a/`, `+++ b/`, `git diff`) and blocks the spawn if found. Defense-in-depth: do not inline diff text yourself.

Spawn three agents in parallel:
- `subagent_type: "adversarial-vacuousness"` — vacuous assertions + test-mutation patterns.
- `subagent_type: "adversarial-happy-path"` — happy-path bias + uncovered-edge-case enumeration.
- `subagent_type: "adversarial-misalignment"` — test ↔ `intended_behavior` alignment.

Wait for all three. Each returns a specialist report (JSON).

**Then spawn one Opus synthesis agent:**
- `subagent_type: "adversarial-synthesis"`.
- Pass the three specialist reports as input (`specialist_reports`).
- Also pass `tooling_available` from `tooling.json` (so synthesis can produce `mutation_runner_tasks`).

The synthesis output is the canonical adversarial review: deduplicated `static_findings`, merged `new_work_unit_proposals`, and `mutation_runner_tasks`.

**Mutation runner dispatch.** For each task in `mutation_runner_tasks`, spawn `subagent_type: "mutation-runner"`. **Spawn mutation runners as a parallel team capped at 2-3 concurrent** (single message, multiple Agent blocks). Each returns surviving mutants.

If mutation tooling was absent (Phase 1 step 3), skip mutation runners and log loudly.

### 4b. Fuzz Harness Author + Runners

Skip this entire sub-phase if any of: `args.quick`, `args.no_fuzz`, no fuzz tooling detected.

Build the spawn header containing:
- Fuzzable targets (work units with `fuzzable: true`).
- Project-specific scaffolding info from `regression-tests fuzz-setup --repo-root <repo_root> --stack <stack>`.
- The per-target time budget (`args.fuzz_time` or 60s).

Spawn `subagent_type: "fuzz-harness-author"`. It writes harnesses and returns runner tasks.

For each runner task, spawn `subagent_type: "fuzz-runner"`. **Parallel team capped at 2.**

For each crash artifact, run:
```
regression-tests reproducer-to-test \
  --repro-path <path> \
  --target-file <file> \
  --target-function <symbol> \
  --stack <stack> \
  --repo-root <repo_root> \
  --work-units-file <run_id>/work-units.json
```
This generates a deterministic regression test named after a hash of the input bytes, places it in a `regressions/` test module, and appends a new WorkUnit.

---

## Phase 5 — Verify & Finalize

1. **Run new tests 3x.** Run `regression-tests run-new-tests --repo-root <repo_root> --work-units-file <run_id>/work-units.json --stack <stack> --log-dir <run_id>`. The CLI returns per-unit results with `classification` (all_pass / all_fail / flaky / never_found) and a recommended `status`.

   Classify per the recommended status:
   - `all_pass` → `status: written`. Keep.
   - `all_fail` → `status: surfaced_bug`. **ESCALATE in summary.**
   - `flaky` → `status: quarantined`. Move to `<run_id>/quarantine/`.

2. **Iteration check.** Trigger another round if:
   - Adversarial synthesis flagged unresolved findings (severity ≥ medium), OR
   - Mutation runners reported surviving mutants killed by no test, OR
   - Fuzz runners produced crash artifacts not yet converted to tests.

   For each surviving mutant, construct a NEW work unit whose `intended_behavior` describes the **under-tested behavior class**, NOT the mutant itself.

3. **Termination.** Halt iteration when ANY of:
   - `round` ≥ `args.max_rounds` (default 3).
   - No new mutants killed this round.
   - No new tests produced this round.

4. **Coverage delta.** If coverage tool was present, re-run and compute deltas against `coverage-baseline.json`.

5. **`--dry-run` finalization.** Print location of staged tests for user to apply.

6. **Final summary report.** Produce a single markdown report. Sections:
   - **Run metadata**: run_id, mode, stacks, time elapsed.
   - **Tests added** (by file, count per file).
   - **🚨 Surfaced bugs** (if any): ESCALATE.
   - **Surviving mutants** (if any).
   - **Fuzz reproducers committed**.
   - **Coverage delta**.
   - **Quarantined flaky tests**.
   - **Degraded steps** (tooling-absent paths).
   - **Known-limitation note** (always): "`intended_behavior` was inferred by an LLM. Tests are anchored to that inference; if the inference was wrong, tests will faithfully enforce the wrong contract."

Present the report verbatim to the user as your end-of-turn output.

---

## Error handling

- Subagent timeout / failure → retry once. On second failure, mark the affected work unit `status: quarantined`, continue with others.
- CLI subcommand failure (non-zero exit) → log full stdout/stderr and surface in the summary.
- If the user interrupts: best effort — write `<run_id>/state.json` checkpoint after each phase boundary.

## Notes

- The skill spawns Opus agents (coverage-reviewer, integration-test-author, the three adversarial specialists, synthesis, fuzz-harness-author), Sonnet (unit-test-author), and Haiku runners, and may iterate up to 3 rounds. The adversarial trio is now Opus (chosen for critique catch-rate), so a non-trivial diff is mostly Opus turns — budget accordingly. Print the budget estimate from Phase 1 step 9.
- All file artifacts live under `<repo>/.claude-regression/<run-id>/`.
- The `regression-tests` CLI is on PATH via the plugin's `bin/` directory while the plugin is enabled.
