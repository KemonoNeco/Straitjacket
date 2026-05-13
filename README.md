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

Currently Windows-x86_64 only. Cross-platform binaries (Linux, macOS) via GitHub Actions matrix is future work.

## Prerequisites

For **using** the plugin (most users): the shipped `bin/regression-tests.exe` is the only requirement — no toolchain needed.

For **building from source** on Windows:

- **Rust toolchain** (`rustup` with `stable-x86_64-pc-windows-msvc`). Verify with `cargo --version` and `rustc --version`.
- **MSVC C++ Build Tools** + **Windows SDK** (Visual Studio Installer → Individual Components → "Windows 11 SDK" and "MSVC v143 build tools"). Required for `link.exe` and `kernel32.lib` during `cargo build`.
- **`rustup component add rust-analyzer`** — only if you also have the `rust-analyzer-lsp` Claude Code plugin enabled. Without this component, the rustup proxy at `~/.cargo/bin/rust-analyzer.exe` exits with code 1 when the LSP starts, surfacing as `LSP server plugin:rust-analyzer-lsp:rust-analyzer crashed with exit code 1`. Installing the component is the fix; nothing in this plugin depends on it.

## Install

**Recommended** — via Claude plugin marketplace:

```bash
claude plugin marketplace add https://github.com/KemonoNeco/regression-tests-plugin
claude plugin install regression-tests@regression-tests
```

**Local dev** — point Claude Code at a checkout:

```bash
claude --plugin-dir ~/Code/regression-tests-plugin
```

Then invoke `/regression-tests:regression-tests` or `/regression-tests:tdd` in a Rust or C# project's git working tree.

## Build from source

```bash
cargo build --release
cp target/release/regression-tests.exe bin/regression-tests.exe
```
