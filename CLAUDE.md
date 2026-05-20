# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A Claude Code plugin (not just a Rust crate). It ships **two skills**, **eleven specialist agents**, **three hooks**, and a **Rust CLI binary** that together implement two multi-agent test workflows: `regression-tests` (lock current behavior) and `tdd` (drive new feature development with failing-tests-first). The Rust crate at the repo root is the *helper binary* for the plugin — not the plugin's primary output. The primary output is the skills + agents + hooks that orchestrate Claude Code subagents.

Read `README.md` for end-user info, and `docs/TECHNICAL.md` for the architecture deep-dive (phase flowcharts, agent dispatch graph, file lifecycle, extension recipes). The plan that drove the build lives at `~/.claude/plans/do-we-need-a-twinkly-bonbon.md` (476 lines, source of truth for design decisions).

## Build / test commands

### Toolchain bootstrap (Windows)

**Cargo requires MSVC `link.exe` + Windows SDK + vcvars sourced.** Without it, `cargo build` fails one of two ways:

- `link: extra operand` - Git Bash's `link.exe` shadows the MSVC linker on PATH
- `LNK1181: cannot open kernel32.lib` - Windows SDK lib paths absent from the env

Source the environment at the top of every Cargo invocation:

```powershell
# Installer dir on PATH BEFORE vcvars runs - vcvars64.bat shells out to vswhere.exe
$installerDir = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer'
$env:PATH = "$installerDir;$env:PATH"

# Source vcvars64.bat into the current PowerShell session
$vcvars = 'C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /c "`"$vcvars`" >NUL 2>&1 && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($matches[1])" -Value $matches[2] }
}

# Prevent MSBuild node-reuse from holding files open across builds
$env:MSBUILDDISABLENODEREUSE = '1'
```

> Claude note: The Installer-dir-before-vcvars ordering is load-bearing. `vcvars64.bat` calls `vswhere.exe` internally to locate the active VS install; if `vswhere` isn't on PATH first, vcvars silently picks the wrong toolchain (or none) and the resulting env is incomplete.

### Standard commands

After bootstrap:

- `cargo check --all-targets` - Fast type-check, no codegen
- `cargo clippy --all-targets -- -D warnings` - Lint gate, must be clean
- `cargo test --lib` - Runs the 145 tests embedded in each module's `#[cfg(test)] mod tests`; ~3 seconds after first build
- `cargo test --lib commands::detect_stack` - Single-module run by qualified path

> Claude note: Cargo `test` takes exactly one filter positional - you can't pass two module paths in the same invocation. To run two modules, run two commands.

### Shipping the binary

```bash
cargo build --release
cp target/release/regression-tests.exe bin/regression-tests.exe
```

- `bin/regression-tests.exe` IS committed (~3MB) - downstream plugin consumers don't have a Rust toolchain
- `target/` is gitignored

### LSP integration

If the `rust-analyzer-lsp` plugin is enabled, install the component:

```bash
rustup component add rust-analyzer
```

> Claude note: The rustup proxy at `~/.cargo/bin/rust-analyzer.exe` exits with code 1 when the component isn't installed - this surfaces as a Claude plugin LSP crash rather than a missing-binary error, so it's easy to misdiagnose.

## Optional dev tooling for dogfooding regression-tests/tdd on this crate

The skills in this plugin shell out to mutation/fuzz/coverage tools when present and degrade gracefully when absent (see `Phase 1 step 3` in either SKILL.md). For an end-to-end run against this crate's own Rust source, install:

- `cargo install cargo-mutants --locked` — enables Phase 4a real mutation runners. Currently installed: v27.0.0. Absent → adversarial pass is static-only.
- `cargo install cargo-fuzz --locked` + `rustup toolchain install nightly` — enables Phase 4b fuzz harness/runners. Nightly is mandatory because libFuzzer instrumentation is nightly-only. Currently installed: cargo-fuzz v0.13.1; nightly rustc 1.97.0 (2026-05-12). Absent → Phase 4b skipped.
- `cargo install cargo-llvm-cov --locked` — enables Phase 5 coverage delta. Currently installed: v0.8.4.

Cosmetic gotcha: `cargo fuzz --version` panics on some Windows consoles because cargo-fuzz v0.13.1 pulls in `is-terminal v0.4.1` (range-out-of-bounds in terminal-width probing). The panic is harmless — `cargo fuzz init` and `cargo fuzz run` are unaffected. Use `cargo fuzz --version 2>&1 | Out-File ...` to read the version if needed.

C# equivalents (for dogfooding the C# code path of this plugin, not the plugin itself which is Rust-only): `dotnet tool install -g dotnet-stryker`, `dotnet tool install -g SharpFuzz.CommandLine`, `dotnet tool install -g dotnet-reportgenerator-globaltool`.

## Architecture: where the work lives

Five layers, each with a different cardinal rule:

1. **`bin/regression-tests.exe`** — the Rust CLI. Pure-data helpers (parsers, walkers, hashers) live as `pub fn` in `src/commands/*.rs` alongside a `pub fn run(args: Args) -> anyhow::Result<()>` shell. Split is enforced by testability: pure helpers get unit tests; `run` is the thin glue that calls helpers + prints JSON + sets exit code. When porting/extending a subcommand, extract a pure-data helper first, then write the test, then the `run` glue.

