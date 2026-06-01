# Straitjacket stage vocabulary & shared chunks

This is the canonical reference for the plugin's **reusable chunks** - the specialist agents,
the workflow stages, the coverage modes, and the cardinal rules that every skill composes.
The skills (`tdd`, `audit`, `fuzz`, `mutation`, `debug`, `triage`, `report-bug`) are thin
launchers; the engine lives here.

> The retired `regression` skill's machinery is documented here. Its diff/target coverage
> modes and the `fanout`/`adversarial` stages are **retained-dormant** - functional, with no
> launcher of their own - and consumed by `tdd` (fix mode), `triage`, and `audit`. A
> `characterize` launcher could re-expose diff-mode later; none is needed now.

## The substrate classifier - stage vs agent vs main-session

One rule decides where each piece of work lives:

| Shape | Substrate | Examples |
|---|---|---|
| Gateless multi-agent fan-out | **Workflow stage** (`workflows/*.js`) | `fanout`, `adversarial`, `audit` |
| Single agent, no fan-out | **Direct `Agent` dispatch** (not a stage) | `coverage-reviewer`, `fuzz-harness-author`, `root-cause-analyst` |
| Needs a user gate / owns state | **Main session** (the skill) | red/green gates, savepoint commit, the single-writer `work-units.json` merge, ledger writes |
| Intra-turn iterate-with-tools | **A single agent that loops internally** | `root-cause-analyst` |

Every skill follows the same arc: **parse args → own gates/state → emit a stage script via the
CLI and run it through the `Workflow` tool with `args` bindings → merge the structured result
as the single writer.** Judgment, prompts, and checkpoints stay in the session; deterministic
choreography lives in the `.js` stages; isolation lives in each agent's `tools:` list.

## Specialist agent roster

You spawn these by `subagent_type` (bare name - the plugin namespace is implicit). Do **not**
pass `model:` - each agent's frontmatter locks its tier and tool list. Tool restrictions are
**load-bearing isolation guarantees**, not advice.

| `subagent_type` | Model | Effort | Tools | Role |
|---|---|---|---|---|
| `coverage-reviewer` | opus | xhigh | Read, Grep, Glob | Enumerates behaviors and **locks** immutable `intended_behavior` work units. Modes: diff / target / spec. Single agent. |
| `unit-test-author` | sonnet | high | Read, Grep, Glob, Write, Edit | Writes unit tests for assigned work units. Parallel team (chunk ~3-5 units/agent). |
| `integration-test-author` | opus | xhigh | Read, Grep, Glob, Write, Edit | Writes integration tests (setup/teardown, doubles, determinism). Parallel team. |
| `adversarial-vacuousness` | opus | high | Read, Grep, Glob | Vacuous-assertion + test-mutation-pattern lens. **No Bash** (diff-blind). |
| `adversarial-happy-path` | opus | high | Read, Grep, Glob | Happy-path-bias + edge-case-enumeration lens. **No Bash.** |
| `adversarial-misalignment` | opus | high | Read, Grep, Glob | Test ↔ contract alignment lens. **No Bash.** |
| `adversarial-synthesis` | opus | xhigh | Read, Grep, Glob | Dedupes/ranks the three reports; emits `new_work_unit_proposals` + `mutation_runner_tasks`. |
| `mutation-runner` | haiku | - | Read, Bash, PowerShell | Mechanical: runs cargo-mutants / dotnet-stryker on a target → surviving mutants. |
| `fuzz-harness-author` | opus | xhigh | Read, Grep, Glob, Write, Edit, Bash, PowerShell | Writes libFuzzer / SharpFuzz harnesses → runner tasks. Single. |
| `fuzz-runner` | haiku | - | Read, Glob, Bash, PowerShell | Mechanical: runs one harness for a time budget → crash artifacts. |
| `implementation-author` | opus | xhigh | Read, Grep, Glob, Write, Edit | Replaces `unimplemented!()` / `NotImplementedException` stubs (tdd green) **or** fixes buggy source (fix mode). **Never modifies tests.** |
| `gate-runner` | haiku | - | Read, Glob, Bash, PowerShell, Write | Mechanical: materializes work-units.json and runs one straitjacket CLI gate (run-new-tests / verify-*); the in-workflow sequential single-writer for `tdd-cycle`. |
| `audit-<lens>` (×7) | opus | high | Read, Grep, Glob | Isolated source-audit finders, one per lens (latent-bug, security, performance, dead-code, doc-drift, concurrency, error-handling). **No Bash.** Emit findings per `schemas/audit-finding.schema.json` (`lens` field is un-prefixed). |
| `audit-runner` | haiku | - | Read, Bash, PowerShell, Glob | Mechanical: runs one `straitjacket audit-run --tool …` and returns its normalized findings. |
| `audit-refuter` | opus | high | Read, Grep, Glob | Skeptic: votes refute/survive/uncertain over the full LLM-finding set; defaults to refute when unconfirmable. **No Bash.** |
| `audit-synthesis` | opus | xhigh | Read, Grep, Glob | Dedupes/ranks audit survivors + mechanical findings; corroborates LLM+tool agreement; assigns disposition. Distinct from `adversarial-synthesis` (which works on test reports). |
| `root-cause-analyst` | opus | xhigh | Read, Grep, Glob, Bash, PowerShell | The debugger (debug/triage skills). Reproduces + root-causes one bug from green (**NO Edit**; leaves the tree green); returns the 3 bridge fields + root_cause + reproduction. Single intra-turn agent, not a stage. |

