// audit.js — the source-audit workflow stage.
//
// Emitted by `straitjacket workflow-script audit` and run via the Workflow tool. Finds
// latent defects in SOURCE (not tests): correctness/latent bugs, dead code, doc-drift,
// performance, security, concurrency, error-handling — via isolated LLM lenses AND mechanical
// tool-runners — then a REFUTE pass drops the false positives before anything is reported.
//
// LLM source-audits are false-positive-heavy, so refutation is the SPINE, not a flag:
//   Mechanical (audit-runner x tools)  ∥  Lenses (audit-<lens> x selected)
//     -> Refute (audit-refuter x skeptics, each voting over the FULL llm-finding set)
//     -> Synthesis (dedupe/rank survivors + mechanical; assign disposition)
//
// Isolation: the lens finders + the refuter are Read/Grep/Glob-only (no Bash), like the
// adversarial trio. The diff is NEVER an input — they Read the scope themselves. The refuter
// is handed each finding's claim + evidence + source, NOT the finder's private reasoning.
//
// Bindings via `args`:
//   auditScope        files / dirs / symbols to review (the lenses Read these themselves)
//   stack             "rust" | "csharp" | "both"
//   lenses            string[] of lens names (e.g. ["latent-bug","security","dead-code"])
//   mechanicalTools   string[] of tool names for audit-runner (e.g. ["clippy-dead-code","cargo-audit"])
//   repoRoot          absolute repo root (for the mechanical runners, which keep Bash)
//   skeptics          refuters per round (default 3 — a true majority quorum; cap 3, Opus at medium effort)

export const meta = {
  name: 'audit',
  description: 'Source-audit: mechanical tool-runners + isolated LLM lenses fan out to find latent defects, then a refute pass (N skeptics vote over the full finding set, default refute when unconfirmable) drops false positives, then synthesis dedupes/ranks survivors and assigns a disposition (report / bug_record / work_unit_proposal). The diff is never an input; lens finders + the refuter are Read-only.',
  phases: [
    { title: 'Mechanical', detail: 'audit-runner team wraps the deterministic tools (cap 3)' },
    { title: 'Lenses', detail: 'isolated LLM lens finders, one per selected lens (cap 6)' },
    { title: 'Refute', detail: 'skeptics vote on the full LLM-finding set; default refute when unconfirmable (cap <=3; Opus refuters at medium effort)' },
    { title: 'Synthesis', detail: 'dedupe/rank survivors + mechanical; corroborated = pre-trusted; assign disposition' },
  ],
}

const { auditScope, stack, lenses = [], mechanicalTools = [], repoRoot } = args
// `skeptics` is SANITIZED, not a destructure default: a destructure default only fills `undefined`,
// so a null / 0 / negative / non-numeric arg would flow into `Math.min(skeptics, 3)` below as 0 and
// SILENTLY disable the refute phase (0 refuters → every finding judged with no votes). Floor at 1;
// the upper cap of 3 is applied at each use site. (Same args-degeneracy class as bug-2026-06-01-13.)
const skeptics = Math.max(1, parseInt(args.skeptics, 10) || 3)

const RUNNER_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { tool: { type: 'string' }, available: { type: 'boolean' }, nothing_scanned: { type: 'boolean' }, findings: { type: 'array' } },
  required: ['findings'],
}
const LENS_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { lens: { type: 'string' }, findings: { type: 'array' }, nothing_scanned: { type: 'boolean' }, isolation_check: { type: 'object' } },
  required: ['findings'],
}
const REFUTER_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { votes: { type: 'array' }, isolation_check: { type: 'object' } },
  required: ['votes'],
}
const SYNTH_SCHEMA = {
  type: 'object', additionalProperties: true,
  properties: { confirmed_findings: { type: 'array' }, refuted_findings: { type: 'array' }, uncertain_findings: { type: 'array' }, synthesis_status: { type: 'string' } },
  required: ['confirmed_findings'],
}

function chunk(arr, size) {
  const out = []
  for (let i = 0; i < arr.length; i += size) out.push(arr.slice(i, i + size))
  return out
}

if (!args || typeof args !== 'object' || Array.isArray(args)) {
  throw new Error(`straitjacket:audit — args must be a plain object, got ${args === null ? 'null' : (Array.isArray(args) ? 'Array' : typeof args)}; pass { auditScope, stack, lenses, ... } not a CLI string`)
}
if (!auditScope || (Array.isArray(auditScope) && !auditScope.length)) throw new Error('straitjacket:audit — required arg `auditScope` is missing or empty')
if (!Array.isArray(lenses) || !lenses.length) throw new Error('straitjacket:audit — required arg `lenses` must be a non-empty array')

