---
name: fuzz
description: "Stand-alone coverage-guided fuzzing of a target: write libFuzzer/SharpFuzz harnesses for fuzzable functions, run them for a time budget, and mine every crash into a deterministic regression test pinned to the crashing input. Use when the user wants to fuzz a parser/deserializer/decoder, fuzz an untrusted-input handler, find crashing inputs, or harden a function against malformed input — WITHOUT running the whole tdd cycle. Supports Rust (cargo-fuzz, nightly) and C# (SharpFuzz); degrades to a clear skip when the fuzz toolchain is absent."
---

# fuzz

A thin launcher over the plugin's existing fuzz machinery — the `fuzz-harness-author` and
`fuzz-runner` agents and the `reproducer-to-test` CLI. The shared engine (agent roster, the
`fanout` stage, dispatch convention, run-state) lives in [`docs/STAGES.md`](../../docs/STAGES.md);
this skill does not restate it.

## Cardinal rules

1. **You are the single writer** of `work-units.json`; `reproducer-to-test` appends the mined regression units and you merge.
2. **A harness catches only the exceptions the contract allows.** Catching everything defeats the fuzzer — the `fuzz-harness-author` owns this; don't relax it.
3. **Every crash becomes a committed, deterministic regression test** named by a hash of the input bytes — that is the durable output, not the transient corpus.

## Args

- `<target>` — a file, `crate::module` symbol, or fuzzable function to fuzz. Absent → the existing fuzz targets reported by `fuzz-setup`.
- `--fuzz-time <seconds>` — per-target budget (default 60).
- `--max-targets N` — cap the number of harnesses run this session.

## Preflight

1. Confirm a git repo; resolve `repo_root`; tree should be **green** (fuzz is in the green-baseline preflight matcher). Generate `run_id`.
2. `straitjacket detect-stack --repo-root <repo_root>` → `stack`.
3. `straitjacket fuzz-setup --repo-root <repo_root> --stack <stack>` — probe the fuzz toolchain (Rust: `cargo-fuzz` + nightly; C#: SharpFuzz) and list existing fuzz targets. **If the toolchain is absent → STOP** with a clear message (fuzzing cannot degrade to static — unlike audit/mutation, there is no fallback). Record presence in `<run_id>/tooling.json`.

## Run the fuzz pass

1. **Author harnesses.** Dispatch a single `fuzz-harness-author` (direct `Agent` — single agent, not a fan-out) with the fuzzable targets + the `fuzz-setup` scaffolding info + the per-target budget. It writes the harnesses, builds them to confirm they compile, and returns `runner_tasks` (one per harness).
2. **Run the harnesses.** Dispatch the `fuzz-runner` team — **capped at 2** — over the runner tasks. Workflow path: the `fanout` stage (`tasks` = one per runner, `cap: 2`); read the runners' shape off the stage's `raw` (each returns `{crashes:[...]}`, not `results`). Agent path: spawn the runners in one message. Each returns crash artifacts (path, SHA-256, trace).
3. **Mine crashes into tests.** For each crash artifact, run:
   ```
   straitjacket reproducer-to-test --repro-path <path> --target-file <file> \
     --target-function <symbol> --stack <stack> --repo-root <repo_root> \
     --work-units-file <run_id>/work-units.json
   ```
   This writes a deterministic regression test (named by the input hash) into a `regressions/` test module and appends a WorkUnit.
4. **Verify** the mined regression tests run: `straitjacket run-new-tests --work-units-file <run_id>/work-units.json --stack <stack> --log-dir <run_id>` (branch on `nothing_to_run`). A mined test that does not now fail-then-pin the crash is a `surfaced_bug` — escalate, do not silence.

## Final summary (present verbatim)

- **Run metadata**: run_id, stack, fuzz budget, targets fuzzed.
- **Harnesses written** (files) / **skipped** (with reason: target unsuitable, tool missing).
- **🐛 Crashes found** → **Reproducers committed** (test name + input hash + a one-line trace).
- **Degraded / skipped** (loud): toolchain absent, targets with no harness.
- **Known-limitation note**: "Fuzzing finds crashes, not correctness — a mined test pins *that the input no longer crashes*, which is only as meaningful as the harness's allowed-exception set."

## Notes

- A single Opus `fuzz-harness-author` + a capped Haiku `fuzz-runner` team. Artifacts under `<repo>/.claude-regression/<run_id>/`; the `straitjacket` CLI is on `PATH` via the plugin's `bin/`.
- To turn a parked fuzz finding into a fix, hand the reproducer to `tdd` fix-mode / `triage`.
