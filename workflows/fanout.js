// fanout.js — generic capped parallel-dispatch workflow stage.
//
// Emitted by `straitjacket workflow-script fanout` and run via the Workflow tool's
// inline `script`. The SKILL builds each task's full self-contained prompt + picks the
// agentType (judgment stays in the skill); this script is pure plumbing — it runs the
// tasks in parallel within the plugin's cost cap and returns each task's structured
// result for the main session to merge into work-units.json (Cardinal Rule 1).
//
// Used by: tdd authoring + implementation (test+stub, then green — via tdd-cycle), the
//          mutation skill (mutation-runner team), and the fuzz skill (fuzz-runner team).
//
// Bindings via `args` (the diff is NEVER passed; authoring agents Read source themselves):
//   args.tasks  [{ agentType, prompt, label }]  — one entry per chunk
//   args.cap    max concurrent (skill sets: 6 for authors, 4 for implementation)

export const meta = {
  name: 'fanout',
  description: 'Generic capped parallel dispatch of authoring/implementation agents. The skill builds each task prompt and chooses the agentType; the script runs them in parallel within the cap and returns per-task structured results. Used by tdd authoring (B) + implementation (D) and straitjacket Phase 3.',
  phases: [
    { title: 'Fanout', detail: 'parallel agent tasks, batched to the cap' },
  ],
}

const { tasks = [], cap = 6 } = args
// cap is clamped to a positive integer so a 0 / negative / NaN / stringified cap can't spin
// forever (i += 0) or silently process nothing (NaN < length === false) — same class of hazard
// as tdd-cycle.js's chunk() size (issue #31).
const step = Math.max(1, Math.floor(Number(cap)) || 1)

// Two agent shapes ride this stage. Authoring/impl agents return a wrapper of per-unit
// results ({results:[{work_unit_id,status,file_written,...}]}); the mechanical runners this
// stage is ALSO reused for (mutation-runner → {surviving_mutants}, fuzz-runner → {crashes})
// have NO `results` key. So the schema must NOT require `results` (else a runner's valid
// output is rejected and retried forever). The caller picks `results` (flattened, for
// authoring) OR `raw` (per-chunk verbatim, for runners) — both are returned.
const CHUNK_RESULT_SCHEMA = {
  type: 'object',
  additionalProperties: true,
  properties: {
    results: { type: 'array' },
    notes_to_orchestrator: { type: 'string' },
  },
}

phase('Fanout')
let chunkResults = []
for (let i = 0; i < tasks.length; i += step) {
  const batch = tasks.slice(i, i + step).map((t) => () =>
    agent(t.prompt, { agentType: t.agentType, schema: CHUNK_RESULT_SCHEMA, phase: 'Fanout', label: t.label || t.agentType }))
  chunkResults = chunkResults.concat((await parallel(batch)).filter(Boolean))
}

// `results`: per-chunk {results:[...]} flattened for the authoring/impl merge path.
// `raw`: every chunk verbatim, so runner shapes ({surviving_mutants}/{crashes}) survive.
const results = chunkResults.flatMap((c) => (c && Array.isArray(c.results)) ? c.results : [])
return { stage: 'fanout', chunk_count: chunkResults.length, results, raw: chunkResults }