// ---- Mechanical: one audit-runner per tool, cap 3 (the plugin's mechanical-team cap) ----
phase('Mechanical')
let mechanicalFindings = []
const mechanicalCoverage = []
for (const wave of chunk(mechanicalTools, 3)) {
  const r = await parallel(wave.map((tool) => () =>
    agent([
      `You are the audit-runner. Run exactly one mechanical static-analysis tool and return its JSON verbatim.`,
      `tool: ${tool}`, `stack: ${stack}`, `repo_root: ${repoRoot}`,
      `Run: straitjacket audit-run --tool ${tool} --stack ${stack} --repo-root ${repoRoot}`,
      `Return the audit-run JSON ({tool, available, nothing_scanned, findings}).`,
    ].join('\n'), { agentType: 'straitjacket:audit-runner', schema: RUNNER_SCHEMA, phase: 'Mechanical', label: `tool:${tool}` })))
  // Account for EVERY dispatched runner, including a null return (issue #40): a runner agent that died
  // returns falsy and would otherwise vanish entirely (the Mechanical phase had NO coverage ledger), so
  // a tool that never ran was indistinguishable from one that scanned and found nothing. Zip the wave's
  // dispatched tool NAMES with the returns (parallel preserves order; a failed thunk resolves to null)
  // so a drop is recorded as failed coverage keyed by the dispatched name, not by res.tool (absent on a
  // dead agent). The findings concat still skips falsy returns — a dropped runner contributes none.
  wave.forEach((tool, j) => {
    const res = r[j]
    if (res) {
      mechanicalCoverage.push({ tool: res.tool || tool, count: (res.findings || []).length, available: res.available !== false, nothing_scanned: !!res.nothing_scanned, failed: false })
      mechanicalFindings = mechanicalFindings.concat((res.findings || []).map((f) => ({ ...f, source: 'mechanical' })))
    } else {
      mechanicalCoverage.push({ tool, count: 0, available: false, nothing_scanned: true, failed: true })
    }
  })
}

// ---- Lenses: one isolated finder per selected lens, cap 6 ----
phase('Lenses')
let llmFindings = []
const lensCoverage = []
for (const wave of chunk(lenses, 6)) {
  const r = await parallel(wave.map((lens) => () =>
    agent([
      `mode: audit; stack: ${stack}`,
      `You are the audit-${lens} lens. Apply ONLY your lens. Operate in isolation.`,
      `audit_scope (READ these yourself; you have Read/Grep/Glob):`, JSON.stringify(auditScope, null, 2),
      `You will NOT be given a diff or "what changed" framing — an audit reviews the current state.`,
      `Emit findings per schemas/audit-finding.schema.json with source:"llm" and lens:"${lens}".`,
      `Fill the bridge fields (suspect_files/suspect_symbol/intended_behavior_seed) for any bug_record/work_unit_proposal.`,
      `Return ONLY JSON per your output contract, incl. isolation_check.`,
    ].join('\n'), { agentType: `straitjacket:audit-${lens}`, schema: LENS_SCHEMA, phase: 'Lenses', label: `lens:${lens}` })))
  // Account for EVERY dispatched lens, including a null return (issue #40): a lens agent that died
  // returns falsy and the old `for (const res of r.filter(Boolean))` dropped it BEFORE the coverage
  // push, so a lens that silently failed to run was indistinguishable from one that scanned clean.
  // Zip the wave's dispatched lens NAMES with the returns (parallel preserves order; a failed thunk
  // resolves to null) and record a drop as failed/nothing_scanned coverage keyed by the dispatched
  // name — `res.lens` is unavailable on a dead agent, so the in-scope `lens` is the only stable key.
  wave.forEach((lens, j) => {
    const res = r[j]
    if (res) {
      lensCoverage.push({ lens: res.lens || lens, count: (res.findings || []).length, nothing_scanned: !!res.nothing_scanned, failed: false })
      llmFindings = llmFindings.concat((res.findings || []).map((f) => ({ ...f, source: f.source || 'llm' })))
    } else {
      lensCoverage.push({ lens, count: 0, nothing_scanned: true, failed: true })
    }
  })
}

