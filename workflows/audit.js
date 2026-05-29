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
//   skeptics          refuters per round (default 2; the skill bumps to 3 for high-severity scopes)

export const meta = {
  name: 'audit',
  description: 'Source-audit: mechanical tool-runners + isolated LLM lenses fan out to find latent defects, then a refute pass (N skeptics vote over the full finding set, default refute when unconfirmable) drops false positives, then synthesis dedupes/ranks survivors and assigns a disposition (report / bug_record / work_unit_proposal). The diff is never an input; lens finders + the refuter are Read-only.',
  phases: [
    { title: 'Mechanical', detail: 'audit-runner team wraps the deterministic tools (cap 3)' },
    { title: 'Lenses', detail: 'isolated LLM lens finders, one per selected lens (cap 6)' },
    { title: 'Refute', detail: 'skeptics vote on the full LLM-finding set; default refute when unconfirmable (cap <=3)' },
    { title: 'Synthesis', detail: 'dedupe/rank survivors + mechanical; corroborated = pre-trusted; assign disposition' },
  ],
}

const { auditScope, stack, lenses = [], mechanicalTools = [], repoRoot, skeptics = 2 } = args

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

// ---- Mechanical: one audit-runner per tool, cap 3 (the plugin's mechanical-team cap) ----
phase('Mechanical')
let mechanicalFindings = []
for (const wave of chunk(mechanicalTools, 3)) {
  const r = await parallel(wave.map((tool) => () =>
    agent([
      `You are the audit-runner. Run exactly one mechanical static-analysis tool and return its JSON verbatim.`,
      `tool: ${tool}`, `stack: ${stack}`, `repo_root: ${repoRoot}`,
      `Run: straitjacket audit-run --tool ${tool} --stack ${stack} --repo-root ${repoRoot}`,
      `Return the audit-run JSON ({tool, available, nothing_scanned, findings}).`,
    ].join('\n'), { agentType: 'straitjacket:audit-runner', schema: RUNNER_SCHEMA, phase: 'Mechanical', label: `tool:${tool}` })))
  mechanicalFindings = mechanicalFindings.concat(
    r.filter(Boolean).flatMap((res) => (res.findings || []).map((f) => ({ ...f, source: 'mechanical' }))))
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
  for (const res of r.filter(Boolean)) {
    lensCoverage.push({ lens: res.lens, count: (res.findings || []).length, nothing_scanned: !!res.nothing_scanned })
    llmFindings = llmFindings.concat((res.findings || []).map((f) => ({ ...f, source: f.source || 'llm' })))
  }
}

// ---- Refute: skeptics vote over the FULL llm-finding set; mechanical findings bypass ----
phase('Refute')
let refuterVotes = []
if (llmFindings.length) {
  // Each refuter sees claim + evidence + source only (no finder reasoning), and the source itself.
  const claimsOnly = llmFindings.map((f, i) => ({ ref: i, lens: f.lens, severity: f.severity, title: f.title, summary: f.summary, suspect_files: f.suspect_files, file: f.file, line: f.line, evidence: f.evidence }))
  const votes = await parallel(Array.from({ length: Math.min(skeptics, 3) }, (_unused, k) => () =>
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
  `Keep an LLM finding only if it SURVIVED the refute quorum (>= half of ${Math.min(skeptics, 3)} skeptics voted survive).`,
  `An LLM lens + a mechanical tool flagging the same issue => mark source:"corroborated" (pre-trusted, keep without refutation).`,
  `Drop refuted findings (list them in refuted_findings); surface uncertain ones (uncertain_findings) but never auto-file them.`,
  `Rank survivors by severity; assign each a disposition (report | bug_record | work_unit_proposal) and ensure bridge fields are filled.`,
  `llm_findings: ${JSON.stringify(llmFindings)}`,
  `refuter_votes: ${JSON.stringify(refuterVotes)}`,
  `mechanical_findings: ${JSON.stringify(mechanicalFindings)}`,
  `Return ONLY JSON: {confirmed_findings, refuted_findings, uncertain_findings, synthesis_status}.`,
].join('\n'), { agentType: 'straitjacket:audit-synthesis', schema: SYNTH_SCHEMA, phase: 'Synthesis', label: 'audit-synthesis' })

return {
  stage: 'audit',
  confirmed_findings: (synthesis && synthesis.confirmed_findings) || [],
  refuted_findings: (synthesis && synthesis.refuted_findings) || [],
  uncertain_findings: (synthesis && synthesis.uncertain_findings) || [],
  mechanical_findings: mechanicalFindings,
  lens_coverage: lensCoverage,
  refutation_summary: { skeptics: Math.min(skeptics, 3), llm_finding_count: llmFindings.length },
  synthesis_status: (synthesis && synthesis.synthesis_status) || 'degraded',
}