## Dispatch convention - workflow-first, with Agent fallback

The deterministic fan-out phases run as **dynamic-Workflow stages** when the `Workflow` tool is
available, and fall back to direct `Agent` dispatch when it is not. Single agents (e.g.
`coverage-reviewer`) and every merge/checkpoint stay in the main session - a workflow cannot
pause, so each fan-out stage is its own invocation and the session regains control between them.

**Capability check:** inspect your own available tools for one named `Workflow`.

- **Present** → run `straitjacket workflow-script <stage>` (Bash) to emit the stage script to
  stdout, capture it verbatim, and call `Workflow({script: <captured>, args: {...bindings}})`.
  Parse the structured result and merge into `work-units.json` (you stay the single writer).
- **Absent** → dispatch the same agents directly via `Agent`, all parallel spawns in one message.

Either way the agents, prompts, schemas, and per-team caps are identical - the workflow only
changes the dispatch substrate. **The diff is never a workflow binding**; isolated specialists
Read the post-change source themselves. Their `tools:` restriction holds for workflow-spawned
agents too (spike `wf_060d27f3`), so the no-diff guarantee stands - but the `PreToolUse`
diff-scan hook fires only in the Agent path, so in the workflow path isolation rests entirely on
the tool restriction + you never passing the diff.

## Workflow stages

### `fanout`

Generic capped parallel dispatch. The skill builds each task's self-contained prompt and picks
the `agentType`; the script runs them in parallel within the cap and returns per-task results.

- **args:** `tasks: [{agentType, prompt, label}]` (one per chunk), `cap` (authors 6, implementation 4, runners 2-3).
- **returns:** `{stage, chunk_count, results, raw}` - `results` is the flattened per-unit list (authoring/impl path); `raw` is every chunk verbatim, so mechanical-runner shapes (`{surviving_mutants}` / `{crashes}`) survive when `fuzz`/`mutation` reuse this stage.

### `adversarial`

Three isolated specialists fan out in parallel → `adversarial-synthesis` dedupes/ranks → (in
`post_green` mode only) a capped mutation-runner team runs.

