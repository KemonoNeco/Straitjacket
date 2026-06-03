// tdd-cycle.js — the consolidated TDD workflow stage.
//
// Emitted by `straitjacket workflow-script tdd-cycle` and run via the Workflow tool's
// inline `script`. With the interactive contract-review removed, tdd has no human-input
// stop left — every gate (red / green / iterate) is an automated assertion — so the whole
// cycle runs as ONE resumable workflow instead of a skill re-invoking a stage per phase.
//
// The runtime has no shell/FS of its own, so:
//   - authoring/impl agents Write the test/stub/impl files themselves (they have Write/Edit);
//   - the red/green/compile gates run via the mechanical `gate-runner` agent, which writes
//     work-units.json from the array this script hands it and runs the straitjacket CLI.
//     Within this workflow the gate-runner is the single SEQUENTIAL writer of work-units.json
//     (the script serializes gate calls), preserving the spirit of Cardinal Rule 1.
//
// This script DUPLICATES the capped-parallel (fanout) + 3-specialist→synthesis (adversarial)
// choreography inline, because workflow scripts cannot import one another — the accepted cost
// of one resumable run (see docs/STAGES.md / the decomposition plan).
//
// The diff is NEVER an input. Specialists/authors Read the spec + current source themselves.
//
// Bindings via `args`:
//   spec                inline specification text
//   stack               "rust" | "csharp" | "both"
//   repoRoot            absolute repo root
//   outputDir           <repoRoot>/.straitjacket/<run_id>/  (logs + work-units.json)
//   workUnitsPath       outputDir + "/work-units.json"
//   testSnapshotPath    snapshot for the end-of-cycle no-mutation audit (optional)
//   toolingAvailable    string[]  (e.g. ["cargo-mutants"])
//   maxRounds           iteration cap (default 3)
//   quick               skip the post-green mutation team (default false)
//   authorCap           parallel author cap (default 6)
//   implCap             parallel implementation cap (default 4)

export const meta = {
  name: 'tdd-cycle',
  description: 'Consolidated test-first cycle: coverage planning → parallel test+stub authoring → red-check gate → pre-impl adversarial review → implementation → green-check gate → post-green adversarial + mutation, iterating to a cap. Gates run via the gate-runner agent and the script branches on their verdicts; no interactive contract-review (it is surfaced non-blocking instead). Returns locked contracts, surfaced bugs, and a ready_to_commit verdict for the main session to commit on green.',
  phases: [
    { title: 'Coverage', detail: 'coverage-reviewer (single) locks intended_behavior from the spec' },
    { title: 'Author', detail: 'parallel test+stub authoring (compiles, fails at runtime)' },
    { title: 'RedCheck', detail: 'gate-runner run-new-tests --expect fail; branch on verdict' },
    { title: 'PreAdversarial', detail: '3 specialists → synthesis on the RED tests; surface strengthenings (unapplied)' },
    { title: 'Implement', detail: 'implementation-author fills stubs; never touches tests' },
    { title: 'GreenCheck', detail: 'gate-runner run-new-tests --expect pass + name-survival' },
    { title: 'PostGreen', detail: 'post-green adversarial + (unless quick) mutation team' },
    { title: 'Finalize', detail: 'no-test-mutation audit; assemble result' },
  ],
}

// Normalize + validate `args` before consuming it (issues #54 + #58). The Workflow runtime can deliver
// `args` as a JSON STRING of a valid object (recurring gotcha — see the literal-binding workaround); the
// #36 plain-object guard would then hard-reject it and the stage runs 0 agents. Parse a string `args` and
// ADOPT it only when it is a plain object; otherwise keep the original. Then run the guard BEFORE the
// destructure (so a null/undefined `args` yields this actionable message, not a raw TypeError from
// destructuring null — issue #58) and before any cfg.X read below. A genuine non-object / CLI-string still
// fails loudly — this NARROWS #36, it does not remove it. Routed through a local `cfg`, NOT a reassignment
// of the injected `args` global (whose mutability is runtime-dependent).
let cfg = args
let _argParseErr = ''   // when args is a string that doesn't yield a plain object, carry the reason into the guard message
if (typeof cfg === 'string') {
  try {
    const _p = JSON.parse(cfg)
    if (_p && typeof _p === 'object' && !Array.isArray(_p)) {
      cfg = _p
    } else {
      _argParseErr = ` (parsed as ${_p === null ? 'null' : (Array.isArray(_p) ? 'Array' : typeof _p)} but expected a plain object)`
    }
  } catch (e) {
    _argParseErr = ` (looks like a string but is not parseable JSON: ${e && e.message})`
  }
}
if (!cfg || typeof cfg !== 'object' || Array.isArray(cfg)) {
  throw new Error(`straitjacket:tdd-cycle — args must be a plain object, got ${cfg === null ? 'null' : (Array.isArray(cfg) ? 'Array' : typeof cfg)}${_argParseErr}; pass { spec, stack, repoRoot, ... } not a CLI string`)
}
const {
  spec,
  mode = 'spec',          // 'spec' (greenfield) | 'target' (fix mode, seeded from a bug-ledger record)
  targetFile,             // target/fix mode: suspect_files  -> coverage-reviewer target_file
  targetSymbol,           // target/fix mode: suspect_symbol -> coverage-reviewer target_symbol
  intendedBehaviorSeed,   // target/fix mode: AUTHORITATIVE locked contract, passed VERBATIM (never re-inferred)
  stack,
  repoRoot,
  outputDir,
  workUnitsPath,
  testSnapshotPath,
  toolingAvailable = [],
  quick = false,
} = cfg