2. **`src/common/`** — shared infrastructure used by multiple commands. **`walk.rs`** uses `WalkDir::filter_entry` for descent-time directory pruning (the load-bearing perf invariant: a post-walk `.filter()` still descends into `target/` and reads every file). **`subprocess.rs::run_with_timeout`** uses (a) `Command::env` for per-child env (`MSBUILDDISABLENODEREUSE=1` etc. — never `std::env::set_var`, which mutates the parent process) and (b) `taskkill /F /T /PID` on Windows for process-tree kill, because plain `child.kill()` orphans grandchildren that inherit the stdio pipes (this is exactly the same hazard the env vars work around for MSBuild). **`json_io.rs`** writes pretty-printed JSON with a trailing newline.

3. **`agents/*.md`** — 11 specialist subagent definitions. Each has YAML frontmatter (`name`, `description`, `tools`, `model`, `effort`) and a body that's the agent's role + procedure + output-contract. Tool restrictions are **load-bearing isolation guarantees**, not advisory: `adversarial-*` agents have no `Bash`/`PowerShell` so they cannot `git diff` even if their prompt tries to make them. The plugin's `PreToolUse` hook scans adversarial-agent prompts as defense-in-depth. When editing an agent, preserve the tool list unless you intend to change its isolation contract.

4. **`skills/regression-tests/SKILL.md`** and **`skills/tdd/SKILL.md`** — orchestrator playbooks. The main Claude session executes every phase; specialists are dispatched via `Agent` tool calls. **Cardinal Rule 0 (from both SKILL.md files): you never write test code yourself** — that's the multi-agent collapse failure mode the skills exist to prevent. If you find yourself reaching for Write/Edit on a `_test.rs` or `Tests.cs`, stop and dispatch the appropriate `unit-test-author` / `integration-test-author` / `implementation-author`. The only files the orchestrator writes are `work-units.json`, `tooling.json`, scaffolded test projects, and the final summary.

5. **`hooks/hooks.json`** — three hook events: `UserPromptExpansion` matcher fires `regression-tests preflight` on skill invocation; `PreToolUse Agent` runs `regression-tests hook pre-adversarial` (scans prompts for `--- a/`, `+++ b/`, `git diff`); `PostToolUse Agent` runs `regression-tests hook post-agent` (dispatches `verify-new-tests-compile` after test authors, plus `run-new-tests` after `implementation-author`). `verify-no-test-mutation` is deliberately NOT a per-author hook — it produced false positives on Rust source files with inline `#[cfg(test)] mod tests`; the orchestrator runs it once at end-of-phase as an audit and relies on `adversarial-vacuousness` / `adversarial-misalignment` specialists for primary cheat detection. Hook event types and JSON shapes live in `src/commands/hook.rs::HookEvent` / `HookDecision`.

The Rust binary is also the *hook executor* — `regression-tests hook <event>` reads stdin JSON, decides via pure functions in `hook.rs`, and emits the Claude-expected response shape. Decision logic is unit-tested; the hook shell is thin.

## Plugin packaging (gotchas worth recording)

- **`.claude-plugin/marketplace.json`** must exist for `claude plugin marketplace add <path-or-url>` to register this repo. The plugin's `source` field MUST be `{"source": "url", "url": "..."}`, **not** `"git-subdir"` with `path: "."`. The latter generates a sparse-checkout filter (`/* + !/*/`) that excludes every subdirectory — the install path ends up with only root-level files (no `agents/`, `skills/`, `bin/`). The `url` source clones the full repo.
- **The `git` source type is not supported by current Claude Code versions** — install fails with `This plugin uses a source type your Claude Code version does not support`. Use `url`.
- **`plugin.json` doesn't enumerate skills/agents/hooks** — Claude Code discovers them by convention (`skills/<name>/SKILL.md`, `agents/<name>.md`, `hooks/hooks.json`). Don't add manifest entries for them; they're auto-discovered.
- **`claude plugin details <name>@<marketplace>`** shows the inventory after install. If it reports 0 skills/agents/hooks but the install succeeded, the actual files didn't land on disk (almost always the sparse-checkout pitfall above).

## Testing invariants to preserve

- **`cargo test --lib` parallelism + env-mutating tests**: cargo runs tests in parallel threads of the same process. Tests that mutate `std::env` (e.g., setting a sentinel value in the parent) will race against any other test that reads the same variable. The current `sets_msbuild_env_var_on_child_only_not_parent` test works only because no test mutates `MSBUILDDISABLENODEREUSE` directly — a stronger sentinel-based variant was attempted and reverted (commit history). If you need to add env-touching tests, add `serial_test` as a dev-dependency and annotate.
- **`subprocess.rs::tests::timeout_kills_entire_process_tree`** is Windows-gated (`#[cfg(windows)]`) and uses a unique `-w` ping tag (`88_000_000 + (pid % 100_000)`) to detect orphans via `Get-CimInstance Win32_Process`. The Linux/macOS implementation of `kill_process_tree` is stubbed; add an equivalent (`kill -- -$pgid` or similar) when cross-platform work begins.
- **Schema-shape tests** like `stack_serializes_as_lowercase_string` guard the JSON output contract that the SKILL.md orchestrator consumes. Don't relax those — the orchestrator parses by exact field names and lowercase enum values (`"rust" | "csharp" | "both" | "none"`, `"unit" | "integration"`, etc.).

## Git workflow notes

- `bin/regression-tests.exe` is committed (~3MB Windows binary). Don't gitignore it.
- `target/`, `.claude-regression/` (per-run state from the skills themselves), and `2026-*-*.txt` (session transcript files) are gitignored.
- The repo uses `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>` in commits where Claude Code contributed.

## Available memory

`~/.claude/projects/C--Users-KemonoNeco-Code-regression-tests-plugin/memory/` contains several memory files relevant to this repo (project scope, cargo build env, several feedback notes). The index at `MEMORY.md` pre-loads into the session context automatically.
