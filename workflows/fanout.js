// fanout.js — generic capped parallel-dispatch workflow stage.
//
// Emitted by `straightjacket workflow-script fanout` and run via the Workflow tool's
// inline `script`. The SKILL builds each task's full self-contained prompt + picks the
// agentType (judgment stays in the skill); this script is pure plumbing — it runs the
// tasks in parallel within the plugin's cost cap and returns each task's structured
// result for the main session to merge into work-units.json (Cardinal Rule 1).
//
// Used by: tdd stage B (test+stub authoring), tdd stage D (implementation),
//          straightjacket Phase 3 (test authoring).
//
// Bindings via `args` (the diff is NEVER passed; authoring agents Read source themselves):
//   args.tasks  [{ agentType, prompt, label }]  — one entry per chunk
//   args.cap    max concurrent (skill sets: 6 for authors, 4 for implementation)

export const meta = {
  name: 'fanout',
  description: 'Generic capped parallel dispatch of authoring/implementation agents. The skill builds each task prompt and chooses the agentType; the script runs them in parallel within the cap and returns per-task structured results. Used by tdd authoring (B) + implementation (D) and straightjacket Phase 3.',
  phases: [
    { title: 'Fanout', detail: 'parallel agent tasks, batched to the cap' },
  ],
}

const { tasks = [], cap = 6 } = args

// Author/impl agents return a wrapper of per-unit results; keep the schema permissive so
// both the test-author shape ({results:[{work_unit_id,status,file_written,...}]}) and the
// implementation-author shape validate.
const CHUNK_RESULT_SCHEMA = {
  type: 'object',
  additionalProperties: true,
  properties: {
    results: { type: 'array' },
    notes_to_orchestrator: { type: 'string' },
  },
  required: ['results'],
}

phase('Fanout')
let chunkResults = []
for (let i = 0; i < tasks.length; i += cap) {
  const batch = tasks.slice(i, i + cap).map((t) => () =>
    agent(t.prompt, { agentType: t.agentType, schema: CHUNK_RESULT_SCHEMA, phase: 'Fanout', label: t.label || t.agentType }))
  chunkResults = chunkResults.concat((await parallel(batch)).filter(Boolean))
}

// Flatten per-chunk {results:[...]} into one list for the main session to merge.
const results = chunkResults.flatMap((c) => (c && Array.isArray(c.results)) ? c.results : [])
return { stage: 'fanout', chunk_count: chunkResults.length, results }
