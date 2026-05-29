# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A Claude Code plugin (not just a Rust crate). It ships **three skills** (`regression` — lock current behavior; `tdd` — drive new features test-first; `report-bug` — a lightweight bug-capture utility, the only one that is *not* a multi-agent orchestrator), **eleven specialist agents** plus `implementation-author`, **three hooks**, **workflow stage scripts** (`workflows/*.js`), and a **Rust CLI binary** (`straightjacket`) that together implement the multi-agent test-engineering workflow. The deterministic fan-out phases run as **dynamic-Workflow stages** when the `Workflow` tool is available, else as direct `Agent` dispatch. The Rust crate at the repo root is the *helper binary* (deterministic helpers + hook executor + `workflow-script` emitter) — not the plugin's primary output. The primary output is the skills + agents + hooks + workflow scripts that orchestrate Claude Code subagents.

> The `tdd` skill + `implementation-author` agent (failing-tests-first new-feature development) were removed in `477372c` and **reimplemented workflow-first** this session (`skills/tdd/SKILL.md`, `agents/implementation-author.md`). The `run-new-tests` name-survival, `target_stub_path`, and the `implementation-author` arm in `decide_post_agent` back them.

Read `README.md` for end-user info, and `docs/TECHNICAL.md` for the architecture deep-dive (phase flowcharts, agent dispatch graph, file lifecycle, extension recipes). The plan that drove the build lives at `~/.claude/plans/do-we-need-a-twinkly-bonbon.md` (476 lines, source of truth for design decisions).

## Build / test commands

### Toolchain bootstrap (Windows)

**Cargo requires MSVC `link.exe` + Windows SDK + vcvars sourced.** Without it, `cargo build` fails one of two ways:

- `link: extra operand` - Git Bash's `link.exe` shadows the MSVC linker on PATH
- `LNK1181: cannot open kernel32.lib` - Windows SDK lib paths absent from the env

**Preferred — git bash.** The repo ships **`scripts\cargo-msvc.cmd`**, which `vswhere`-resolves `vcvars64.bat` (so it survives VS edition/version upgrades — **VS18 / 2026** today, *not* the stale hardcoded 2022 path) and runs cargo with the MSVC env. From git bash:

```bash
cmd //c scripts\cargo-msvc.cmd test --lib
cmd //c scripts\cargo-msvc.cmd clippy --all-targets -- -D warnings
# keep any `| tail` on the BASH side — cmd has no `tail`
```

> Claude note: the wrapper dodges two load-bearing Windows gotchas. (1) Git bash's `/usr/bin/link.exe` shadows MSVC's linker (`link: extra operand`); running cargo *inside* `cmd` after vcvars uses cmd's PATH, where MSVC's link wins. (2) A space-containing `"C:\…\vcvars64.bat"` placed directly on a `cmd //c '…'` arg line gets MSYS-escaped to `\"C:\…\"` and cmd can't parse it — keeping the quoted path inside the `.cmd` avoids it. vcvars is located via `vswhere`, replacing the previously-hardcoded `…\2022\Community\…` path that no longer exists on this machine.

<details><summary>PowerShell alternative (vswhere-resolved)</summary>

```powershell
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$vcvars  = & $vswhere -latest -prerelease -find 'VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /c "`"$vcvars`" >NUL 2>&1 && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($matches[1])" -Value $matches[2] }
}
$env:MSBUILDDISABLENODEREUSE = '1'
```
</details>

### Standard commands

After bootstrap:

- `cargo check --all-targets` - Fast type-check, no codegen
- `cargo clippy --all-targets -- -D warnings` - Lint gate, must be clean
- `cargo test --lib` - Runs the 187 tests embedded in each module's `#[cfg(test)] mod tests`; ~3 seconds after first build
- `cargo test --lib commands::detect_stack` - Single-module run by qualified path

> Claude note: Cargo `test` takes exactly one filter positional - you can't pass two module paths in the same invocation. To run two modules, run two commands.

### Shipping the binary

The committed binaries are **per-platform**, named by Rust target triple, and dispatched by two launcher shims. Don't invoke a raw `bin/straightjacket-*` directly — call `straightjacket` (the launcher picks the host's binary).

- `bin/straightjacket` — POSIX `sh` launcher (mode 100755, LF-locked via `.gitattributes`); `uname`-detects OS/arch and `exec`s `straightjacket-<triple>`.
- `bin/straightjacket.cmd` — Windows launcher; `%PROCESSOR_ARCHITECTURE%`-detects arch and runs `straightjacket-<triple>.exe` (ARM64 falls back to the x64 build under emulation).
- `bin/straightjacket-<triple>[.exe]` — the actual binaries, e.g. `straightjacket-x86_64-pc-windows-msvc.exe`, `straightjacket-aarch64-apple-darwin`.