// Sanitize the numeric caps — do NOT destructure-default them (Gemini review on PR #50). A destructure
// default only fills `undefined`, so a null / 0 / negative / non-numeric `maxRounds` flows straight into
// `while (round < maxRounds && units.length)` below, where `round(0) < null` / `< "x"` is false on the
// FIRST check — the entire cycle is skipped and the run returns ready_to_commit:true with ZERO tests
// authored (a silent fail-open, the exact class this file hardens; also an instance of issue #49). Floor
// each at a positive integer. Mirrors audit.js's `skeptics` sanitization and the chunk() clamp (issue
// #31); authorCap/implCap are additionally clamped inside chunk(), but sanitizing at the source closes
// the maxRounds fail-open and keeps the contract honest.
const maxRounds = Math.max(1, parseInt(cfg.maxRounds, 10) || 3)
const authorCap = Math.max(1, parseInt(cfg.authorCap, 10) || 6)
const implCap = Math.max(1, parseInt(cfg.implCap, 10) || 4)

// ---- schemas (mirror the agent output contracts) -------------------------------------

const COVERAGE_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { work_units: { type: 'array' }, scope_summary: { type: 'string' } },
  required: ['work_units'],
}
const CHUNK_RESULT_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { results: { type: 'array' }, notes_to_orchestrator: { type: 'string' } },
}
const SPECIALIST_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { specialist: { type: 'string' }, static_findings: { type: 'array' }, new_work_unit_proposals: { type: 'array' }, isolation_check: { type: 'object' } },
  required: ['specialist', 'static_findings'],
}
const SYNTHESIS_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { synthesis_status: { type: 'string' }, static_findings: { type: 'array' }, new_work_unit_proposals: { type: 'array' }, mutation_runner_tasks: { type: 'array' } },
  required: ['static_findings'],
}
const MUTATION_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { target_path: { type: 'string' }, surviving_mutants: { type: 'array' } },
  required: ['surviving_mutants'],
}
const GATE_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { gate: { type: 'string' }, exit_code: { type: 'number' }, cli_result: {} },
  required: ['gate'],
}

// ---- helpers -------------------------------------------------------------------------

// chunk(arr, size): split into slices of `size`. `size` is clamped to a positive integer so a
// non-positive / NaN / stringified size can never spin forever (i += 0) or silently yield an
// empty fan-out (NaN < length === false). See issue #31; callers pass authorCap/implCap here.
function chunk(arr, size) {
  const n = Math.max(1, Math.floor(Number(size)) || 1)
  const out = []
  for (let i = 0; i < arr.length; i += n) out.push(arr.slice(i, i + n))
  return out
}

// groupByWriter(units): partition into groups where any two units sharing an output_file_path OR
// a target_stub_path land in the SAME group (connected components over the files each unit writes).
// With each group assigned to one agent and never split across a wave, every test file and stub
// source file has exactly one concurrent writer — the author/impl contract that the old
// by-array-index chunking silently violated (issue #18). A unit with no file key forms its own group.
function groupByWriter(units) {
  const parent = units.map((_u, i) => i)
  const find = (i) => { while (parent[i] !== i) { parent[i] = parent[parent[i]]; i = parent[i] } return i }
  const union = (a, b) => { const ra = find(a), rb = find(b); if (ra !== rb) parent[ra] = rb }
  const claimedBy = new Map() // file path -> first unit index that touched it
  units.forEach((u, i) => {
    for (const key of [u.output_file_path, u.target_stub_path]) {
      if (!key) continue
      if (claimedBy.has(key)) union(i, claimedBy.get(key)); else claimedBy.set(key, i)
    }
  })
  const groups = new Map() // root index -> unit[]
  units.forEach((u, i) => {
    const r = find(i)
    if (!groups.has(r)) groups.set(r, [])
    groups.get(r).push(u)
  })
  return [...groups.values()]
}

// packGroups(writerGroups, targetSize): greedily pack whole writer-groups into agent-chunks of
// ~targetSize units WITHOUT ever splitting a writer-group across chunks (so the single-writer
// guarantee from groupByWriter survives chunking). A group larger than targetSize becomes its own chunk.
function packGroups(writerGroups, targetSize) {
  const chunks = []
  let cur = []
  for (const g of writerGroups) {
    if (cur.length && cur.length + g.length > targetSize) { chunks.push(cur); cur = [] }
    cur = cur.concat(g)
  }
  if (cur.length) chunks.push(cur)
  return chunks
}

// compileFailure(verdict, label): inspect a verify-new-tests-compile verdict
// ({all_passed, per_unit_results:[{work_unit_id, output_file_path, passed, diagnostics_excerpt}]}).
// Returns a loud error string when the new tests/stubs did NOT compile, else null. Binding this is
// load-bearing (issue #21): a non-compiling tree yields no test ok/FAILED lines, so run-new-tests
// would classify every unit NeverFound (not RedOk) with nothing_to_run=false and the cycle would
// proceed on bogus data — this is the workflow path's ONLY compile gate (the PostToolUse hook does
// not fire for workflow-spawned agents).
function compileFailure(verdict, label) {
  // Fail CLOSED (issue #21 + #46): ONLY an affirmative all_passed === true clears this gate. There are
  // two distinct un-verified shapes and both must fail closed:
  //   (1) a null/missing verdict — the gate-runner produced no result at all;
  //   (2) a TRUTHY-but-shapeless verdict that omits the all_passed boolean (e.g. an LLM gate-runner
  //       returns {per_unit_results:[...]} but no all_passed). GATE_SCHEMA leaves cli_result entirely
  //       unconstrained, so such a verdict passes schema validation and reaches here verbatim.
  // The earlier guard reasoned only about case (1); a present-but-shapeless verdict (case 2) used to
  // fall through to `return null` and be misread as "compile passed". This is the workflow path's ONLY
  // compile gate (the PostToolUse hook does not fire for workflow-spawned agents), so a missing flag
  // must never be read as a pass into run-new-tests.
  if (!verdict) {
    return `${label}: compile gate produced no verdict (gate-runner returned nothing) — refusing to proceed on an unverified compile`
  }
  if (verdict.all_passed === false) {
    const failed = (verdict.per_unit_results || []).filter((u) => u && u.passed === false)
    const detail = failed.map((u) => `${u.output_file_path || u.work_unit_id}: ${u.diagnostics_excerpt || 'compile failed'}`).join(' | ')
    return `${label}: new tests/stubs did not compile — ${detail || 'see compile log'}`
  }
  if (verdict.all_passed !== true) {
    const got = verdict.all_passed === undefined ? 'a verdict missing the all_passed flag' : `all_passed=${JSON.stringify(verdict.all_passed)}`
    return `${label}: compile gate verdict did not affirm all_passed === true (${got}) — refusing to proceed on an unverified compile`
  }
  return null
}