- **args:** `{workUnits, stack, mode, toolingAvailable, repoRoot}`. `mode`: `pre_impl` (emit test additions/strengthenings while RED, no mutation), `post_green` (emit `mutation_runner_tasks` + run the team), `lock` (characterization - dormant).
- **returns:** `{stage, mode, synthesis, specialist_reports, mutation_results}`.
- **The diff is never an arg** - specialists Read source + tests themselves.

### `tdd-cycle`

The consolidated test-first cycle as ONE resumable workflow (Phase 1): coverage → author →
red-check → pre-impl adversarial → implement → green-check → post-green adversarial + mutation,
iterating to a cap. Gates run via the `gate-runner` agent and the script branches on the verdict.
Inlines the `fanout` + 3-specialist→synthesis choreography (scripts can't import one another).

- **args:** `{spec, stack, repoRoot, outputDir, workUnitsPath, testSnapshotPath, toolingAvailable, maxRounds, quick}`.
- **returns:** `{rounds_run, locked_contracts, surfaced_bugs, surviving_mutants, no_mutation_audit, ready_to_commit, error}`. No interactive contract-review — contracts are surfaced non-blocking.

### `audit`

Source-audit (Phase 2): `Mechanical(audit-runner ×tools) ∥ Lenses(audit-<lens> ×selected)` →
`Refute(audit-refuter ×skeptics over the full finding set)` → `Synthesis`. Refutation is the
spine: LLM source-audits are false-positive-heavy, so survivors must pass a skeptic quorum;
mechanical + corroborated findings bypass it.

- **args:** `{auditScope, stack, lenses, mechanicalTools, repoRoot, skeptics}`. **Never a diff** - lenses Read the scope themselves.
- **returns:** `{confirmed_findings, refuted_findings, uncertain_findings, mechanical_findings, lens_coverage, refutation_summary, synthesis_status}`.

## Coverage modes - the `coverage-reviewer`'s three entry points

`coverage-reviewer` is the source of truth for "what should be tested." It runs as a **single
direct `Agent` dispatch** (never a stage) and writes a list of work units with **immutable**
`intended_behavior` strings.

### `diff` mode (retained-dormant - the characterization path)

Lock the current behavior of recent changes. Scope detection:

- `default_branch` = `git symbolic-ref refs/remotes/origin/HEAD` (stripped). Fallback: try `main`, `master`, `develop`.
- `merge_base` = `git merge-base HEAD origin/<default_branch>`; `diff` = `git diff <merge_base>...HEAD`.
- `untracked` = `git status --porcelain` entries starting with `??`.
- `scope` = union of files in `diff` + `untracked`.

The reviewer is handed the full diff text + changed-file list and infers intent from the change.
**No launcher ships for this today** - the retired `regression` skill was the only one. A
`characterize` skill could re-expose it.

### `target` mode (+ the report-bug ledger seed; the fix-mode seam)

Lock/cover the behavior of an explicit file, directory, or `crate::module` symbol. The reviewer
gets the resolved paths/symbols + any `CLAUDE.md` in or above them + nearby test files for
convention. Today it **infers** `intended_behavior` from docstrings and code intent.

**Optional ledger seed:** if `<repo>/.straitjacket/bugs.json` exists, the orchestrator may pass
`open` records whose `suspect_files` intersect the target scope; the reviewer can turn each
`intended_behavior_seed` into a work unit's `intended_behavior` (bridge fields
`suspect_files`→`target_file`, `suspect_symbol`→`target_symbol`).

**Fix mode (Phase 3) sharpens this seam:** for a known bug, the seed is **authoritative** - the
reviewer uses `intended_behavior_seed` verbatim as the locked contract and must **not** re-infer
current (buggy) behavior, or the test would lock the bug instead of the fix.

### `spec` mode (tdd)

Decompose a spec into work units that drive both test authoring and stub generation. No source
exists yet, so the reviewer also pre-assigns `target_stub_path` (where the `unimplemented!()` /
`NotImplementedException` stub will live so the test compiles-but-fails).

## Cardinal rules (shared by every skill)