> Why both surfaces still work unchanged: hooks call `${CLAUDE_PLUGIN_ROOT}/bin/straightjacket` (extensionless → resolves the sh launcher on Unix, and `.cmd` via PATHEXT on Windows); skills call bare `straightjacket` on PATH (same resolution). Neither references a triple.

**Cross-platform builds are CI's job.** `.github/workflows/build-binaries.yml` cross-compiles the five targets natively (one runner per target — no `cross`), then commits the refreshed binaries back into `bin/` on `workflow_dispatch` or a `v*` tag (and attaches them to a GitHub Release on tag). To refresh the binaries, dispatch that workflow or cut a tag — don't hand-build all five. A local single-target build for quick iteration:

```bash
# Windows x64 example (uses the MSVC wrapper from "Toolchain bootstrap"):
cmd //c scripts\cargo-msvc.cmd build --release
cp target/release/straightjacket.exe bin/straightjacket-x86_64-pc-windows-msvc.exe
```

- `bin/straightjacket-<triple>[.exe]` ARE committed (~3MB each) - downstream plugin consumers don't have a Rust toolchain
- `target/` is gitignored

### LSP integration

If the `rust-analyzer-lsp` plugin is enabled, install the component:

```bash
rustup component add rust-analyzer
```

> Claude note: The rustup proxy at `~/.cargo/bin/rust-analyzer.exe` exits with code 1 when the component isn't installed - this surfaces as a Claude plugin LSP crash rather than a missing-binary error, so it's easy to misdiagnose.

## Optional dev tooling for dogfooding straightjacket on this crate

The skill in this plugin shells out to mutation/fuzz/coverage tools when present and degrades gracefully when absent (see `Phase 1 step 3` in SKILL.md). For an end-to-end run against this crate's own Rust source, install:

- `cargo install cargo-mutants --locked` — enables Phase 4a real mutation runners. Currently installed: v27.0.0. Absent → adversarial pass is static-only.
- `cargo install cargo-fuzz --locked` + `rustup toolchain install nightly` — enables Phase 4b fuzz harness/runners. Nightly is mandatory because libFuzzer instrumentation is nightly-only. Currently installed: cargo-fuzz v0.13.1; nightly rustc 1.97.0 (2026-05-12). Absent → Phase 4b skipped.
- `cargo install cargo-llvm-cov --locked` — enables Phase 5 coverage delta. Currently installed: v0.8.4.

Cosmetic gotcha: `cargo fuzz --version` panics on some Windows consoles because cargo-fuzz v0.13.1 pulls in `is-terminal v0.4.1` (range-out-of-bounds in terminal-width probing). The panic is harmless — `cargo fuzz init` and `cargo fuzz run` are unaffected. Use `cargo fuzz --version 2>&1 | Out-File ...` to read the version if needed.

C# equivalents (for dogfooding the C# code path of this plugin, not the plugin itself which is Rust-only): `dotnet tool install -g dotnet-stryker`, `dotnet tool install -g SharpFuzz.CommandLine`, `dotnet tool install -g dotnet-reportgenerator-globaltool`.

## Architecture: where the work lives

Six layers, each with a different cardinal rule:

1. **`bin/straightjacket` (launcher → `bin/straightjacket-<triple>`)** — the Rust CLI. Pure-data helpers (parsers, walkers, hashers) live as `pub fn` in `src/commands/*.rs` alongside a `pub fn run(args: Args) -> anyhow::Result<()>` shell. Split is enforced by testability: pure helpers get unit tests; `run` is the thin glue that calls helpers + prints JSON + sets exit code. When porting/extending a subcommand, extract a pure-data helper first, then write the test, then the `run` glue.