// Run one CLI gate through the gate-runner agent; returns its parsed cli_result (or null).
async function runGate(gate, units, { expect, phaseName } = {}) {
  const res = await agent([
    `You are the gate-runner. Run the straitjacket gate and return its JSON verdict verbatim.`,
    `gate: ${gate}`,
    `repo_root: ${repoRoot}`,
    `work_units_path: ${workUnitsPath}`,
    `log_dir: ${outputDir}`,
    `stack: ${stack}`,
    expect ? `expect: ${expect}` : '',
    gate === 'verify-no-test-mutation' && testSnapshotPath ? `snapshot_file: ${testSnapshotPath}` : '',
    `work_units (write this to work_units_path verbatim, then run the command):`,
    JSON.stringify({ work_units: units }, null, 2),
  ].filter(Boolean).join('\n'), { agentType: 'straitjacket:gate-runner', schema: GATE_SCHEMA, phase: phaseName, label: gate })
  return (res && res.cli_result) || null
}

function authorPrompt(units) {
  return [
    `mode: tdd; stack: ${stack}`,
    `Author the tests AND minimal stubs for these work units. APPEND a test into each unit's`,
    `output_file_path; CREATE/EXTEND a stub at target_stub_path whose body is unimplemented!()`,
    `(Rust) / throw new NotImplementedException() (C#) — it must COMPILE and FAIL at runtime.`,
    `READ the spec context and any referenced source YOURSELF. Do NOT modify any other test file.`,
    `Do NOT rewrite intended_behavior. You own only the files named in your units.`,
    `Work units:`, JSON.stringify(units, null, 2),
    `Return ONLY JSON: {"results":[{work_unit_id,status,file_written,test_name,stub_written}]}.`,
  ].join('\n')
}

function implPrompt(units) {
  return [
    `stack: ${stack}`,
    `Replace the stub bodies so these failing tests pass. You may NOT modify any test, period —`,
    `if a test contradicts the locked intended_behavior, surface it in notes_to_orchestrator`,
    `instead of weakening it. Satisfy the CONTRACT, not just the literal assertion. READ the`,
    `failing tests + stubbed source YOURSELF; follow language idioms; compile/lint clean.`,
    `Work units (grouped by target_stub_path):`, JSON.stringify(units, null, 2),
    `Return ONLY JSON: {"results":[{work_unit_id,status,target_file,target_symbol,lines_changed}]}.`,
  ].join('\n')
}

function specialistPrompt(dim, units, mode) {
  return [
    `mode: ${mode}; stack: ${stack}`,
    `You are the adversarial-${dim} specialist. Operate in isolation.`,
    `Work units (locked intended_behavior + paths):`, JSON.stringify(units, null, 2),
    `READ the current source at each target_file and the test at each output_file_path YOURSELF.`,
    `You will NOT be given any diff or author transcript; operating from a diff is itself a misalignment.`,
    `Apply ONLY your ${dim} lens. Return ONLY JSON per your output contract, incl. isolation_check.`,
  ].join('\n')
}

// Run the 3-specialist → synthesis adversarial pass inline (mode: pre_impl | post_green).
const ADVERSARIAL_SPECIALISTS = 3
async function adversarial(units, mode) {
  const reports = (await parallel([
    () => agent(specialistPrompt('vacuousness', units, mode), { agentType: 'straitjacket:adversarial-vacuousness', schema: SPECIALIST_SCHEMA, phase: mode === 'pre_impl' ? 'PreAdversarial' : 'PostGreen', label: 'vacuousness' }),
    () => agent(specialistPrompt('happy-path', units, mode), { agentType: 'straitjacket:adversarial-happy-path', schema: SPECIALIST_SCHEMA, phase: mode === 'pre_impl' ? 'PreAdversarial' : 'PostGreen', label: 'happy-path' }),
    () => agent(specialistPrompt('misalignment', units, mode), { agentType: 'straitjacket:adversarial-misalignment', schema: SPECIALIST_SCHEMA, phase: mode === 'pre_impl' ? 'PreAdversarial' : 'PostGreen', label: 'misalignment' }),
  ])).filter(Boolean)

  const synthesis = await agent([
    `mode: ${mode}; stack: ${stack}`,
    `Synthesize these three adversarial specialist reports — dedupe, rank by severity. Do NOT re-read source.`,
    `tooling_available: ${JSON.stringify(toolingAvailable)}`,
    `work_units_locked: ${JSON.stringify(units)}`,
    `specialist_reports: ${JSON.stringify(reports)}`,
    mode === 'post_green'
      ? `POST-GREEN: emit mutation_runner_tasks for the surviving-mutant hunt + any new_work_unit_proposals.`
      : `PRE-implementation (no impl exists yet): emit ranked test additions/strengthenings as new_work_unit_proposals; leave mutation_runner_tasks empty.`,
    `Return ONLY JSON matching the adversarial-synthesis output contract.`,
  ].join('\n'), { agentType: 'straitjacket:adversarial-synthesis', schema: SYNTHESIS_SCHEMA, phase: mode === 'pre_impl' ? 'PreAdversarial' : 'PostGreen', label: 'synthesis' })

  // specialistsRun lets the caller detect a DROPPED specialist (issue #37): a null specialist return
  // is filtered out above, silently shrinking the review from 3 lenses to fewer. The caller records
  // any shortfall (and a null synthesis) as a DEGRADED quality phase that blocks a clean
  // ready_to_commit — a review that ran incomplete must not read as "the review passed".
  // (The `reports` array is used here to build `synthesis` + the run count; it is NOT returned —
  // callers only need `synthesis` + the counts, so returning it was dead surface (issue #38).)
  return { synthesis, specialistsRun: reports.length, specialistsExpected: ADVERSARIAL_SPECIALISTS }
}

