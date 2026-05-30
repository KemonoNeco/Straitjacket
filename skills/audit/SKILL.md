---
name: audit
description: "Find latent defects in SOURCE without writing tests — correctness/latent bugs, dead code, false docs (doc-drift), performance, security, concurrency, and error-handling issues — via isolated LLM lenses plus mechanical tool-runners (clippy dead_code, cargo-audit/deny/geiger/udeps; C# analogs), with a refute pass that drops false positives before anything is reported. Use when the user wants to audit code for bugs/issues, find latent defects, hunt dead code or false docs, do a security or performance review, or check a module/diff/PR for problems WITHOUT generating tests. Analysis-only: confirmed defects are filed to the bug ledger (report-bug) and test-worthy gaps are emitted as proposals for tdd/triage to lift — audit never writes or spawns test authors. Supports Rust and C#."
---

# audit

## What this is

A **read-only** issue-finder. It runs the `audit` workflow stage (see
[`docs/STAGES.md`](../../docs/STAGES.md)): mechanical tool-runners ∥ isolated LLM lenses →
**refute** (skeptics drop false positives) → synthesis. You — the main session — own the routing
of survivors and the single-writer `audit-findings.json`.

Audit is **analysis-only**. It NEVER writes tests, edits source, or spawns author agents. A
confirmed defect is filed to the bug ledger; a correct-but-untested gap is emitted as a proposal
for `tdd`/`triage` to lift later. Audit is **not** in the green-baseline preflight matcher — you
often audit *because* the tree is unhealthy.

## Cardinal rules

1. **You are the single writer** of `<repo>/.claude-regression/<run_id>/audit-findings.json`. The stage returns data; you merge + route.
2. **Refutation is the spine, not a flag.** Never report an unrefuted LLM finding; the stage's refute pass + synthesis produce `confirmed_findings`.
3. **`nothing_scanned` is loud.** A mechanical runner that scanned nothing (tool absent / empty scope) is reported distinctly from a clean scan — never silently treated as "no issues."
4. **Analysis-only** — the surfaced-bug reflex ([STAGES.md](../../docs/STAGES.md) rule 7). Findings route to a report, the bug ledger (`report-bug`), or a proposal — audit never authors, fixes, or pivots to consulting on a fix; lifting a finding into a fix is a `tdd`/`triage` job.

## Args

- `<path>` — scope: a file, directory, or `crate::module` symbol. Absent → the repo source tree.
- `--lenses a,b,c` — LLM lenses to run. Default: `latent-bug,error-handling,security,dead-code`.
- `--all` — run all seven lenses (adds `performance,doc-drift,concurrency`).
- `--skeptics N` — refuters per round (default 2; use 3 for a high-stakes audit).
- `--no-file` — report only; do NOT write `bug_record` findings to the ledger.

## Preflight

1. Confirm a git repo; resolve `repo_root`. (No green-baseline gate — audit is read-only.)
2. Generate `run_id`; create `<repo_root>/.claude-regression/<run_id>/`.
3. `straitjacket detect-stack --repo-root <repo_root>` → `stack`.
4. **Probe mechanical tools** for the stack and keep only the available ones (degrade gracefully):
   - Rust: `clippy-dead-code` (always — clippy ships with rust), `cargo-audit`, `cargo-deny`, `cargo-geiger`, `cargo-udeps` (each only if installed).
   - C#: `dotnet-vulnerable` (always — ships with dotnet).
   - Probe by running `straitjacket audit-run --tool <t> --stack <stack> --repo-root <repo_root>` and treating `available:false` as "skip, note as degraded."

## Run the audit

**Capability check:** inspect your own tools for one named `Workflow`.

- **Present →** `straitjacket workflow-script audit` (Bash) emits the script; run
  `Workflow({script, args})` with: `auditScope` (the resolved files/dirs/symbols),
  `stack`, `lenses` (the selected lens names), `mechanicalTools` (the available tools),
  `repoRoot`, `skeptics`. **Never** pass a diff — the lenses Read the scope themselves.
- **Absent →** staged Agent dispatch: spawn the `audit-runner` team (one per tool, cap 3) and
  the `audit-<lens>` finders (one per lens, cap 6) in one message; collect findings; spawn
  `audit-refuter` ×`skeptics` over the full LLM-finding set; then `audit-synthesis`.

## Route the survivors (this session writes everything)

The stage returns `{ confirmed_findings, refuted_findings, uncertain_findings,
mechanical_findings, lens_coverage, refutation_summary, synthesis_status }`. Write all of it to
`audit-findings.json`, then route each **confirmed** finding by its `disposition`:

- **`bug_record`** → unless `--no-file`, file it via `straitjacket:report-bug` (local ledger
  first; remotes opt-in). The finding's `title/summary/expected/actual/severity` and bridge fields
  (`suspect_files`/`suspect_symbol`/`intended_behavior_seed`) map 1:1 onto the BugRecord — pass
  them straight through. report-bug's local dedupe guard prevents double-filing.
- **`work_unit_proposal`** (correct-but-untested) → **emit as data** in the summary (a list of
  proposed `intended_behavior` + `target_file`/`target_symbol`) for a later `tdd`/`triage` run to
  lift. Do NOT spawn authors.
- **`report`** (dead-code / doc-drift) → a cleanup list in the summary.
- **`uncertain_findings`** → surface in the summary, clearly labelled "unconfirmed — not filed."
- **`refuted_findings`** → an appendix only (dropped, logged for transparency).

## Final summary (present verbatim)

- **Run metadata**: run_id, stack, lenses run, tools run (+ degraded/absent).
- **🔴 Confirmed defects** filed (bug ids) — by severity.
- **🧪 Test-worthy gaps** (work-unit proposals — lift with tdd/triage).
- **🧹 Cleanup** (dead code / doc-drift).
- **❓ Unconfirmed** (uncertain — not filed).
- **Dropped** (refuted count) + **`nothing_scanned`** tools (loud, not silent).
- **Known-limitation note**: "LLM lenses are advisory and false-positive-prone; survivors passed a
  skeptic refute quorum but are not proof. Mechanical findings are as reliable as their tool."

## Notes

- Mostly Opus turns (the lens finders + refuter + synthesis) plus Haiku `audit-runner`s.
- All artifacts live under `<repo>/.claude-regression/<run_id>/`; the bug ledger at
  `<repo>/.straitjacket/bugs.json` is tracked/committed. The CLI is on `PATH` via the plugin's `bin/`.
