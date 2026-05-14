# regression-tests plugin

A Claude Code plugin that ships two multi-agent test workflows on top of a shared specialist-agent framework. Both skills harden test suites against the same four failure modes — **happy-path bias**, **vacuous assertions**, **test-mutation cheats**, and **test-contract misalignment** — using parallel specialist subagents, mutation testing, and (optionally) fuzzing.

* **Looking for usage?** Jump to [Quickstart](#quickstart).
* **Looking for design?** Read [docs/TECHNICAL.md](docs/TECHNICAL.md) for the architecture deep-dive.
* **Contributing to the plugin?** Read [CLAUDE.md](CLAUDE.md) for the load-bearing invariants.

## Skills

| Slash command | Purpose |
|---|---|
| `/regression-tests:regression-tests` | Generate regression tests for recent changes or a target module. Locks current behavior. |
| `/regression-tests:tdd` | Drive new feature development by writing failing tests first, then implementing against them. |

Both skills run the same pipeline shape: **coverage planning → parallel test authoring → adversarial team review → mutation testing → optional fuzzing**. The `tdd` skill adds a green phase (`implementation-author` writes source to pass the tests) followed by a second adversarial pass to verify the *passing reason* is real, not gamed.

## Quickstart

```bash
# 1. Install (via Claude plugin marketplace)
claude plugin marketplace add https://github.com/KemonoNeco/regression-tests-plugin
claude plugin install regression-tests@regression-tests

# 2. Verify install — should report skills/agents/hooks counts
claude plugin details regression-tests@regression-tests

# 3. Run inside any Rust or C# repo with a clean baseline
cd ~/path/to/my-rust-project
claude
> /regression-tests:regression-tests          # diff mode (vs. origin/main)
> /regression-tests:regression-tests src/parser.rs   # target mode
> /regression-tests:tdd "Parse RFC-2822 dates with timezone offsets"
```

The skill writes tests directly into your repo. All transient state lives under `.claude-regression/<run_id>/` (auto-gitignored on first run).

## Agents

Eleven specialist agents are shared between the two skills:

| Agent | Model | Role |
|---|---|---|
| `coverage-reviewer` | opus | Synthesis: diff/spec → locked work-unit contracts |
| `unit-test-author` | sonnet | Parallel team, unit-level test code |
| `integration-test-author` | opus | Reasoning-heavy boundary tests |
| `adversarial-vacuousness` | sonnet | Specialist: vacuous tests + test-mutation patterns |
| `adversarial-happy-path` | sonnet | Specialist: happy-path bias + edge cases |
| `adversarial-misalignment` | sonnet | Specialist: test ↔ contract alignment |
| `adversarial-synthesis` | opus | Synthesis over the three specialists' findings |
| `mutation-runner` | haiku | Mechanical: cargo-mutants / dotnet-stryker |
| `fuzz-harness-author` | opus | Reasoning-heavy fuzz harness design |
| `fuzz-runner` | haiku | Mechanical: cargo-fuzz / SharpFuzz |
| `implementation-author` | opus | TDD green-phase code writer |

See [docs/TECHNICAL.md#agent-dispatch-graph](docs/TECHNICAL.md#agent-dispatch-graph) for the full tool inventory and concurrency limits.

## Hooks

`hooks/hooks.json` enforces invariants automatically:

* **`UserPromptExpansion`** on the plugin's skill names → runs `regression-tests preflight` (detect-stack + baseline-check + lint-check). Blocks the skill if the baseline is red.
* **`PreToolUse`** on the `Agent` tool → scans prompts for forbidden strings (`--- a/`, `+++ b/`, `git diff`) before adversarial specialists spawn (defense-in-depth on top of their tool restrictions).
* **`PostToolUse`** on the `Agent` tool → auto-runs `verify-new-tests-compile` after each test-author returns, plus `run-new-tests` after the `implementation-author` returns. Blocks with diagnostics for retry.

`verify-no-test-mutation` is *not* a per-author hook (see [TECHNICAL.md#hook-lifecycle](docs/TECHNICAL.md#hook-lifecycle) for the rationale). The orchestrator runs it once at end-of-phase as an audit; the adversarial-vacuousness and adversarial-misalignment specialists provide primary cheat detection.

## Rust binary (`bin/regression-tests`)

A single CLI exposing the deterministic helpers:

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

The pre-built binary is committed at `bin/regression-tests.exe` (~3 MB Windows x86_64), so downstream consumers do **not** need a Rust toolchain to use the plugin.

## Status

Currently **Windows x86_64 only**. Cross-platform binaries (Linux, macOS) via a GitHub Actions matrix is future work. The Rust source itself has no platform-specific business logic outside `src/common/subprocess.rs`; the `kill_process_tree` POSIX path is stubbed and tracked.

## Prerequisites

**For basic use of the plugin** (skill orchestration + authoring + adversarial review): the shipped `bin/regression-tests.exe` is the only requirement — no toolchain needed.

**For full multi-phase regression testing on a Rust project under test**:

```bash
cargo install cargo-mutants --locked     # Phase 4a mutation testing (else static-only)
cargo install cargo-fuzz --locked        # Phase 4b fuzz harness/runners
rustup toolchain install nightly         # cargo-fuzz needs nightly for libFuzzer
cargo install cargo-llvm-cov --locked    # Phase 5 coverage delta
```

> Cosmetic note: `cargo fuzz --version` panics under some Windows consoles because cargo-fuzz v0.13.1 pulls in `is-terminal v0.4.1` (an old range-out-of-bounds bug in terminal-width probing). `cargo fuzz init` and `cargo fuzz run` are unaffected.

**For full multi-phase regression testing on a C# project under test**:

```bash
dotnet tool install -g dotnet-stryker                  # Phase 4a mutation testing
dotnet tool install -g SharpFuzz.CommandLine           # Phase 4b fuzzing
dotnet tool install -g dotnet-reportgenerator-globaltool   # Phase 5 coverage delta
```

The skills shell out to these tools when present and degrade gracefully when absent. Tooling status is recorded in `<run_id>/tooling.json` at the start of every run.

**For building this plugin from source** on Windows:

* **Rust toolchain** (`rustup` with `stable-x86_64-pc-windows-msvc`). Verify with `cargo --version` and `rustc --version`.
* **MSVC C++ Build Tools** + **Windows SDK** (Visual Studio Installer → Individual Components → "Windows 11 SDK" and "MSVC v143 build tools"). Required for `link.exe` and `kernel32.lib` during `cargo build`. See [CLAUDE.md](CLAUDE.md#toolchain-bootstrap-windows) for the vcvars bootstrap snippet.
* **`rustup component add rust-analyzer`** — only if you also have the `rust-analyzer-lsp` Claude Code plugin enabled. Without this component, the rustup proxy crashes when the LSP starts.

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

Then invoke `/regression-tests:regression-tests` or `/regression-tests:tdd` in any Rust or C# project's git working tree.

## Build from source

```bash
cargo build --release
cp target/release/regression-tests.exe bin/regression-tests.exe
```

The pre-built binary at `bin/regression-tests.exe` is committed to the repo so end users don't need a Rust toolchain. See [CLAUDE.md](CLAUDE.md) for the full toolchain bootstrap (vcvars sourcing, MSBuild env vars, etc.).

## License

MIT. See [LICENSE](LICENSE).