// Cap-batched parallel fan-out: ~4 units per agent, `cap` agents in flight per wave.
async function fanout(units, kindPredicate, cap, promptFn, agentType, phaseName) {
  const selected = units.filter(kindPredicate)
  if (!selected.length) return []
  // Single-writer-per-file (issue #18): group units sharing an output_file_path/target_stub_path
  // BEFORE chunking and never split a writer-group across concurrent agents within a wave. The two
  // author fanout calls (unit-kind, then integration-kind) are awaited SEQUENTIALLY by the caller,
  // so a file shared across kinds is never written concurrently; within a call this grouping is the
  // concurrency guarantee. chunk() (the wave size, `cap`) is itself clamped so cap<=0/NaN can't loop.
  const groups = packGroups(groupByWriter(selected), 4)
  let results = []
  for (const wave of chunk(groups, cap)) {
    const r = await parallel(wave.map((g) => () =>
      agent(promptFn(g), { agentType, schema: CHUNK_RESULT_SCHEMA, phase: phaseName, label: `${agentType}:${g[0] && g[0].id}` })))
    results = results.concat(r.filter(Boolean).flatMap((c) => Array.isArray(c.results) ? c.results : []))
  }
  return results
}

// bail(error): an early-exit result carrying the COMPLETE shape the final return uses, so a caller
// can uniformly read result.surfaced_bugs / .degraded / .pre_impl_strengthenings on the refusal
// paths without a TypeError (all early exits are pre-loop, hence rounds_run:0). Addresses Gemini
// review on PR #48 (API consistency) -- keeps EVERY early return consistent instead of just one.
function bail(error) {
  return {
    stage: 'tdd-cycle', rounds_run: 0, error,
    degraded: [], locked_contracts: [], surfaced_bugs: [], pre_impl_strengthenings: [], dropped_impl: [],
    surviving_mutants: [], mutation_runners_failed: 0, no_mutation_audit: null, ready_to_commit: false,
  }
}

// (the args-shape guard now runs above, BEFORE the destructure, on the normalized `cfg` — issues #54 + #58.)
if (!spec) throw new Error('straitjacket:tdd-cycle — required arg `spec` is missing or empty')

// ---- the cycle -----------------------------------------------------------------------

phase('Coverage')
// Coverage runs in one of two modes (issue #14). SPEC mode (default) decomposes a greenfield
// spec. TARGET/fix mode (triage fix-mode seam #1) decomposes a fix for a KNOWN defect: the
// intended_behavior_seed is the AUTHORITATIVE contract for the CORRECT behavior — it is passed
// VERBATIM as each unit's locked intended_behavior and the reviewer must NOT re-infer it or
// characterize the current (buggy) behavior, or the test would lock the bug instead of the fix.
// Guard the seam (issue #14): target mode with an empty/degenerate seed would silently hand the
// reviewer an EMPTY authoritative contract → it would re-infer the buggy behavior (the very thing
// this seam exists to prevent). Fail loudly instead — mirroring the iterate-materialize guard.
if (mode === 'target' && (!intendedBehaviorSeed || String(intendedBehaviorSeed).trim().length < 10)) {
  return bail('target/fix mode requires a non-empty intendedBehaviorSeed (the authoritative locked contract); refusing to run coverage-reviewer without one — an empty seed would force it to re-infer the buggy behavior')
}
const coveragePromptLines = (mode === 'target')
  ? [
      `mode: target; stack: ${stack}`,
      `Fix mode. Decompose a fix for a KNOWN defect into work units that pin the CORRECT behavior,`,
      `each with a target_stub_path so the test compiles-but-fails. Enumerate edge cases.`,
      `target_file: ${targetFile || ''}`,
      `target_symbol: ${targetSymbol || ''}`,
      `intended_behavior (AUTHORITATIVE — use VERBATIM as each unit's locked intended_behavior; do`,
      ` NOT re-infer it and do NOT characterize the current buggy behavior):`,
      intendedBehaviorSeed || '',
      `Context (NOT the contract — the seed above is the contract):`, spec || '(none)',
      `Read schemas/work-unit.schema.json. Return ONLY JSON: {"work_units":[...], "scope_summary": "..."}.`,
    ]
  : [
      `mode: spec; stack: ${stack}`,
      `Decompose this specification into work units with locked intended_behavior and a target_stub_path`,
      `per unit (so the test compiles-but-fails against a stub). Enumerate edge cases, not just happy paths.`,
      `Specification:`, spec,
      `Read schemas/work-unit.schema.json. Return ONLY JSON: {"work_units":[...], "scope_summary": "..."}.`,
    ]
const coverage = await agent(coveragePromptLines.join('\n'), { agentType: 'straitjacket:coverage-reviewer', schema: COVERAGE_SCHEMA, phase: 'Coverage', label: 'coverage-reviewer' })
// Distinguish a NULL agent return (the coverage-reviewer produced nothing after its retry budget)
// from a successfully-empty plan (issue #37): both currently collapse to units=[] and the generic
// "produced no work units" error, but only the latter is a real coverage verdict. Fail loudly and
// specifically when the agent itself returned nothing rather than implying it ran and found none.
if (!coverage) {
  return bail('coverage-reviewer returned nothing (agent produced no result after its retry budget) — refusing to proceed without a coverage plan')
}

let units = (coverage && coverage.work_units) || []
// Guard a null / non-object work_units element (issue #37 robustness; Gemini review on PR #48).
// COVERAGE_SCHEMA constrains work_units to an array but NOT its items, so an LLM emitting a null or
// garbage element would crash at lockedContracts (u.id on null) -- the FIRST per-element access,
// which is why guarding only the later badKind/uncollectable filters is incomplete. Fail at source.
if (units.some((u) => !u || typeof u !== 'object')) {
  return bail('coverage-reviewer emitted a null or non-object work unit -- refusing to proceed on a malformed coverage plan')
}
if (!units.length) {
  return bail('coverage-reviewer produced no work units')
}

