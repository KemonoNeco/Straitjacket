# regression-tests plugin

A Claude Code plugin with two multi-agent test workflows backed by a shared specialist-agent framework.

## Skills

- **`/regression-tests:regression-tests`** — generate regression tests for recent changes or a target module. Locks current behavior. Coverage planning → parallel test authoring → adversarial team review (3 Sonnet specialists + 1 Opus synthesis) → mutation testing → optional fuzzing with reproducer mining.
- **`/regression-tests:tdd`** — drive new feature development. Tests written first with minimal stubs (compile-but-fail) → pre-validated by the adversarial team → implementation written by an Opus `implementation-author` → re-validated by adversarial team + mutation testing to verify the "passing reason" is real (not gamed).

## Agents (shared across both skills)

| Agent | Model | Effort | Role |
|---|---|---|---|
| `coverage-reviewer` | opus | xhigh | Synthesis: spec/diff → locked work-unit contracts |
| `unit-test-author` | sonnet | high | Parallel team, code writing |
| `integration-test-author` | opus | xhigh | Reasoning-heavy boundary work |
| `adversarial-vacuousness` | sonnet | high | Specialist: vacuous tests + test-mutation patterns |
| `adversarial-happy-path` | sonnet | high | Specialist: happy-path bias + edge cases |
| `adversarial-misalignment` | sonnet | high | Specialist: test ↔ contract alignment |
| `adversarial-synthesis` | opus | xhigh | Synthesis over the three specialists' findings |
| `mutation-runner` | haiku | — | Mechanical: cargo-mutants / dotnet-stryker |
| `fuzz-harness-author` | opus | xhigh | Reasoning-heavy harness design |
| `fuzz-runner` | haiku | — | Mechanical: cargo-fuzz / SharpFuzz |
| `implementation-author` | opus | xhigh | TDD green-phase code writer |

## Hooks

`hooks/hooks.json` enforces invariants automatically:

- **`UserPromptExpansion`** on the plugin's skill names → runs preflight (git/stack/tooling/baseline/lint). Blocks if baseline is red.
- **`PreToolUse`** on `Agent` → scans prompts for forbidden strings before adversarial agents spawn (defense-in-depth on top of their tool restrictions).
- **`PostToolUse`** on `Agent` → auto-runs `verify-no-test-mutation` + `verify-new-tests-compile` after authors return, or `run-new-tests` after implementation-author returns. Blocks with diagnostics for retry.

## Rust binary (`bin/regression-tests`)

Single CLI with subcommands for the deterministic helpers:

```
regression-tests detect-stack
regression-tests baseline-check  --repo-root <p> --stack <s> --log-dir <d>
regression-tests lint-check      --repo-root <p> --stack <s> --log-dir <d>
regression-tests snapshot-tests  --repo-root <p> --out-file <p>
regression-tests verify-no-test-mutation --repo-root <p> --snapshot-file <p>
regression-tests verify-new-tests-compile --repo-root <p> --work-units <p> --stack <s>
regression-tests fuzz-setup      --repo-root <p> --stack <s>
regression-tests reproducer-to-test --repro <p> --target <name> --stack <s> --work-units <p>
regression-tests run-new-tests   --repo-root <p> --work-units <p> --stack <s> [--expect=fail]
regression-tests preflight       (combined: detect-stack + baseline-check + lint-check)
regression-tests hook <event>    (hook entry points: preflight | pre-adversarial | post-agent)
```

## Status

Currently Windows-x86_64 only. Cross-platform binaries (Linux, macOS) via GitHub Actions matrix is future work. To use, install Rust toolchain, then `cargo build --release && cp target/release/regression-tests.exe bin/`.

## Install (local dev)

```bash
claude --plugin-dir ~/Code/regression-tests-plugin
```

Then invoke `/regression-tests:regression-tests` or `/regression-tests:tdd` in a Rust or C# project's git working tree.
