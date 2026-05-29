# straightjacket plugin

A Claude Code multi-agent test-engineering plugin — *it does sanity tests*. Built as **composable chunks** (specialist agents + workflow stages + a Rust CLI) that **thin skills** compose. The flagship **`tdd`** skill drives new features test-first (spec → red → adversarial-on-red → green → mutation) under a savepoint discipline; a companion **`report-bug`** skill captures bugs found along the way into a tracked ledger (and optional GitHub/Jira tickets) that a later run can lift into tests. It hardens tests against four failure modes — **happy-path bias**, **vacuous assertions**, **test-mutation cheats**, and **test-contract misalignment** — using parallel specialist subagents, mutation testing, and (optionally) fuzzing, run as dynamic-Workflow stages when the `Workflow` tool is available (else direct `Agent` dispatch).

> All seven skills share one engine — the specialist agents, the `fanout` / `adversarial` / `audit` / `tdd-cycle` workflow stages, and the `straightjacket` Rust CLI — documented in [docs/STAGES.md](docs/STAGES.md). The decomposition retired the old monolithic `regression` command; its machinery survives as those reusable chunks.

* **Looking for usage?** Jump to [Quickstart](#quickstart).
* **Looking for the shared chunks?** Read [docs/STAGES.md](docs/STAGES.md) for the stage/agent vocabulary.
* **Looking for design?** Read [docs/TECHNICAL.md](docs/TECHNICAL.md) for the architecture deep-dive.
* **Contributing to the plugin?** Read [CLAUDE.md](CLAUDE.md) for the load-bearing invariants.

## Skills

| Slash command | Purpose |
|---|---|
| `/straightjacket:tdd` | Drive a new feature test-first from a spec: red → adversarial-on-red → green → mutation, under a savepoint discipline. |
| `/straightjacket:audit` | Find latent defects in source — bugs / dead code / false docs / performance / security / concurrency / error-handling — via isolated LLM lenses + mechanical tool-runners, refute-filtered. Analysis-only (no test-writing). |
| `/straightjacket:debug` | Root-cause ONE bug from a green tree and return its cause + a reproduction. Diagnoses; does not fix. |
| `/straightjacket:triage` | Close the loop: route a captured bug → debug (if needed) → tdd fix-mode (failing test for correct behavior + fix) → flip it to `fixed`. |
| `/straightjacket:fuzz` | Stand-alone fuzzing: write harnesses, run them, mine each crash into a deterministic regression test. |
| `/straightjacket:mutation` | Stand-alone mutation testing: surface surviving mutants as under-tested-behavior proposals for tdd to cover. |
| `/straightjacket:report-bug` | Capture a found bug to a tracked local ledger (`.straightjacket/bugs.json`), then optionally mirror it to a GitHub issue and/or Jira ticket. Local-first, opt-in remotes — file a bug *without derailing* the work in progress, and feed it back as test context later. |

All seven skills compose one shared engine — the specialist agents, the `fanout` / `adversarial` / `audit` / `tdd-cycle` workflow stages, and the `straightjacket` CLI — documented in [docs/STAGES.md](docs/STAGES.md).

Shared pipeline shape: **coverage planning → parallel authoring → adversarial team review (+ synthesis) → mutation testing → optional fuzzing**, run as dynamic-Workflow stages when the `Workflow` tool is available, else direct `Agent` dispatch.

## Quickstart

```bash
# 1. Install (via Claude plugin marketplace)
claude plugin marketplace add https://github.com/KemonoNeco/regression-tests-plugin
claude plugin install straightjacket@straightjacket

# 2. Verify install - should report skills/agents/hooks counts
claude plugin details straightjacket@straightjacket

# 3. Run inside any Rust or C# repo with a clean baseline
cd ~/path/to/my-rust-project
claude
> /straightjacket:tdd "add a function that parses a header and rejects inputs over 4 KiB"
```

The skill writes tests (and, in tdd, implementation) directly into your repo. All transient state lives under `.claude-regression/<run_id>/` (auto-gitignored on first run); the bug ledger at `.straightjacket/bugs.json` is tracked/committed.

## Agents

The specialist agents that make up the workflow:

| Agent | Model | Role |
|---|---|---|
| `coverage-reviewer` | opus | Synthesis: diff/target/spec → locked work-unit contracts |
| `unit-test-author` | sonnet | Parallel team, unit-level test code |
| `integration-test-author` | opus | Reasoning-heavy boundary tests |
| `adversarial-vacuousness` | opus | Specialist: vacuous tests + test-mutation patterns |
| `adversarial-happy-path` | opus | Specialist: happy-path bias + edge cases |
| `adversarial-misalignment` | opus | Specialist: test ↔ contract alignment |
| `adversarial-synthesis` | opus | Synthesis over the three specialists' findings |
| `mutation-runner` | haiku | Mechanical: cargo-mutants / dotnet-stryker |
| `fuzz-harness-author` | opus | Reasoning-heavy fuzz harness design |
| `fuzz-runner` | haiku | Mechanical: cargo-fuzz / SharpFuzz |
| `implementation-author` | opus | Fills stubs (tdd green) or fixes buggy source (fix mode); never edits tests |

The `audit-<lens>` finders + `audit-runner`/`audit-refuter`/`audit-synthesis` (for `audit`) and `root-cause-analyst` (for `debug`) are added as those skills land. See [docs/STAGES.md#specialist-agent-roster](docs/STAGES.md#specialist-agent-roster) for the full tool inventory and concurrency limits.

## Hooks

`hooks/hooks.json` enforces invariants automatically:

* **`UserPromptExpansion`** on the green-baseline skill names (`tdd`, `mutation`, `fuzz`, `debug`) → runs `straightjacket hook preflight`, the gate point for a clean baseline. (`audit` is read-only and `triage` is a router, so they skip it.)
* **`PreToolUse`** on the `Agent` tool → scans prompts for forbidden strings (`--- a/`, `+++ b/`, `git diff`) before adversarial specialists spawn (defense-in-depth on top of their tool restrictions).
* **`PostToolUse`** on the `Agent` tool → auto-runs `verify-new-tests-compile` after each test-author returns. Blocks with diagnostics for retry.

`verify-no-test-mutation` is *not* a per-author hook (see [TECHNICAL.md#hook-lifecycle](docs/TECHNICAL.md#hook-lifecycle) for the rationale). The orchestrator runs it once at end-of-phase as an audit; the adversarial-vacuousness and adversarial-misalignment specialists provide primary cheat detection.

## Rust binary (`bin/straightjacket`)

A single CLI exposing the deterministic helpers:

```
straightjacket detect-stack
straightjacket baseline-check  --repo-root <p> --stack <s> --log-dir <d>
straightjacket lint-check      --repo-root <p> --stack <s> --log-dir <d>
straightjacket snapshot-tests  --repo-root <p> --out-file <p>
straightjacket verify-no-test-mutation --repo-root <p> --snapshot-file <p>
straightjacket verify-new-tests-compile --repo-root <p> --work-units <p> --stack <s>
straightjacket fuzz-setup      --repo-root <p> --stack <s>
straightjacket reproducer-to-test --repro <p> --target <name> --stack <s> --work-units <p>
straightjacket run-new-tests   --repo-root <p> --work-units <p> --stack <s> [--expect=fail]
straightjacket preflight       (combined: detect-stack + baseline-check + lint-check)
straightjacket hook <event>    (hook entry points: preflight | pre-adversarial | post-agent)
straightjacket workflow-script <stage>   (emits the fanout/adversarial stage scripts)
```

Pre-built per-platform binaries are committed under `bin/` (named by Rust target triple, ~3 MB each), so downstream consumers do **not** need a Rust toolchain. You always invoke `straightjacket`; two launcher shims dispatch to the right binary for the host:

- `bin/straightjacket` — POSIX `sh` launcher (Linux/macOS, any arch)
- `bin/straightjacket.cmd` — Windows launcher
- `bin/straightjacket-<triple>[.exe]` — the actual binaries the launchers pick from

## Status

**Cross-platform.** `.github/workflows/build-binaries.yml` cross-compiles the binary on a native runner per target — `x86_64`/`aarch64` Linux, `x86_64`/`aarch64` macOS, and `x86_64` Windows — and commits the refreshed binaries back into `bin/` on a manual dispatch or a `v*` release tag. Windows-on-ARM uses the x64 build under emulation. The only platform-specific business logic is in `src/common/subprocess.rs`; the `kill_process_tree` POSIX path is stubbed and tracked.

## Prerequisites

**For basic use of the plugin** (skill orchestration + authoring + adversarial review): the shipped `bin/` binaries are the only requirement - no toolchain needed.

**For the full multi-phase workflow on a Rust project under test**:

```bash
cargo install cargo-mutants --locked     # mutation testing (else static-only)
cargo install cargo-fuzz --locked        # fuzz harness/runners
rustup toolchain install nightly         # cargo-fuzz needs nightly for libFuzzer
cargo install cargo-llvm-cov --locked    # coverage delta
```

> Cosmetic note: `cargo fuzz --version` panics under some Windows consoles because cargo-fuzz v0.13.1 pulls in `is-terminal v0.4.1` (an old range-out-of-bounds bug in terminal-width probing). `cargo fuzz init` and `cargo fuzz run` are unaffected.

**For the full multi-phase workflow on a C# project under test**:

```bash
dotnet tool install -g dotnet-stryker                  # mutation testing
dotnet tool install -g SharpFuzz.CommandLine           # fuzzing
dotnet tool install -g dotnet-reportgenerator-globaltool   # coverage delta
```

The skills shell out to these tools when present and degrade gracefully when absent. Tooling status is recorded in `<run_id>/tooling.json` at the start of every run.

**For building this plugin from source** on Windows:

* **Rust toolchain** (`rustup` with `stable-x86_64-pc-windows-msvc`). Verify with `cargo --version` and `rustc --version`.
* **MSVC C++ Build Tools** + **Windows SDK** (Visual Studio Installer → Individual Components → "Windows 11 SDK" and "MSVC v143 build tools"). Required for `link.exe` and `kernel32.lib` during `cargo build`. See [CLAUDE.md](CLAUDE.md#toolchain-bootstrap-windows) for the vcvars bootstrap snippet.
* **`rustup component add rust-analyzer`** - only if you also have the `rust-analyzer-lsp` Claude Code plugin enabled. Without this component, the rustup proxy crashes when the LSP starts.

## Install

**Recommended** - via Claude plugin marketplace:

```bash
claude plugin marketplace add https://github.com/KemonoNeco/regression-tests-plugin
claude plugin install straightjacket@straightjacket
```

**Local dev** - point Claude Code at a checkout:

```bash
claude --plugin-dir ~/Code/regression-tests-plugin
```

Then invoke `/straightjacket:tdd` in any Rust or C# project's git working tree.

## Build from source

The committed binaries are produced by CI (`.github/workflows/build-binaries.yml`) — to refresh them across all platforms, trigger that workflow (`workflow_dispatch`) or push a `v*` tag. For a local single-target build:

```bash
cargo build --release
# name the output by your host triple so the launcher finds it, e.g. Windows x64:
cp target/release/straightjacket.exe bin/straightjacket-x86_64-pc-windows-msvc.exe
```

The per-platform binaries under `bin/` are committed so end users don't need a Rust toolchain. See [CLAUDE.md](CLAUDE.md) for the full toolchain bootstrap (vcvars sourcing, MSBuild env vars, etc.) and the launcher/CI details.

## License

MIT. See [LICENSE](LICENSE).