const surfacedBugs = []
const survivingMutants = []
const degraded = []                  // issue #37: non-fatal agent-phase failures (incomplete adversarial review /
                                     // null synthesis). Green is REAL but a quality phase ran incomplete — so this
                                     // blocks a clean ready_to_commit (the --auto-commit path acts on it with no
                                     // human in the loop). Distinct from lastError, which is for false-green-capable
                                     // (untested-behavior-reaching-commit) failures that break the loop.
const preImplStrengthenings = []     // issue #38: pre-impl adversarial proposals, SURFACED (not authored).
const droppedImpl = []               // issue #47: units an implementation-author chunk produced NO result for
                                     // (null after its retry budget). DIAGNOSTIC-ONLY + NON-gating: GreenCheck
                                     // already fails closed on an unimplemented stub (its test runs and fails),
                                     // so this only attributes WHICH unit/stub dropped — it never writes status
                                     // (load-bearing for gate collection) nor blocks ready_to_commit.
let mutationRunnersFailed = 0        // issue #37: dropped mutation-runner agents — NON-gating (mutation is advisory
                                     // + --quick-skippable); counted/surfaced but never blocks ready_to_commit.
// allUnitsById (issue #51/#52): the single source of truth for the CUMULATIVE authored-and-gated work
// units across ALL rounds, keyed by id. The PASS-side gates (GreenCheck + its compile gate, name-survival,
// the notPassing scan, the Finalize no-mutation audit), readyToCommit, AND the returned locked_contracts all
// derive from this — so a later round's implementation regressing an EARLIER round's previously-green test is
// re-run and caught (the false green #51 fixed). The RED side stays on `units` (this round's NEW units only):
// feeding an already-green prior-round unit into RedCheck (expect:fail) would PASS against no stub and trip
// the vacuous-pre-impl guard. round-N ids (`r{N}-p{i}`) never collide with round-1, so a re-proposed unit is
// updated in place rather than double-counted.
const allUnitsById = new Map()
// Dedup the GreenCheck notPassing scan across rounds (issue surfaced by the PR-1 rule-8 audit): the scan
// now iterates the CUMULATIVE allUnits every round, so a persistently-regressed earlier-round unit would
// be pushed to surfacedBugs once per subsequent round. Surface each non-passing work_unit_id at most once.
const surfacedNotPassingIds = new Set()
let round = 0
let lastError = null