// ---- Refute: skeptics vote over the FULL llm-finding set; mechanical findings bypass ----
phase('Refute')
let refuterVotes = []
let refutersDispatched = 0   // skeptics actually dispatched this run (0 when there were no LLM findings to refute)
if (llmFindings.length) {
  // Each refuter sees claim + evidence + source only (no finder reasoning), and the source itself.
  // Carry every finding field the audit-refuter contract (agents/audit-refuter.md "Inputs") enumerates
  // as provided — notably suspect_symbol, the drift-resistant language-qualified locator, plus the
  // when-present expected/actual (issue #43). Omit the array index `ref`: the refuter is told below to
  // key votes by title (NOT the index) and synthesis joins by title, so `ref` was dead payload.
  const claimsOnly = llmFindings.map((f) => ({ lens: f.lens, severity: f.severity, title: f.title, summary: f.summary, expected: f.expected, actual: f.actual, suspect_files: f.suspect_files, suspect_symbol: f.suspect_symbol, file: f.file, line: f.line, evidence: f.evidence }))
  refutersDispatched = Math.min(skeptics, 3)
  const votes = await parallel(Array.from({ length: refutersDispatched }, (_unused, k) => () =>
    agent([
      `You are an audit-refuter (skeptic #${k + 1}). For EACH finding below, vote refute / survive / uncertain.`,
      `DEFAULT to refute when you cannot independently confirm the claim by reading the cited source — this audit drops the unconfirmable.`,
      `You see each finding's claim + evidence + source ONLY (not the finder's private reasoning). READ the cited files yourself.`,
      `findings:`, JSON.stringify(claimsOnly, null, 2),
      `Key each vote by the finding's "title" (add its file:line if titles repeat) as finding_ref — NOT the array index — so audit-synthesis can join your votes to the findings.`,
      `Return ONLY JSON: {"votes":[{"finding_ref":"<finding title>","verdict":"refute|survive|uncertain","reason":"..."}], "isolation_check":{...}}.`,
    ].join('\n'), { agentType: 'straitjacket:audit-refuter', schema: REFUTER_SCHEMA, phase: 'Refute', label: `refuter:${k + 1}` })))
  refuterVotes = votes.filter(Boolean)
}

// ---- Synthesis: dedupe/rank survivors + mechanical; corroborate; assign disposition ----
phase('Synthesis')
const synthesis = await agent([
  `mode: audit; stack: ${stack}`,
  `Synthesize this audit. Inputs: the LLM findings, the refuter votes over them, and the mechanical findings.`,
  // Quorum (issue #42): keep an LLM finding only on a STRICT MAJORITY of the DISPATCHED skeptic pool —
  // never the number of vote sets that happened to return. A refuter that died is a NON-confirmation,
  // not an abstention that shrinks the bar; counting only returned votes is the lenient direction and
  // would violate this audit's "default refute when unconfirmable" spine (a lone survivor among two
  // dead refuters must NOT carry a finding). A refute, an uncertain, AND a missing/null vote each count
  // as NOT survive, so a tie at an even skeptic count REFUTES.
  `Keep an LLM finding only if it SURVIVED the refute quorum: a STRICT MAJORITY of the ${Math.min(skeptics, 3)} DISPATCHED skeptics voted survive — i.e. survive_votes * 2 > ${Math.min(skeptics, 3)}. The denominator is the DISPATCHED skeptic count (${Math.min(skeptics, 3)}), NOT the number of vote sets returned; a refute, an uncertain, or a missing/null vote each counts as NOT survive, so a tie refutes (default-refute when unconfirmable).`,
  `An LLM lens + a mechanical tool flagging the same issue => mark source:"corroborated" (pre-trusted, keep without refutation).`,
  `Drop refuted findings (list them in refuted_findings); surface uncertain ones (uncertain_findings) but never auto-file them.`,
  `Rank survivors by severity; assign each a disposition (report | bug_record | work_unit_proposal) and ensure bridge fields are filled.`,
  // Coverage (issue #40): a lens/runner that returned null scanned NOTHING. If any coverage entry has
  // failed:true the scan is PARTIAL — set synthesis_status:"degraded" and note it; never report "ok"
  // over a silently-incomplete scan. (The orchestrator also enforces this deterministically below.)
  `lens_coverage: ${JSON.stringify(lensCoverage)}`,
  `mechanical_coverage: ${JSON.stringify(mechanicalCoverage)}`,
  `If any lens_coverage / mechanical_coverage entry has failed:true, set synthesis_status:"degraded" (a lens or tool failed to run — the scan is partial).`,
  `llm_findings: ${JSON.stringify(llmFindings)}`,
  `refuter_votes: ${JSON.stringify(refuterVotes)}`,
  `mechanical_findings: ${JSON.stringify(mechanicalFindings)}`,
  `Return ONLY JSON: {confirmed_findings, refuted_findings, uncertain_findings, synthesis_status}.`,
].join('\n'), { agentType: 'straitjacket:audit-synthesis', schema: SYNTH_SCHEMA, phase: 'Synthesis', label: 'audit-synthesis' })

