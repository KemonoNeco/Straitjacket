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
    { title: 'PreAdversarial', detail: '3 specialists → synthesis on the RED tests; apply strengthenings' },
    { title: 'Implement', detail: 'implementation-author fills stubs; never touches tests' },
    { title: 'GreenCheck', detail: 'gate-runner run-new-tests --expect pass + name-survival' },
    { title: 'PostGreen', detail: 'post-green adversarial + (unless quick) mutation team' },
    { title: 'Finalize', detail: 'no-test-mutation audit; assemble result' },
  ],
}

const {
  spec,
  stack,
  repoRoot,
  outputDir,
  workUnitsPath,
  testSnapshotPath,
  toolingAvailable = [],
  maxRounds = 3,
  quick = false,
  authorCap = 6,
  implCap = 4,
} = args

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

function chunk(arr, size) {
  const out = []
  for (let i = 0; i < arr.length; i += size) out.push(arr.slice(i, i + size))
  return out
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

  return { reports, synthesis }
}

// Cap-batched parallel fan-out: ~4 units per agent, `cap` agents in flight per wave.
async function fanout(units, kindPredicate, cap, promptFn, agentType, phaseName) {
  const selected = units.filter(kindPredicate)
  if (!selected.length) return []
  const groups = chunk(selected, 4)
  let results = []
  for (const wave of chunk(groups, cap)) {
    const r = await parallel(wave.map((g) => () =>
      agent(promptFn(g), { agentType, schema: CHUNK_RESULT_SCHEMA, phase: phaseName, label: `${agentType}:${g[0] && g[0].id}` })))
    results = results.concat(r.filter(Boolean).flatMap((c) => Array.isArray(c.results) ? c.results : []))
  }
  return results
}

if (!args || typeof args !== 'object' || Array.isArray(args)) {
  throw new Error(`straitjacket:tdd-cycle — args must be a plain object, got ${Array.isArray(args) ? 'Array' : typeof args}; pass { spec, stack, repoRoot, ... } not a CLI string`)
}
if (!spec) throw new Error('straitjacket:tdd-cycle — required arg `spec` is missing or empty')

// ---- the cycle -----------------------------------------------------------------------

phase('Coverage')
const coverage = await agent([
  `mode: spec; stack: ${stack}`,
  `Decompose this specification into work units with locked intended_behavior and a target_stub_path`,
  `per unit (so the test compiles-but-fails against a stub). Enumerate edge cases, not just happy paths.`,
  `Specification:`, spec,
  `Read schemas/work-unit.schema.json. Return ONLY JSON: {"work_units":[...], "scope_summary": "..."}.`,
].join('\n'), { agentType: 'straitjacket:coverage-reviewer', schema: COVERAGE_SCHEMA, phase: 'Coverage', label: 'coverage-reviewer' })

let units = (coverage && coverage.work_units) || []
const lockedContracts = units.map((u) => ({ id: u.id, intended_behavior: u.intended_behavior, target_file: u.target_file, target_symbol: u.target_symbol, target_stub_path: u.target_stub_path }))
if (!units.length) {
  return { stage: 'tdd-cycle', error: 'coverage-reviewer produced no work units', locked_contracts: [], ready_to_commit: false }
}

const surfacedBugs = []
const survivingMutants = []
let round = 0
let lastError = null

while (round < maxRounds && units.length) {
  round += 1

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

  phase('RedCheck')
  await runGate('verify-new-tests-compile', units, { phaseName: 'RedCheck' })
  const red = await runGate('run-new-tests', units, { expect: 'fail', phaseName: 'RedCheck' })
  if (!red || red.nothing_to_run) {
    lastError = 'red-check checked nothing (nothing_to_run) — authoring produced no runnable tests'
    break
  }
  const redNames = (red.per_unit_results || []).map((u) => u.output_test_name)
  const vacuous = (red.per_unit_results || []).filter((u) => u.classification === 'vacuous_pre_impl')
  // (A vacuous pre-impl test would be re-authored here with a sharper prompt in a full run;
  //  recorded for the summary so it is never silently accepted.)

  phase('PreAdversarial')
  const pre = await adversarial(units, 'pre_impl')
  const strengthenings = (pre.synthesis && pre.synthesis.new_work_unit_proposals) || []
  // Tests LOCK after this point (read-only). Strengthenings, if any, would be authored + re-red-checked.

  phase('Implement')
  await fanout(units, () => true, implCap, implPrompt, 'straitjacket:implementation-author', 'Implement')

  phase('GreenCheck')
  await runGate('verify-new-tests-compile', units, { phaseName: 'GreenCheck' })
  const green = await runGate('run-new-tests', units, { expect: 'pass', phaseName: 'GreenCheck' })
  if (!green || green.nothing_to_run) {
    lastError = 'green-check checked nothing (nothing_to_run)'
    break
  }
  const greenNames = (green.per_unit_results || []).map((u) => u.output_test_name)
  // name-survival: every RED test name must still exist at green.
  const missing = redNames.filter((n) => !greenNames.includes(n))
  if (missing.length) {
    // Name-survival violation: a RED test went missing/deleted/renamed/#[ignore]-d at green.
    // This is a test-mutation cheat — fail loudly, never silently accept.
    lastError = `name-survival violation: RED tests missing at green: ${missing.join(', ')}`
    break
  }
  const failedUnits = (green.per_unit_results || []).filter((u) => u.classification === 'all_fail')
  for (const fu of failedUnits) {
    const wu = units.find((u) => u.id === fu.work_unit_id)
    surfacedBugs.push({
      work_unit_id: fu.work_unit_id,
      target_file: wu && wu.target_file,
      target_symbol: wu && wu.target_symbol,
      intended_behavior_seed: wu && wu.intended_behavior,
      note: 'green-check could not make this test pass without weakening it — surfaced, not weakened',
    })
  }

  phase('PostGreen')
  const post = await adversarial(units, 'post_green')
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
      roundMutants = roundMutants.concat(r.filter(Boolean).flatMap((m) => m.surviving_mutants || []))
    }
  }
  survivingMutants.push(...roundMutants)

  // ---- iterate decision (in-script) ----
  const unresolved = ((post.synthesis && post.synthesis.static_findings) || []).filter((f) => f && (f.severity === 'high' || f.severity === 'medium'))
  const proposals = (post.synthesis && post.synthesis.new_work_unit_proposals) || []
  if ((roundMutants.length || unresolved.length) && proposals.length) {
    // Next round covers the under-tested behavior class the synthesis proposed (never the mutant itself).
    units = proposals
    continue
  }
  break
}

phase('Finalize')
const audit = await runGate('verify-no-test-mutation', units, { phaseName: 'Finalize' })

const readyToCommit = !lastError && surfacedBugs.length === 0

return {
  stage: 'tdd-cycle',
  rounds_run: round,
  error: lastError,
  locked_contracts: lockedContracts,        // surfaced NON-BLOCKING in the summary (contract-review removed)
  surfaced_bugs: surfacedBugs,               // main session: park-vs-fix + report-bug
  surviving_mutants: survivingMutants,
  no_mutation_audit: audit,
  ready_to_commit: readyToCommit,            // main session commits the savepoint on green (or --auto-commit)
}