while (round < maxRounds && units.length) {
  round += 1

  // Every unit must be routable to an author team. The two fanout selectors below match
  // kind==='unit' / kind==='integration' with NO catch-all, so a unit with an absent or unrecognized
  // kind would be selected by NEITHER team — never authored, never gated, left status:'pending'
  // (issue #39). COVERAGE_SCHEMA does not constrain `kind` and coverage-reviewer is an LLM, so a
  // missing/other kind is reachable. (Since #51, lockedContracts derives from the post-gate
  // accumulator, so such a unit no longer rides out as a contract — but it must still fail loudly here.)
  // Fail the round loudly here rather than silently drop the unit. (Iterate-materialize already
  // coerces kind to unit|integration, so in practice this guards round-1 coverage-reviewer output.)
  const badKind = units.filter((u) => u.kind !== 'unit' && u.kind !== 'integration')
  if (badKind.length) {
    lastError = `coverage produced work unit(s) with an unrecognized kind (must be 'unit' or 'integration'): ${badKind.map((u) => `${u.id || '?'}:${u.kind === undefined ? 'undefined' : u.kind}`).join(', ')} — refusing to author a partial batch that would leave them silently uncovered`
    break
  }

  phase('Author')
  const unitResults = await fanout(units, (u) => u.kind === 'unit', authorCap, authorPrompt, 'straitjacket:unit-test-author', 'Author')
  const integrationResults = await fanout(units, (u) => u.kind === 'integration', authorCap, authorPrompt, 'straitjacket:integration-test-author', 'Author')
  // Reconcile the authors' results back into `units` BEFORE RedCheck re-materializes work-units.json:
  // run-new-tests collects ONLY status=='written' units (run_new_tests.rs:166-169), and coverage-reviewer
  // hands them in as status:'pending', so without this every red-check would see nothing_to_run on round 1.
  // Match by work_unit_id and propagate the author-REPORTED status (not a hardcoded 'written') — a unit an
  // author did not actually write stays uncollected, keeping a total author failure loud rather than masked.
  const authoredStatusById = new Map()
  for (const r of [...unitResults, ...integrationResults]) {
    if (r && r.work_unit_id && r.status) authoredStatusById.set(r.work_unit_id, r.status)
  }
  units = units.map((u) => authoredStatusById.has(u.id) ? { ...u, status: authoredStatusById.get(u.id) } : u)
  // Fail CLOSED on partial author failure (issue #37). The reconcile above only updates the ids an
  // author actually REPORTED, so a unit whose author chunk returned null (the agent exhausted its
  // retries) — or that an author reported with a non-'written' status — keeps coverage's 'pending'.
  // run-new-tests collects ONLY status=='written' (run_new_tests.rs:167-169), so an uncollectable unit
  // is silently SKIPPED while its 'written' siblings keep nothing_to_run=false: the RedCheck guard
  // never fires and the cycle can reach ready_to_commit on a never-authored contract (false green).
  // With #39 guaranteeing every unit is unit|integration (so every unit IS selected by a fanout team),
  // every unit must end 'written'; any that didn't means an author phase silently dropped work.
  const uncollectable = units.filter((u) => u.status !== 'written')
  if (uncollectable.length) {
    lastError = `author phase left unit(s) uncollectable (status != 'written', so run-new-tests would silently skip them while written siblings ride to a false green): ${uncollectable.map((u) => `${u.id}:${u.status || 'unset'}`).join(', ')} — an author agent likely returned null after its retry budget`
    break
  }
  // Fold this round's authored-and-gated units into the cumulative set (issue #51/#52). Only units that
  // passed the uncollectable gate above (all status:'written') reach here, so allUnitsById holds exactly
  // the units that were genuinely authored. `allUnits` is the PASS-side gate set for THIS round; the RED
  // gates below stay on `units` (this round's new units only).
  for (const u of units) allUnitsById.set(u.id, u)
  const allUnits = [...allUnitsById.values()]

  phase('RedCheck')
  // Bind + branch on the compile verdict BEFORE run-new-tests (issue #21): a non-compiling tree
  // would otherwise be misread as NeverFound/regression downstream and the cycle would proceed.
  const redCompileErr = compileFailure(await runGate('verify-new-tests-compile', units, { phaseName: 'RedCheck' }), 'RedCheck compile')
  if (redCompileErr) { lastError = redCompileErr; break }
  const red = await runGate('run-new-tests', units, { expect: 'fail', phaseName: 'RedCheck' })
  if (!red || red.nothing_to_run) {
    lastError = 'red-check checked nothing (nothing_to_run) — authoring produced no runnable tests'
    break
  }
  // A vacuous pre-impl test PASSED against the unimplemented stub — it asserts nothing real and
  // must never be silently accepted (issue #20): surface each and fail this round loudly. (The
  // engine already classifies these 'rejected_lint'; only the JS used to compute-then-drop them.)
  const vacuous = (red.per_unit_results || []).filter(Boolean).filter((u) => u.classification === 'vacuous_pre_impl')
  if (vacuous.length) {
    for (const vu of vacuous) {
      const wu = units.find((u) => u.id === vu.work_unit_id)
      surfacedBugs.push({
        work_unit_id: vu.work_unit_id,
        target_file: wu && wu.target_file,
        target_symbol: wu && wu.target_symbol,
        intended_behavior_seed: wu && wu.intended_behavior,
        note: 'vacuous pre-impl: test passed against the stub (asserts nothing) — re-author with a sharper assertion',
      })
    }
    lastError = `vacuous pre-impl test(s) asserted nothing (passed against the stub): ${vacuous.map((u) => u.work_unit_id).join(', ')}`
    break
  }

  phase('PreAdversarial')
  const pre = await adversarial(units, 'pre_impl')
  // Record an INCOMPLETE pre-impl review as degraded (issue #37): a dropped specialist or a null
  // synthesis means fewer than 3 adversarial lenses actually ran. The tests are red-confirmed and will
  // be green-gated regardless, so this is NOT false-green-capable (not lastError) — but a review that
  // ran incomplete must not read as a clean pass, so it blocks ready_to_commit below.
  if (pre.specialistsRun < pre.specialistsExpected) degraded.push(`pre-impl adversarial review incomplete: only ${pre.specialistsRun}/${pre.specialistsExpected} specialists returned (a dropped specialist = a lost lens)`)
  if (!pre.synthesis) degraded.push('pre-impl adversarial synthesis returned nothing — the review could not be consolidated')
  // Tests LOCK after this point (read-only). The pre-impl pass SURFACES its proposed strengthenings in
  // the cycle result rather than authoring them (issue #38): authoring + re-red-checking a new contract
  // before lock is a separate feature with its own verification burden. The OLD behavior computed
  // `strengthenings` then never read it, while meta.phases claimed the phase "applies" them — that
  // silent discard is the defect. The main session now sees them as unapplied and can lift any into a
  // follow-up run. A proposal EXISTING is not a failure and does NOT gate ready_to_commit.
  preImplStrengthenings.push(...((pre.synthesis && pre.synthesis.new_work_unit_proposals) || []).map((p) => ({ ...p, round })))

  phase('Implement')
  // Capture the impl fanout result and attribute any DROPPED implementation-author (issue #47): a chunk
  // that returned null after its retry budget contributes no per-unit result, so a unit left
  // unimplemented was invisible here and surfaced only as a generic GreenCheck classification with no
  // link back to which work_unit / target_stub_path dropped. This is DIAGNOSTIC-ONLY and mirrors the
  // adversarial()->degraded attribution pattern, NOT the author-phase reconcile (whose status-write is
  // load-bearing for gate collection): we do NOT write status (the unimplemented stub must stay
  // status:'written' so its test still runs and fails CLOSED at green) and we add NO commit gate — the
  // dropped units are recorded for attribution only, never folded into readyToCommit.
  const implResults = await fanout(units, () => true, implCap, implPrompt, 'straitjacket:implementation-author', 'Implement')
  const implementedIds = new Set(implResults.map((r) => r && r.work_unit_id).filter(Boolean))
  for (const u of units.filter((u) => !implementedIds.has(u.id))) {
    droppedImpl.push({ work_unit_id: u.id, target_stub_path: u.target_stub_path, target_file: u.target_file, round })
  }

  phase('GreenCheck')
  // GreenCheck + its compile gate run over the CUMULATIVE allUnits (issue #51), not just this round's new
  // units, so a round-N implementation that regressed an earlier round's test is re-run and surfaces below
  // (its classification goes non-all_pass → notPassing → surfacedBugs → blocks ready_to_commit). The Rust
  // gate classifies ONLY the units it is handed (run_new_tests.rs), so the earlier-round test must be IN the
  // handed set to be caught — that is precisely why the pass side carries allUnits while the red side carries
  // `units`.
  const greenCompileErr = compileFailure(await runGate('verify-new-tests-compile', allUnits, { phaseName: 'GreenCheck' }), 'GreenCheck compile')
  if (greenCompileErr) { lastError = greenCompileErr; break }
  const green = await runGate('run-new-tests', allUnits, { expect: 'pass', phaseName: 'GreenCheck' })
  if (!green || green.nothing_to_run) {
    lastError = 'green-check checked nothing (nothing_to_run)'
    break
  }
  // Name-survival (issue #28): join red and green per_unit_results on the STABLE work_unit_id —
  // NOT the declared output_test_name, which is echoed from input and identical at both gates (so
  // the old name compare was vacuous, `missing` always []). Every test that RAN at red must still
  // exist at green; one that is gone/non-executing (never_found) is a test-mutation cheat
  // (deleted/renamed/#[ignore]-d) and fails loudly.
  // redRan is THIS round's new units (the red gate ran `units`); greenByUnit is the cumulative allUnits
  // (the green gate ran allUnits). Survival therefore only asserts this round's tests still exist at green —
  // a regression of an EARLIER round's test is not a survival concern (that test never ran at this round's
  // red) but is caught by the notPassing scan below, which iterates the cumulative green per_unit_results.
  const greenByUnit = new Map((green.per_unit_results || []).filter(Boolean).map((u) => [u.work_unit_id, u]))
  const redRan = (red.per_unit_results || []).filter(Boolean).filter((u) => u.classification && u.classification !== 'never_found')
  const survivalViolations = redRan
    .filter((ru) => { const gu = greenByUnit.get(ru.work_unit_id); return !gu || gu.classification === 'never_found' })
    .map((ru) => ru.work_unit_id)
  if (survivalViolations.length) {
    lastError = `name-survival violation: RED tests missing/non-executing at green (by work_unit_id): ${survivalViolations.join(', ')}`
    break
  }
  // ready_to_commit must never be true with a green unit that did not CLEANLY pass (issue #29):
  // surface EVERY non-all_pass classification (all_fail / flaky / never_found), not just all_fail.
  const notPassing = (green.per_unit_results || []).filter(Boolean).filter((u) => u.classification !== 'all_pass')
  for (const fu of notPassing) {
    if (surfacedNotPassingIds.has(fu.work_unit_id)) continue  // already surfaced in an earlier round (cumulative scan)
    surfacedNotPassingIds.add(fu.work_unit_id)
    const wu = allUnitsById.get(fu.work_unit_id)  // cumulative set: a regressed EARLIER-round unit is not in this round's `units`
    surfacedBugs.push({
      work_unit_id: fu.work_unit_id,
      target_file: wu && wu.target_file,
      target_symbol: wu && wu.target_symbol,
      intended_behavior_seed: wu && wu.intended_behavior,
      note: `green-check classified this unit '${fu.classification}', not all_pass — surfaced, not weakened`,
    })
  }

  phase('PostGreen')
  const post = await adversarial(units, 'post_green')
  // A failed post-green review must not read as "clean" (issue #37): a dropped specialist or a null
  // synthesis means the post-green adversarial pass ran incomplete. Green is REAL (every unit passed
  // the green gate above), so this is degraded, not lastError — but it blocks a clean ready_to_commit.
  if (post.specialistsRun < post.specialistsExpected) degraded.push(`post-green adversarial review incomplete: only ${post.specialistsRun}/${post.specialistsExpected} specialists returned`)
  if (!post.synthesis) degraded.push('post-green adversarial synthesis returned nothing — cannot confirm the post-green review found no further weaknesses')
  let roundMutants = []
  const tasks = (post.synthesis && post.synthesis.mutation_runner_tasks) || []
  const canMutate = !quick && (toolingAvailable.includes('cargo-mutants') || toolingAvailable.includes('dotnet-stryker') || toolingAvailable.includes('stryker'))
  if (canMutate && tasks.length) {
    for (const wave of chunk(tasks, 3)) {
      const r = await parallel(wave.map((t) => () =>
        agent([
          `Run mutation testing for ${t.target_path || t.target_file} (scope: ${t.scope || 'file'}, stack: ${stack}).`,
          `repo_root: ${repoRoot}`, `Return surviving mutants as JSON.`,
        ].join('\n'), { agentType: 'straitjacket:mutation-runner', schema: MUTATION_SCHEMA, phase: 'PostGreen', label: `mutation:${t.target_path || t.target_file}` })))
      // A dropped mutation-runner (null after retries) undercounts surviving mutants. This is
      // explicitly NON-gating (issue #37): mutation is advisory + --quick-skippable, so a failed runner
      // is COUNTED and surfaced but never blocks ready_to_commit (unlike the adversarial review above) —
      // folding a non-gating concern into the commit gate would be its own no-silent-green inversion.
      const returned = r.filter(Boolean)
      mutationRunnersFailed += wave.length - returned.length
      roundMutants = roundMutants.concat(returned.flatMap((m) => m.surviving_mutants || []))
    }
  }
  survivingMutants.push(...roundMutants)

  // ---- iterate decision (in-script) ----
  const unresolved = ((post.synthesis && post.synthesis.static_findings) || []).filter((f) => f && (f.severity === 'high' || f.severity === 'medium'))
  const proposals = (post.synthesis && post.synthesis.new_work_unit_proposals) || []
  // Another GATED round runs ONLY if one is wanted AND a round remains in the cap (issue #45). On the
  // final allowed round (round === maxRounds) the old code still hit `units = materialized; continue`,
  // but the loop guard `round < maxRounds` was then false, so it EXITED without ever authoring/gating
  // the materialized units — and the unresolved findings that wanted the round lived only in the
  // loop-local `unresolved` (recorded in no gating variable), so readyToCommit could read true over open
  // findings. Gate the iterate on a remaining round; the cap-exhausted AND the no-proposals cases fall
  // through to the unresolved-findings guard below so neither silently drops open findings.
  const wantsAnotherRound = !!((roundMutants.length || unresolved.length) && proposals.length)
  if (wantsAnotherRound && round < maxRounds) {
    // Materialize each synthesis proposal into a SCHEMA-COMPLETE WorkUnit before the next round
    // (issue #19): a proposal carries only target_file/target_symbol/kind/intended_behavior, but the
    // gates collect ONLY status=='written' units and reconcile by id — so feeding raw proposals
    // (no id, no output_file_path, no status) made round 2+ collect nothing and break. This mirrors
    // the round-1 materialization coverage-reviewer output receives; FAIL LOUDLY rather than feed a
    // partial proposal forward (the fix-mode seed permits a loud fail over a silent mis-run).
    const materialized = []
    for (let i = 0; i < proposals.length; i += 1) {
      const p = proposals[i] || {}
      if (!p.target_file || !p.intended_behavior || String(p.intended_behavior).length < 10) {
        lastError = `iterate: synthesis proposal #${i} lacks a usable target_file/intended_behavior — refusing to feed a partial proposal into round ${round + 1}`
        break
      }
      materialized.push({
        id: `r${round + 1}-p${i}`,
        target_file: p.target_file,
        target_symbol: p.target_symbol || p.target_file,
        kind: p.kind === 'integration' ? 'integration' : 'unit',
        intended_behavior: p.intended_behavior,
        preconditions: p.preconditions || '',
        inputs: p.inputs || '',
        expected: p.expected || '',
        fuzzable: !!p.fuzzable,
        output_file_path: p.output_file_path || p.target_stub_path || p.target_file,
        output_test_name: p.output_test_name || `test_r${round + 1}_p${i}`,
        target_stub_path: p.target_stub_path || p.target_file,
        status: 'pending',
        round: round + 1,
        source_of_unit: 'adversarial_reviewer',
      })
    }
    if (lastError) break
    // Next round covers the under-tested behavior class the synthesis proposed (never the mutant itself).
    units = materialized
    continue
  }
  // Not iterating further this run (issue #45): either the maxRounds cap is exhausted, or there are no
  // materializable proposals to carry findings forward. If unresolved high/medium findings remain they
  // were the reason another round was wanted and will now NEVER be re-authored/re-gated — record them as
  // a blocking degraded reason so ready_to_commit fails closed instead of reporting commit-ready over
  // open findings. (degraded, not lastError: the green that ran IS real; this is the "a quality concern
  // is unresolved at the cap" tier. Surviving mutants alone do NOT block — mutation is advisory/non-gating
  // per #37 and is already surfaced in the return — so this gates only on unresolved static findings.)
  if (unresolved.length) {
    const why = !proposals.length
      ? 'no materializable proposals to carry them forward'
      : `the maxRounds cap (${maxRounds}) is exhausted`
    const titles = unresolved.map((f) => f.title || f.summary || '(untitled finding)').slice(0, 8).join('; ')
    degraded.push(`post-green left ${unresolved.length} unresolved high/medium finding(s) that will not be re-authored/re-gated — ${why}: ${titles}${unresolved.length > 8 ? ' …' : ''}`)
  }
  break
}

