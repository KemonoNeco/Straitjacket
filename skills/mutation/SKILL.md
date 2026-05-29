---
name: mutation
description: "Stand-alone mutation testing of a target: run cargo-mutants (Rust) or Stryker.NET (C#) over a file/module/project, surface the surviving mutants, and emit each as a work-unit proposal describing the UNDER-TESTED behavior class for a later tdd run to cover. Use when the user wants to mutation-test code, measure mutation score, find gaps the existing tests don't catch, or harden a module's test suite — WITHOUT writing tests in this run. Analysis-only: it proposes coverage gaps as data; it never writes tests itself. Supports Rust (cargo-mutants) and C# (dotnet-stryker); degrades to a clear skip when the tool is absent."
---

# mutation

A thin launcher over the existing `mutation-runner` agent and the `fanout` stage. The shared
engine lives in [`docs/STAGES.md`](../../docs/STAGES.md); this skill does not restate it.

Mutation is **analysis-only** (like `audit`): it surfaces surviving mutants and emits
**work-unit proposals as data** for `tdd`/`triage` to lift — it never writes or spawns test
authors. A surviving mutant means *no test fails when the code is broken there* — a coverage gap.

## Cardinal rules

1. **You are the single writer** of any run-state you persist; the stage returns data, you merge.
2. **A proposal describes the under-tested BEHAVIOR CLASS, never the mutant.** "X must reject a zero-length header" — not "kill the `< ` → `<=` mutant at line 42". Anchoring a test to a mutant produces a brittle, vacuous test.
3. **`nothing_scanned`/zero-mutant is loud.** A tool that scanned nothing (absent / empty scope / build failure) is reported distinctly from a real 100% mutation score.

## Args

- `<target>` — a file, `crate::module`, directory, or project to mutate. Absent → the crate/project at `repo_root`.
- `--scope file|module|project` — mutation granularity (default: `file` for Rust, `project` for C# — Stryker's warm-up makes per-file thrash).
- `--file-proposals` — also write the proposals to `<run_id>/work-units.json` as `pending` units (default: emit them in the summary only, for review first).

## Preflight

1. Confirm a git repo; resolve `repo_root`; tree should be **green** (mutation is in the green-baseline preflight matcher — mutants are only meaningful against a passing suite). Generate `run_id`.
2. `straitjacket detect-stack --repo-root <repo_root>` → `stack`.
3. Probe the mutation tool (`cargo mutants --version` / `dotnet stryker --version`) → `<run_id>/tooling.json`. **If absent → STOP** with a clear message (mutation has no static fallback).

## Run the mutation pass

1. **Partition** the target into runner tasks by `--scope` (one task per file/module/project).
2. **Run the mutation team — capped at 2-3.** Workflow path: the `fanout` stage (`tasks` = one `mutation-runner` per target, `cap: 3`); read the runners' shape off the stage's `raw` (each returns `{surviving_mutants:[...]}`, not `results`). Agent path: spawn the runners in one message. Each returns its mutation score + surviving mutants (file, line, function, original, mutated, mutator).
3. **Turn survivors into proposals.** For each surviving mutant, infer the under-tested behavior class it exposes and emit a work-unit proposal: `{ intended_behavior, target_file, target_symbol, kind }`. Dedupe proposals that target the same behavior. If `--file-proposals`, write them to `work-units.json` as `pending`, `source_of_unit: "mutation_runner"`, for a `tdd` run to author.

## Final summary (present verbatim)

- **Run metadata**: run_id, stack, scope, targets mutated.
- **Mutation score** per target (killed / (killed + survived)).
- **🧟 Surviving mutants** (file:line · mutator) — grouped by behavior class.
- **🧪 Coverage-gap proposals** (the data to lift with `tdd`): each proposed `intended_behavior` + `target_file`/`target_symbol`.
- **Degraded / skipped** (loud): tool absent, targets that failed to build, `nothing_scanned`.
- **Known-limitation note**: "A surviving mutant is a *hint* of a coverage gap, not proof of a bug; some mutants are equivalent (no behavioral change) and should be dismissed, not tested."

## Notes

- A capped Haiku `mutation-runner` team; the proposal-framing is the skill's judgment. Artifacts under `<repo>/.claude-regression/<run_id>/`; the CLI is on `PATH` via the plugin's `bin/`.
- This is the same machinery the `tdd` post-green stage runs inline; here it is exposed stand-alone so a dev can mutation-test without a full cycle.
