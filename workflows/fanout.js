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
  description: 'Generic capped parallel dispatch of agent tasks. The skill builds each task prompt and chooses the agentType; the script runs them in parallel within the cap and returns per-task structured results (incl. an `attempted` count for partial-dispatch detection). Consumed by the mutation and fuzz skills\' runner teams. NOTE: tdd-cycle does NOT use this stage — it inlines its own fanout() because workflow scripts cannot import one another.',
  phases: [
    { title: 'Fanout', detail: 'parallel agent tasks, batched to the cap' },
  ],
}

// Normalize + validate `args` before consuming it (issues #54 + #58). The Workflow runtime can deliver
// `args` as a JSON STRING of a valid object; parse-and-adopt it when it is a plain object, else keep the
// original, then run the plain-object guard BEFORE the destructure (null/undefined yields the actionable
// message, not a raw TypeError — #58). Genuine non-object / CLI-string still fails loudly — NARROWS #36,
// does not remove it. Routed through a local `cfg`, NOT a reassignment of the injected `args` global.
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
  throw new Error(`straitjacket:fanout — args must be a plain object, got ${cfg === null ? 'null' : (Array.isArray(cfg) ? 'Array' : typeof cfg)}${_argParseErr}; pass { tasks: [...], cap } not a CLI string`)
}
const { tasks = [], cap = 6 } = cfg
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

// (the args-shape guard now runs above, BEFORE the destructure, on the normalized `cfg` — issues #54 + #58.)

phase('Fanout')
let chunkResults = []
for (let i = 0; i < tasks.length; i += step) {
  const batch = tasks.slice(i, i + step).map((t) => () =>
    agent(t.prompt, { agentType: t.agentType, schema: CHUNK_RESULT_SCHEMA, phase: 'Fanout', label: t.label || t.agentType }))
  chunkResults = chunkResults.concat((await parallel(batch)).filter(Boolean))
}

// `results`: per-chunk {results:[...]} flattened for the authoring/impl merge path.
// `raw`: every chunk verbatim, so runner shapes ({surviving_mutants}/{crashes}) survive.
// `attempted` vs `chunk_count`: tasks DISPATCHED vs chunks that RETURNED a non-null result. A gap means
// an agent returned null after its retry budget and was dropped by the .filter(Boolean) above — so a
// consumer (the mutation / fuzz skills; tdd-cycle uses its own INLINE fanout(), not this stage) can
// detect partial dispatch failure instead of mistaking a shrunken result for a complete one (issue #37).
const results = chunkResults.flatMap((c) => (c && Array.isArray(c.results)) ? c.results : [])
return { stage: 'fanout', attempted: tasks.length, chunk_count: chunkResults.length, results, raw: chunkResults }