2. **`src/common/`** — shared infrastructure used by multiple commands. **`walk.rs`** uses `WalkDir::filter_entry` for descent-time directory pruning (the load-bearing perf invariant: a post-walk `.filter()` still descends into `target/` and reads every file). **`subprocess.rs::run_with_timeout`** uses (a) `Command::env` for per-child env (`MSBUILDDISABLENODEREUSE=1` etc. — never `std::env::set_var`, which mutates the parent process) and (b) `taskkill /F /T /PID` on Windows for process-tree kill, because plain `child.kill()` orphans grandchildren that inherit the stdio pipes (this is exactly the same hazard the env vars work around for MSBuild). **`json_io.rs`** writes pretty-printed JSON with a trailing newline. **`cargo_target.rs`** holds `resolve_cargo_target` (pure manifest-paths → `CargoTarget`: a root-level `Cargo.toml` → run with `--workspace`; a single nested crate → run from that crate's dir *without* `--workspace`; multiple nested with no root → `Ambiguous`, never a silent pick) and `cargo_invocation` (maps a `CargoTarget` + base cargo args → cwd + argv, inserting `--workspace` only for a real workspace). `baseline_check`/`lint_check`/`run_new_tests` use them to run cargo from the correct directory on nested-crate layouts instead of a blind `--workspace` from `repo_root`.

3. **`agents/*.md`** — 11 specialist subagent definitions + `implementation-author` (tdd's green phase). Each has YAML frontmatter (`name`, `description`, `tools`, `model`, `effort`) and a body that's the agent's role + procedure + output-contract. Tool restrictions are **load-bearing isolation guarantees**, not advisory: `adversarial-*` agents have no `Bash`/`PowerShell` so they cannot `git diff` even if their prompt tries to make them — and spike `wf_060d27f3` confirmed this restriction also holds for **workflow-spawned** agents, so diff-isolation survives the workflow path. The `PreToolUse` hook scans adversarial-agent prompts as defense-in-depth (Agent path only). When editing an agent, preserve the tool list unless you intend to change its isolation contract.

4. **`skills/regression/SKILL.md` + `skills/tdd/SKILL.md`** — the two orchestrator skills (`straightjacket:regression` locks behavior; `straightjacket:tdd` drives test-first). Both are **workflow-first thin launchers**: the main session owns the checkpoints + the single-writer `work-units.json` merge, and delegates each deterministic fan-out phase to a **dynamic-Workflow stage** (`straightjacket workflow-script <fanout|adversarial>` → the `Workflow` tool) when available, else direct `Agent` dispatch. **Cardinal Rule 0: you never write test/impl code yourself** — that's the multi-agent collapse failure mode the skills exist to prevent; dispatch the `*-author` specialists. The only files the orchestrator writes are `work-units.json`, `tooling.json`, scaffolded test projects, and the final summary.

   **`skills/report-bug/SKILL.md`** is the odd skill out — a *single-session, no-fan-out, no-agent* capture utility (Cardinal Rule 0 there is "capture fast, don't derail," not "don't write code"). It writes one tracked JSON ledger (`<repo>/.straightjacket/bugs.json`, schema `schemas/bug-record.schema.json`) in the consumer repo **before** any remote call, then opt-in-mirrors to a GitHub issue (`gh` CLI primary, github MCP fallback) and/or a Jira ticket (atlassian MCP, reusing `atlassian:triage-issue`'s `getAccessibleAtlassianResources → getVisibleJiraProjects → createJiraIssue` ordering). It's wired as a side-call from the `surfaced_bug` branches of both orchestrator skills, and its three bridge fields (`suspect_files`/`suspect_symbol`/`intended_behavior_seed`) map onto `coverage-reviewer`'s `target_file`/`target_symbol`/`intended_behavior` so a later run can lift a parked bug into a test work unit. The ledger is **tracked/committed** — distinct from the gitignored `.claude-regression/` run state; don't gitignore it.

5. **`hooks/hooks.json`** — three hook events: `UserPromptExpansion` matcher fires `straightjacket preflight` on skill invocation; `PreToolUse Agent` runs `straightjacket hook pre-adversarial` (scans prompts for `--- a/`, `+++ b/`, `git diff`); `PostToolUse Agent` runs `straightjacket hook post-agent` (dispatches `verify-new-tests-compile` after test authors). `verify-no-test-mutation` is deliberately NOT a per-author hook — it produced false positives on Rust source files with inline `#[cfg(test)] mod tests`; the orchestrator runs it once at end-of-phase as an audit and relies on `adversarial-vacuousness` / `adversarial-misalignment` specialists for primary cheat detection. Hook event types and JSON shapes live in `src/commands/hook.rs::HookEvent` / `HookDecision`. **The `Agent` hooks do NOT fire for workflow-spawned agents** — in the workflow path, diff-isolation rests on the agents' tool restrictions and the orchestrator runs the verify/audit steps itself; the `UserPromptExpansion` preflight is unaffected (it fires on skill invocation in the main session).

6. **`workflows/*.js`** — shipped dynamic-Workflow stage scripts (`adversarial`, `fanout`), `include_str!`'d into the binary and emitted by `straightjacket workflow-script <stage>`. A SKILL reads one (via the CLI), fills `args` bindings (work units, stack, mode — **never the diff**), and runs it through the `Workflow` tool; the script fans out our custom agents (`parallel()` + `agent({agentType})`) and returns a compact structured result the main session merges. The scripts hold the *deterministic choreography* (parallel / caps / synthesis / iterate); the SKILL holds the prompts + judgment + checkpoints. Workflows are NOT a plugin-discovered component, so the **binary-emit** path is how they reach an installed plugin (vs. `.claude/workflows/`, which wouldn't ship).

The Rust binary is also the *hook executor* — `straightjacket hook <event>` reads stdin JSON, decides via pure functions in `hook.rs`, and emits the Claude-expected response shape. Decision logic is unit-tested; the hook shell is thin.

## Plugin packaging (gotchas worth recording)

- **`.claude-plugin/marketplace.json`** must exist for `claude plugin marketplace add <path-or-url>` to register this repo. The plugin's `source` field MUST be `{"source": "url", "url": "..."}`, **not** `"git-subdir"` with `path: "."`. The latter generates a sparse-checkout filter (`/* + !/*/`) that excludes every subdirectory — the install path ends up with only root-level files (no `agents/`, `skills/`, `bin/`). The `url` source clones the full repo.
- **The `git` source type is not supported by current Claude Code versions** — install fails with `This plugin uses a source type your Claude Code version does not support`. Use `url`.
- **`plugin.json` doesn't enumerate skills/agents/hooks** — Claude Code discovers them by convention (`skills/<name>/SKILL.md`, `agents/<name>.md`, `hooks/hooks.json`). Don't add manifest entries for them; they're auto-discovered.
- **`claude plugin details <name>@<marketplace>`** shows the inventory after install. If it reports 0 skills/agents/hooks but the install succeeded, the actual files didn't land on disk (almost always the sparse-checkout pitfall above).

## Testing invariants to preserve

- **`cargo test --lib` parallelism + env-mutating tests**: cargo runs tests in parallel threads of the same process. Tests that mutate `std::env` (e.g., setting a sentinel value in the parent) will race against any other test that reads the same variable. The current `sets_msbuild_env_var_on_child_only_not_parent` test works only because no test mutates `MSBUILDDISABLENODEREUSE` directly — a stronger sentinel-based variant was attempted and reverted (commit history). If you need to add env-touching tests, add `serial_test` as a dev-dependency and annotate.
- **`subprocess.rs::tests::timeout_kills_entire_process_tree`** is Windows-gated (`#[cfg(windows)]`) and uses a unique `-w` ping tag (`88_000_000 + (pid % 100_000)`) to detect orphans via `Get-CimInstance Win32_Process`. The Linux/macOS implementation of `kill_process_tree` is stubbed; add an equivalent (`kill -- -$pgid` or similar) when cross-platform work begins.
- **Schema-shape tests** like `stack_serializes_as_lowercase_string` guard the JSON output contract that the SKILL.md orchestrator consumes. Don't relax those — the orchestrator parses by exact field names and lowercase enum values (`"rust" | "csharp" | "both" | "none"`, `"unit" | "integration"`, etc.).
- **No-silent-green guarantees (Finding 2)**: `verify_no_test_mutation` exposes `no_files_checked` (true when 0 files were snapshotted) and `run_new_tests` exposes `nothing_to_run` (true when 0 units collected) — both **orthogonal to `clean`/success** so the orchestrator branches on "checked nothing" loudly instead of mistaking a 0-check for a pass. `run_new_tests::name_survival(expected_red_names, green_statuses)` is the behavioral immutability backstop: a previously-RED test that goes `missing` (Unknown/absent → deleted, renamed, or `#[ignore]`-d) fails the survival gate (`ok == missing.is_empty() && regressed.is_empty() && !nothing_to_verify`; empty expected set → `nothing_to_verify` + not ok). `collect_units_by_name` selects units by name **ignoring the manually-flipped `status`** (so the gate can't no-op) and accepts the `{"work_units":[...]}` wrapper. Don't reintroduce a silent 0-check path.

## Git workflow notes

- The per-platform `bin/straightjacket-<triple>[.exe]` binaries (~3MB each) plus the `bin/straightjacket` / `bin/straightjacket.cmd` launchers are committed. Don't gitignore them. `.gitattributes` keeps the sh launcher LF-only (a CRLF shebang breaks `/bin/sh`) and marks the binaries `binary`.
- `target/`, `.claude-regression/` (per-run state from the skills themselves), and `2026-*-*.txt` (session transcript files) are gitignored.
- The repo uses `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>` in commits where Claude Code contributed.

## Available memory

`~/.claude/projects/C--Users-KemonoNeco-Code-regression-tests-plugin/memory/` contains several memory files relevant to this repo (project scope, cargo build env, several feedback notes). The index at `MEMORY.md` pre-loads into the session context automatically.