// Deterministically reflect partial coverage (issue #40). synthesis_status is a FREE LLM string and the
// synthesis agent's downgrade-on-partial is best-effort, so the orchestrator enforces the floor itself:
// a lens or mechanical runner that returned null scanned NOTHING, so the audit is partial regardless of
// what synthesis reported. Force the status down to "degraded" so a false-clean ("ok") result can never
// ride out over a silently-incomplete scan — and surface exactly which lenses/tools failed.
const failedLenses = lensCoverage.filter((c) => c.failed).map((c) => c.lens)
const failedMechanicalTools = mechanicalCoverage.filter((c) => c.failed).map((c) => c.tool)
const coverageComplete = !failedLenses.length && !failedMechanicalTools.length
// Zero-scan floor (issue #49, mechanism 1) — one level up from #40's failed-lens floor. #40 forces
// degraded when a DISPATCHED lens/tool returned null (failed:true). But a lens can also return a
// schema-valid result with nothing_scanned:true — every audit-<lens> contract emits that when its
// audit_scope resolved to zero readable source files. Such a lens has failed:false, so coverageComplete
// stays true and a clean 'ok' would ride out over an audit that examined NOTHING. anythingScanned is true
// only if at least one lens OR mechanical tool AFFIRMATIVELY scanned something (failed === false AND
// nothing_scanned === false); when false, the terminal status is forced to 'degraded' regardless of what
// synthesis reported. This is the OUTPUT-side fail-closed the input-side args guards (#36) cannot provide.
// Affirmative `=== false` (not `!c.failed`) per the repo's fail-closed rule (Gemini review on PR #64):
// a malformed coverage entry missing these flags must NOT count as "scanned" (an undefined flag under
// `!c.failed` would fail OPEN); requiring an explicit false keeps a degenerate entry from masking a no-scan.
const anythingScanned =
  lensCoverage.some((c) => c.failed === false && c.nothing_scanned === false) ||
  mechanicalCoverage.some((c) => c.failed === false && c.nothing_scanned === false)
// Refuter-coverage floor (issue #49 — "specialists dispatched-and-returned"). A null (dead) refuter is
// dropped by `votes.filter(Boolean)`, but unlike a failed lens/tool it is recorded in NO coverage ledger
// and the strict-majority quorum counts it as not-survive — so a TRUE finding whose skeptics died is
// flipped survive->dropped on a non-quorum it never lost on the merits, and a clean 'ok' could still ride
// out over that silently-incomplete refutation. Surface the shortfall and force 'degraded', mirroring the
// lens/mechanical floors. (Only gates when refuters were actually dispatched — none are when there were no
// LLM findings to refute, which is not an incomplete refutation.)
const refutersReturned = refuterVotes.length
const refuterCoverageComplete = refutersReturned >= refutersDispatched
const rawSynthesisStatus = (synthesis && synthesis.synthesis_status) || 'degraded'

return {
  stage: 'audit',
  confirmed_findings: (synthesis && synthesis.confirmed_findings) || [],
  refuted_findings: (synthesis && synthesis.refuted_findings) || [],
  uncertain_findings: (synthesis && synthesis.uncertain_findings) || [],
  mechanical_findings: mechanicalFindings,
  lens_coverage: lensCoverage,
  mechanical_coverage: mechanicalCoverage,
  coverage_complete: coverageComplete,
  anything_scanned: anythingScanned,
  failed_lenses: failedLenses,
  failed_mechanical_tools: failedMechanicalTools,
  refuter_coverage: { dispatched: refutersDispatched, returned: refutersReturned, complete: refuterCoverageComplete },
  refutation_summary: { skeptics: Math.min(skeptics, 3), llm_finding_count: llmFindings.length, refuters_dispatched: refutersDispatched, refuters_returned: refutersReturned },
  // A clean 'ok' requires a complete scan (no dispatched lens/tool failed), a non-empty scan (something was
  // actually examined), AND complete refuter coverage (no dispatched skeptic died) — else degraded, so an
  // incomplete scan OR an incomplete refutation can never ride out as clean (issue #40 + #49).
  synthesis_status: (coverageComplete && anythingScanned && refuterCoverageComplete) ? rawSynthesisStatus : 'degraded',
}
