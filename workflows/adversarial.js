// adversarial.js — shared adversarial-validation workflow stage.
//
// Emitted by `straightjacket workflow-script adversarial` (include_str!'d into the
// binary) and run via the Workflow tool's inline `script`. Used by:
//   - straightjacket Phase 4a (lock existing behavior),
//   - tdd stage C  (pre-impl validity review, on the RED tests),
//   - tdd stage E  (post-green passing-reason review + mutation).
//
// SPIKE wf_060d27f3 confirmed: workflow agents honor frontmatter `tools:` (the trio
// runs Read/Grep/Glob only — diff-blind, Rule 4 safe) but CANNOT spawn sub-agents.
// So the fan-out is script-level here (synthesis is the converging node, not a spawner).
//
// Bindings come from the skill via `args` (the main session injects them and merges the
// result into work-units.json — Cardinal Rule 1). The diff is NEVER an input binding;
// specialists Read the current source + tests themselves.
//   args.workUnits        locked units: {id, intended_behavior, target_file, target_symbol,
//                                        output_file_path, output_test_name}
//   args.stack            "rust" | "csharp" | "both"
//   args.mode             "pre_impl" | "post_green" | "lock"
//   args.toolingAvailable string[]  (e.g. ["cargo-mutants"])
//   args.repoRoot         absolute repo root (for the mutation runners, which keep Bash)

export const meta = {
  name: 'adversarial-validation',
  description: 'Adversarial test-validity review: three isolated specialists (vacuousness / happy-path / misalignment) fan out in parallel, then adversarial-synthesis dedupes + ranks; post-green also runs a mutation-runner team. Shared by straightjacket Phase 4a and tdd stages C (pre-impl, on red tests) + E (post-green).',
  phases: [
    { title: 'Specialists', detail: '3 isolated adversarial specialists in parallel (no diff in scope; they Read source themselves)' },
    { title: 'Synthesis', detail: 'adversarial-synthesis dedupes/ranks the three reports' },
    { title: 'Mutation', detail: 'post_green only: mutation-runner team on synthesis tasks (cap 3)' },
  ],
}

const { workUnits, stack, mode, toolingAvailable = [], repoRoot } = args

const SPECIALIST_SCHEMA = {
  type: 'object',
  additionalProperties: true,
  properties: {
    specialist: { type: 'string' },
    static_findings: { type: 'array' },
    new_work_unit_proposals: { type: 'array' },
    isolation_check: { type: 'object' },
  },
  required: ['specialist', 'static_findings', 'isolation_check'],
}

const SYNTHESIS_SCHEMA = {
  type: 'object',
  additionalProperties: true,
  properties: {
    synthesis_status: { type: 'string' },
    static_findings: { type: 'array' },
    new_work_unit_proposals: { type: 'array' },
    mutation_runner_tasks: { type: 'array' },
    isolation_check: { type: 'object' },
  },
  required: ['static_findings'],
}

const MUTATION_SCHEMA = {
  type: 'object',
  additionalProperties: true,
  properties: {
    target_path: { type: 'string' },
    surviving_mutants: { type: 'array' },
  },
  required: ['surviving_mutants'],
}

function specialistPrompt(dim) {
  return [
    `mode: ${mode}; stack: ${stack}`,
    `You are the adversarial-${dim} specialist. Operate in isolation.`,
    `Work units (locked intended_behavior + paths):`,
    JSON.stringify(workUnits, null, 2),
    `READ the current source at each target_file and the test at each output_file_path YOURSELF (you have Read/Grep/Glob).`,
    `You will NOT be given any diff or author transcript, and you must not request one — operating from the diff is itself a misalignment.`,
    `Apply ONLY your ${dim} lens; do not drift into the other specialists' lanes.`,
    `Return ONLY JSON per your output contract, including an isolation_check.`,
  ].join('\n')
}

phase('Specialists')
const reports = (await parallel([
  () => agent(specialistPrompt('vacuousness'), { agentType: 'straightjacket:adversarial-vacuousness', schema: SPECIALIST_SCHEMA, phase: 'Specialists', label: 'vacuousness' }),
  () => agent(specialistPrompt('happy-path'), { agentType: 'straightjacket:adversarial-happy-path', schema: SPECIALIST_SCHEMA, phase: 'Specialists', label: 'happy-path' }),
  () => agent(specialistPrompt('misalignment'), { agentType: 'straightjacket:adversarial-misalignment', schema: SPECIALIST_SCHEMA, phase: 'Specialists', label: 'misalignment' }),
])).filter(Boolean)

phase('Synthesis')
const synthesis = await agent([
  `mode: ${mode}; stack: ${stack}`,
  `Synthesize these three adversarial specialist reports — dedupe overlapping findings, rank by severity. Do NOT re-read source or tests.`,
  `tooling_available: ${JSON.stringify(toolingAvailable)}`,
  `work_units_locked: ${JSON.stringify(workUnits)}`,
  `specialist_reports: ${JSON.stringify(reports)}`,
  mode === 'post_green'
    ? `This is the POST-GREEN round: emit mutation_runner_tasks for the surviving-mutant hunt.`
    : `This is a PRE-implementation round (no implementation exists yet): emit ranked test additions/strengthenings to apply while still RED; leave mutation_runner_tasks empty.`,
  `Return ONLY JSON matching the adversarial-synthesis output contract.`,
].join('\n'), { agentType: 'straightjacket:adversarial-synthesis', schema: SYNTHESIS_SCHEMA, phase: 'Synthesis', label: 'synthesis' })

let mutationResults = []
if (mode === 'post_green') {
  const tasks = (synthesis && synthesis.mutation_runner_tasks) || []
  const canMutate = toolingAvailable.includes('cargo-mutants') || toolingAvailable.includes('stryker') || toolingAvailable.includes('dotnet-stryker')
  if (canMutate && tasks.length) {
    phase('Mutation')
    for (let i = 0; i < tasks.length; i += 3) { // plugin cap ≤3, enforced here not via the runtime ceiling
      const batch = tasks.slice(i, i + 3).map((t) => () => {
        const target = t.target_path || t.target_file || ''
        return agent([
          `Run mutation testing for ${target} (scope: ${t.scope || 'file'}, stack: ${stack}).`,
          `repo_root: ${repoRoot}`,
          `Return surviving mutants as JSON.`,
        ].join('\n'), { agentType: 'straightjacket:mutation-runner', schema: MUTATION_SCHEMA, phase: 'Mutation', label: `mutation:${target}` })
      })
      mutationResults = mutationResults.concat((await parallel(batch)).filter(Boolean))
    }
  }
}

// Compact structured result → main session merges into work-units.json (single writer).
// No source/test bodies cross back (keeps the orchestrator's context lean).
return {
  stage: 'adversarial-validation',
  mode,
  synthesis,
  specialist_reports: reports,
  mutation_results: mutationResults,
}