phase('Finalize')
// locked_contracts (issue #52) and the Finalize no-mutation audit both derive from the CUMULATIVE
// authored-and-gated set, not the loop-local `units` (which on an iterated run holds only the LAST
// round's new units). finalUnits is the same accumulator the per-round allUnits was built from, read
// once after the loop. (On an early break before any author phase, allUnitsById is empty and lastError
// is set — readyToCommit is false regardless, so an empty finalUnits on the error path is benign.)
const finalUnits = [...allUnitsById.values()]
const lockedContracts = finalUnits.map((u) => ({ id: u.id, intended_behavior: u.intended_behavior, target_file: u.target_file, target_symbol: u.target_symbol, target_stub_path: u.target_stub_path }))

// The no-test-mutation gate diffs against a snapshot. When testSnapshotPath is unset (e.g. the
// triage fix-mode caller), the CLI's required --snapshot-file is absent and the gate would
// clap-exit-2; skip it and mark it not-run rather than invoke a degenerate audit (issue #22).
const audit = testSnapshotPath
  ? await runGate('verify-no-test-mutation', finalUnits, { phaseName: 'Finalize' })
  : { skipped: true }

// Fail CLOSED on a requested-but-DROPPED no-mutation audit (issue #53). runGate returns null when the
// gate-runner produced nothing after its retry budget; with testSnapshotPath SET, that null means the
// audit was REQUESTED but never ran — it must NOT read as clean (the exact inverse of compileFailure's
// fail-closed-on-null, issue #46, which the OLD `!audit` clause here violated by swallowing the null as a
// pass). Record it as a degraded quality phase (degraded gates readyToCommit below); green is real, the
// audit just didn't run, so it is degraded rather than a hard lastError.
if (testSnapshotPath && !audit) {
  degraded.push('no-test-mutation audit was requested (testSnapshotPath set) but the gate-runner returned nothing — cannot confirm the tests were not mutated; refusing to read a dropped audit as clean')
}
// auditClean (issue #30 + #53): clean ONLY on an affirmative verdict — a real `clean === true` OR the
// intentional skip (`skipped === true`, no snapshot). EVERYTHING else fails closed: `clean === false`
// (flagged mutation), a null-when-REQUESTED audit (dropped gate-runner — also recorded as degraded above),
// AND a truthy-but-malformed verdict missing the `clean` field (`{}` -> `clean` undefined). The earlier
// `audit ? audit.clean !== false : false` form still failed OPEN on that last case (`undefined !== false`
// is `true`); requiring an affirmative `=== true`/`skipped` is the strict fail-closed the #53/#46 hardening
// intends (Gemini review on PR #57). This is the exact inverse of compileFailure's affirmative-only gate.
const auditClean = !!audit && (audit.clean === true || audit.skipped === true)
// ready_to_commit blocks on `degraded` too (issue #37): the --auto-commit path acts on this with NO
// human in the loop, so an incomplete adversarial review / null synthesis must fail closed. (Fatal
// false-green-capable failures already set lastError and broke the loop; `degraded` is the "green is
// real but a quality phase ran incomplete" tier.) Mutation-runner failures are deliberately EXCLUDED
// (non-gating), and pre_impl_strengthenings existing is NOT a failure — neither gates the commit.
const readyToCommit = !lastError && surfacedBugs.length === 0 && auditClean && degraded.length === 0

return {
  stage: 'tdd-cycle',
  rounds_run: round,
  error: lastError,
  degraded,                                  // issue #37: incomplete quality phases (strings) — blocks ready_to_commit
  locked_contracts: lockedContracts,         // surfaced NON-BLOCKING in the summary (contract-review removed)
  surfaced_bugs: surfacedBugs,               // main session: park-vs-fix + report-bug
  pre_impl_strengthenings: preImplStrengthenings,  // issue #38: proposed test strengthenings, SURFACED (not authored)
  dropped_impl: droppedImpl,                 // issue #47: units a dropped implementation-author left unimplemented (diagnostic, NON-gating)
  surviving_mutants: survivingMutants,
  mutation_runners_failed: mutationRunnersFailed,  // issue #37: dropped mutation-runner agents (NON-gating, advisory)
  no_mutation_audit: audit,
  ready_to_commit: readyToCommit,            // main session commits the savepoint on green (or --auto-commit)
}