1. **You are the single writer** of `<repo>/.straitjacket/<run_id>/work-units.json`. Subagents/stages return JSON; you merge.
2. **Subagent prompts must be self-contained.** Agents have no memory of prior runs; pass work-unit data + source paths inline.
3. **`intended_behavior` is immutable** once the `coverage-reviewer` writes it. Reject any output that rewrites it.
4. **Isolated specialists never see the diff** (or "what changed" framing, or transcripts). The `PreToolUse` hook scans adversarial prompts for `--- a/`, `+++ b/`, `git diff` (Agent path only). Defense-in-depth: never inline diff text yourself.
5. **Parallel spawns go in a single message** (Agent path) / one `parallel()` batch (workflow path).
6. **JSON parse failures:** retry once with a "return only valid JSON matching <schema>" prefix, then abort that unit.
7. **Surfaced-bug reflex — capture, don't pivot.** When a run surfaces a real defect you were
   **not** instructed to fix — in the *target* code **or** in straitjacket's *own* tooling
   (a wrong CLI verdict, a false-negative probe, a broken helper) — the safe next step is
   `straitjacket:report-bug`: write the ledger record, then **resume the task or stop**. Do
   **not** begin fixing it, and do **not** pivot to investigating or consulting-on a fix
   (fixing is its own separate instruction; absent it, the chain is *capture → continue*).
   Map what you know onto the BugRecord bridge fields (`suspect_files` / `suspect_symbol` /
   `intended_behavior_seed`) so a later `tdd` fix-mode / `triage` run can lift it without
   re-deriving it. This applies to **preflight too**: a STOP caused by a *suspected* straitjacket
   bug (e.g. a toolchain reported absent that you can see is installed) is captured, not just
   announced. The only skills that fix in-run are `tdd` and `triage`; every other skill that
   meets a bug it won't fix routes through this reflex.
8. **TDD-unverifiable → `straitjacket:audit`, not "live-run-guarded" alone.** When TDD is the
   active method (`tdd` / `triage` fix-mode) and a change genuinely **cannot** be driven RED→GREEN
   — it lands in non-unit-tested **orchestration** (`workflows/*.js`, `skills/*/SKILL.md`,
   `agents/*.md`, `hooks.json`) or otherwise has no test seam — do **not** stop at a hand-authored,
   live-run-guarded fix. Verify it with `straitjacket:audit` scoped to the changed file(s) (the LLM
   lenses read what no test can cover), then state the verification basis explicitly: *test-backed*
   vs. *audit-checked + live-run-guarded*. This covers only what the loop truly can't reach —
   **testable code still goes through the loop** (Cardinal Rule 0 / CLAUDE.md "fix testable bugs via
   the loop, not hand-patches"); never use this rule to dodge a test you could have written.

## Severity axes

Two deliberate severity scales coexist - this is **by design, not drift**:

- **Adversarial *test-validity* findings** use a 3-level scale: `low | medium | high`. A test is never "critical" - the axis measures *how badly a test fails to constrain behavior*.
- **Audit *defect-impact* + bug-record findings** use a 4-level scale: `critical | high | medium | low`. The axis is *real-world impact*; it maps 1:1 onto `BugRecord.severity` and drives the audit refuter count (higher severity → more skeptics).

## Run-state layout

All per-run artifacts live under `<repo>/.straitjacket/<run_id>/` (run_id =
`YYYYMMDDThhmmss-<4hex>`), gitignored via `.straitjacket/*/`: `work-units.json`, `tooling.json`,
`test-snapshot.json`, `state.json`, logs, `quarantine/`, `staged-tests/`, `audit-findings.json`
(the audit skill's transient findings - distinct from `work-units.json`). The **bug ledger** at
`<repo>/.straitjacket/bugs.json` is **tracked/committed** — a top-level file under `.straitjacket/`
intentionally outside the `*/` glob — the durable hand-off between `report-bug`, `audit`,
`triage`, and a later fix-mode run.
