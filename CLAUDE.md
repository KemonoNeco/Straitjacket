# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A Claude Code plugin (not just a Rust crate). It is built as **composable chunks** — **specialist agents** (`agents/*.md`), **workflow stage scripts** (`workflows/*.js`), **hooks**, and a **Rust CLI binary** (`straitjacket`) — that **thin skills** compose. Seven skills over one engine: **`tdd`** (drive new features test-first), **`report-bug`** (lightweight bug-capture — the only one that is *not* a multi-agent orchestrator), **`audit`** (find latent defects, analysis-only), **`fuzz`** + **`mutation`** (stand-alone dynamic analysis), and the **`debug`** → **`triage`** loop (root-cause a bug from green, then drive it to a tested fix in tdd fix-mode). The deterministic fan-out phases run as **dynamic-Workflow stages** when the `Workflow` tool is available, else as direct `Agent` dispatch. The Rust crate at the repo root is the *helper binary* (deterministic helpers + hook executor + `workflow-script` emitter) — not the plugin's primary output. The primary output is the skills + agents + hooks + workflow scripts that orchestrate Claude Code subagents.

> **`regression` was retired as a command** (this refactor): its reusable machinery — the `coverage-reviewer` diff/target modes, the author teams, adversarial-on-tests, and the `fanout`/`adversarial` stages — survives as chunks documented in [`docs/STAGES.md`](docs/STAGES.md), consumed by `tdd` (fix mode), `triage`, and `audit`. A `characterize` launcher could re-expose diff-mode later; none ships now.

Read `README.md` for end-user info, `docs/STAGES.md` for the shared-chunk vocabulary, and `docs/TECHNICAL.md` for the architecture deep-dive. The decomposition plan driving this refactor lives at `~/.claude/plans/make-a-plan-instead-calm-pebble.md` (the earlier build plan is `~/.claude/plans/do-we-need-a-twinkly-bonbon.md`).

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

The committed binaries are **per-platform**, named by Rust target triple, and dispatched by two launcher shims. Don't invoke a raw `bin/straitjacket-*` directly — call `straitjacket` (the launcher picks the host's binary).

- `bin/straitjacket` — POSIX `sh` launcher (mode 100755, LF-locked via `.gitattributes`); `uname`-detects OS/arch and `exec`s `straitjacket-<triple>`.
- `bin/straitjacket.cmd` — Windows launcher; `%PROCESSOR_ARCHITECTURE%`-detects arch and runs `straitjacket-<triple>.exe` (ARM64 falls back to the x64 build under emulation).
- `bin/straitjacket-<triple>[.exe]` — the actual binaries, e.g. `straitjacket-x86_64-pc-windows-msvc.exe`, `straitjacket-aarch64-apple-darwin`.

> Why both surfaces still work unchanged: hooks call `${CLAUDE_PLUGIN_ROOT}/bin/straitjacket` (extensionless). It resolves to the **sh launcher** on Unix *and under git-bash / MSYS / Cygwin on Windows* — MSYS does **not** apply PATHEXT, so the sh launcher itself must (and now does) handle `MINGW*`/`MSYS*`/`CYGWIN*` `uname` output and dispatch the windows-msvc `.exe`. Under cmd.exe/PowerShell, PATHEXT resolves the `.cmd` launcher instead. Skills call bare `straitjacket` on PATH (same resolution). Neither references a triple. (Earlier this doc claimed only the `.cmd` handled Windows — that was wrong for the git-bash path and silently no-op'd the hooks there.)

**Cross-platform builds are CI's job.** `.github/workflows/build-binaries.yml` cross-compiles the five targets natively (one runner per target — no `cross`), then commits the refreshed binaries back into `bin/` on `workflow_dispatch` or a `v*` tag (and attaches them to a GitHub Release on tag). To refresh the binaries, dispatch that workflow or cut a tag — don't hand-build all five. A local single-target build for quick iteration:

```bash
# Windows x64 example (uses the MSVC wrapper from "Toolchain bootstrap"):
cmd //c scripts\cargo-msvc.cmd build --release
cp target/release/straitjacket.exe bin/straitjacket-x86_64-pc-windows-msvc.exe
```

- `bin/straitjacket-<triple>[.exe]` ARE committed (~3MB each) - downstream plugin consumers don't have a Rust toolchain
- `target/` is gitignored

> **Edited a `workflows/*.js` script? You MUST rebuild + re-commit the host binary.** The scripts are `include_str!`'d into the binary at build time (`src/commands/workflow_script.rs`), so a source-level change does NOT ship until the binary is rebuilt. The embed-freshness gate `workflow_script::tests::committed_binary_embeds_current_workflow_scripts` (issue #49) enforces this: it execs the **committed** host-triple `bin/straitjacket-*` and diffs each `workflow-script <stage>` against `workflows/<stage>.js` (EOL-normalized), so `cargo test --lib` goes RED on a stale committed binary. `build-binaries.yml` now also triggers its PR build+test on `workflows/**` / `bin/**` (not just `src/**`), so the gate fires on JS-only PRs. The other four targets still resync via CI (dispatch / `v*` tag); the host (Windows) binary is the one you rebuild locally and the only one the gate checks on this machine.

### LSP integration

If the `rust-analyzer-lsp` plugin is enabled, install the component:

```bash
rustup component add rust-analyzer
```

> Claude note: The rustup proxy at `~/.cargo/bin/rust-analyzer.exe` exits with code 1 when the component isn't installed - this surfaces as a Claude plugin LSP crash rather than a missing-binary error, so it's easy to misdiagnose.

## Optional dev tooling for dogfooding straitjacket on this crate

The skill in this plugin shells out to mutation/fuzz/coverage tools when present and degrades gracefully when absent (see `Phase 1 step 3` in SKILL.md). For an end-to-end run against this crate's own Rust source, install:

- `cargo install cargo-mutants --locked` — enables Phase 4a real mutation runners. Currently installed: v27.0.0. Absent → adversarial pass is static-only.
- `cargo install cargo-fuzz --locked` + `rustup toolchain install nightly` — enables Phase 4b fuzz harness/runners. Nightly is mandatory because libFuzzer instrumentation is nightly-only. Currently installed: cargo-fuzz v0.13.1; nightly rustc 1.97.0 (2026-05-12). Absent → Phase 4b skipped.
- `cargo install cargo-llvm-cov --locked` — enables Phase 5 coverage delta. Currently installed: v0.8.4.

Cosmetic gotcha: `cargo fuzz --version` panics on some Windows consoles because cargo-fuzz v0.13.1 pulls in `is-terminal v0.4.1` (range-out-of-bounds in terminal-width probing). The panic is harmless — `cargo fuzz init` and `cargo fuzz run` are unaffected. Use `cargo fuzz --version 2>&1 | Out-File ...` to read the version if needed.

C# equivalents (for dogfooding the C# code path of this plugin, not the plugin itself which is Rust-only): `dotnet tool install -g dotnet-stryker`, `dotnet tool install -g SharpFuzz.CommandLine`, `dotnet tool install -g dotnet-reportgenerator-globaltool`.

## Architecture: where the work lives

Six layers, each with a different cardinal rule:

1. **`bin/straitjacket` (launcher → `bin/straitjacket-<triple>`)** — the Rust CLI. Pure-data helpers (parsers, walkers, hashers) live as `pub fn` in `src/commands/*.rs` alongside a `pub fn run(args: Args) -> anyhow::Result<()>` shell. Split is enforced by testability: pure helpers get unit tests; `run` is the thin glue that calls helpers + prints JSON + sets exit code. When porting/extending a subcommand, extract a pure-data helper first, then write the test, then the `run` glue.

2. **`src/common/`** — shared infrastructure used by multiple commands. **`walk.rs`** uses `WalkDir::filter_entry` for descent-time directory pruning (the load-bearing perf invariant: a post-walk `.filter()` still descends into `target/` and reads every file). **`subprocess.rs::run_with_timeout`** uses (a) `Command::env` for per-child env (`MSBUILDDISABLENODEREUSE=1` etc. — never `std::env::set_var`, which mutates the parent process) and (b) `taskkill /F /T /PID` on Windows for process-tree kill, because plain `child.kill()` orphans grandchildren that inherit the stdio pipes (this is exactly the same hazard the env vars work around for MSBuild). **`json_io.rs`** writes pretty-printed JSON with a trailing newline. **`cargo_target.rs`** holds `resolve_cargo_target` (pure manifest-paths → `CargoTarget`: a root-level `Cargo.toml` → run with `--workspace`; a single nested crate → run from that crate's dir *without* `--workspace`; multiple nested with no root → `Ambiguous`, never a silent pick) and `cargo_invocation` (maps a `CargoTarget` + base cargo args → cwd + argv, inserting `--workspace` only for a real workspace). `baseline_check`/`lint_check`/`run_new_tests` use them to run cargo from the correct directory on nested-crate layouts instead of a blind `--workspace` from `repo_root`.

3. **`agents/*.md`** — 11 specialist subagent definitions + `implementation-author` (tdd's green phase). Each has YAML frontmatter (`name`, `description`, `tools`, `model`, `effort`) and a body that's the agent's role + procedure + output-contract. Tool restrictions are **load-bearing isolation guarantees**, not advisory: `adversarial-*` agents have no `Bash`/`PowerShell` so they cannot `git diff` even if their prompt tries to make them — and spike `wf_060d27f3` confirmed this restriction also holds for **workflow-spawned** agents, so diff-isolation survives the workflow path. The `PreToolUse` hook scans adversarial-agent prompts as defense-in-depth (Agent path only). When editing an agent, preserve the tool list unless you intend to change its isolation contract.

4. **`skills/*/SKILL.md`** — the thin launcher skills. **`tdd`** drives test-first; the engine it composes (coverage planning, the author teams, the adversarial stages, the dispatch convention, the shared agent roster) is documented once in **`docs/STAGES.md`** — skills reference it instead of restating it. Each skill is a **workflow-first thin launcher**: the main session owns the checkpoints + the single-writer `work-units.json` merge, and delegates each deterministic fan-out phase to a **dynamic-Workflow stage** (`straitjacket workflow-script <fanout|adversarial|…>` → the `Workflow` tool) when available, else direct `Agent` dispatch. **Cardinal Rule 0: you never write test/impl code yourself** — that's the multi-agent collapse failure mode the skills exist to prevent; dispatch the `*-author` specialists. The only files the orchestrator writes are `work-units.json`, `tooling.json`, scaffolded test projects, and the final summary. (`audit`/`fuzz`/`mutation`/`debug`/`triage` are being decomposed out of the same engine; `regression` was retired — see the note in "What this repo is".)

   **`skills/report-bug/SKILL.md`** is the odd skill out — a *single-session, no-fan-out, no-agent* capture utility (Cardinal Rule 0 there is "capture fast, don't derail," not "don't write code"). It writes one tracked JSON ledger (`<repo>/.straitjacket/bugs.json`, schema `schemas/bug-record.schema.json`) in the consumer repo **before** any remote call, then opt-in-mirrors to a GitHub issue (`gh` CLI primary, github MCP fallback) and/or a Jira ticket (atlassian MCP, reusing `atlassian:triage-issue`'s `getAccessibleAtlassianResources → getVisibleJiraProjects → createJiraIssue` ordering). It's wired as a side-call from the `surfaced_bug` branch of the `tdd` skill (and, once it lands, `triage`), and its three bridge fields (`suspect_files`/`suspect_symbol`/`intended_behavior_seed`) map onto `coverage-reviewer`'s `target_file`/`target_symbol`/`intended_behavior` so a later run can lift a parked bug into a test work unit. **Repo-local ledger policy (diverges from the shipped contract):** in THIS repo `.straitjacket/bugs.json` is **gitignored / local-only** — the canonical copy lives in the **primary checkout's** working tree (the top-level clone's `.straitjacket/bugs.json`, *not* a `.claude/worktrees/*` copy — `repo_root` resolves to the worktree there, so a `report-bug` run from a worktree writes its own ignored ledger, not this one), not committed, to avoid per-branch ledger churn across the many dev worktrees. This is a deliberate divergence: the shipped `report-bug` contract (`skills/report-bug/SKILL.md`, `schemas/bug-record.schema.json`, `docs/STAGES.md`) still tells *consumers* to **commit** their ledger (it's their durable bug log + test-context source). When dogfooding here, write/append bug records to the primary-checkout ledger; don't re-track it.

5. **`hooks/hooks.json`** — three hook events: `UserPromptExpansion` matcher fires `straitjacket hook preflight` on the **green-baseline skills** (`tdd`, `mutation`, `fuzz`, `debug` — the ones that need a clean/buildable tree; `audit` is read-only, `triage` routes, `report-bug` captures, so they're excluded — the matcher and `hook.rs::is_plugin_skill_invocation` must stay in sync); `PreToolUse Agent` runs `straitjacket hook pre-adversarial` (scans `adversarial-*` prompts for `--- a/`, `+++ b/`, `git diff`); `PostToolUse Agent` runs `straitjacket hook post-agent` (dispatches `verify-new-tests-compile` after test authors). `verify-no-test-mutation` is deliberately NOT a per-author hook — it produced false positives on Rust source files with inline `#[cfg(test)] mod tests`; the orchestrator runs it once at end-of-phase as an audit and relies on `adversarial-vacuousness` / `adversarial-misalignment` specialists for primary cheat detection. Hook event types and JSON shapes live in `src/commands/hook.rs::HookEvent` / `HookDecision`. **The `Agent` hooks do NOT fire for workflow-spawned agents** — in the workflow path, diff-isolation rests on the agents' tool restrictions and the orchestrator runs the verify/audit steps itself; the `UserPromptExpansion` preflight is unaffected (it fires on skill invocation in the main session).

6. **`workflows/*.js`** — shipped dynamic-Workflow stage scripts (`adversarial`, `fanout`), `include_str!`'d into the binary and emitted by `straitjacket workflow-script <stage>`. A SKILL reads one (via the CLI), fills `args` bindings (work units, stack, mode — **never the diff**), and runs it through the `Workflow` tool; the script fans out our custom agents (`parallel()` + `agent({agentType})`) and returns a compact structured result the main session merges. The scripts hold the *deterministic choreography* (parallel / caps / synthesis / iterate); the SKILL holds the prompts + judgment + checkpoints. Workflows are NOT a plugin-discovered component, so the **binary-emit** path is how they reach an installed plugin (vs. `.claude/workflows/`, which wouldn't ship).

The Rust binary is also the *hook executor* — `straitjacket hook <event>` reads stdin JSON, decides via pure functions in `hook.rs`, and emits the Claude-expected response shape. Decision logic is unit-tested; the hook shell is thin.

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
- **Embed-freshness gate** `workflow_script::tests::committed_binary_embeds_current_workflow_scripts` (issue #49): execs the **committed** host-triple `bin/straitjacket-*` and asserts its `workflow-script <stage>` output equals `workflows/<stage>.js` (EOL-normalized) for every stage, so `cargo test --lib` goes RED when the committed binary is stale vs the source. It MUST exec the committed artifact — a constant-vs-on-disk-file comparison would be tautological (both come from the same build) and could never catch a stale *committed* binary. It skips gracefully when the host-triple binary isn't committed (only some triples ship). Don't "simplify" it into a same-build comparison.

## Fixing bugs in this repo (dogfood the loop)

This plugin's reason to exist is **"no fix without a failing test first."** Hold that discipline on the plugin's OWN code — consistently, even for one-liners and review-comment fixes:

- **A bug in testable code (the Rust crate) is fixed test-first through the loop, never hand-patched.** Reproduce → a `*-author` agent writes a RED test pinning the *correct* behavior (it must fail against the bug) → `implementation-author` makes it GREEN → commit. That is exactly `triage`'s fix-mode; run it via the author agents (Cardinal Rule 0: you don't write the test/impl yourself). A direct edit to source without a guarding test is the anti-pattern this plugin exists to prevent. If you've already hand-patched, revert to the bug and redo it test-first.
- **Orchestration (`agents/*.md`, `skills/*/SKILL.md`, `workflows/*.js`, `hooks.json`) has no unit-test harness** — it is hand-authored and guarded by the end-to-end / live-workflow run, not a unit test. State explicitly (in the PR/commit) which fixes are test-backed vs. live-run-guarded, so the verified/unverified line is never blurred. **And don't stop at "live-run-guarded": verify an orchestration fix via `straitjacket:audit` scoped to the changed file(s)** — the LLM lenses read what no test can cover. This is codified as [`docs/STAGES.md`](docs/STAGES.md) Cardinal Rule 8 (TDD-unverifiable → audit) and wired into the `tdd` / `triage` "Handle the result" sections.

## Git workflow notes

- The per-platform `bin/straitjacket-<triple>[.exe]` binaries (~3MB each) plus the `bin/straitjacket` / `bin/straitjacket.cmd` launchers are committed. Don't gitignore them. `.gitattributes` keeps the sh launcher LF-only (a CRLF shebang breaks `/bin/sh`) and marks the binaries `binary`.
- `target/`, `.straitjacket/*/` (per-run state from the skills themselves), and `2026-*-*.txt` (session transcript files) are gitignored. `.straitjacket/bugs.json` is also gitignored in this repo (local-only policy).
- The repo uses `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` in commits where Claude Code contributed.

## Available memory

`~/.claude/projects/C--Users-KemonoNeco-Code-regression-tests-plugin/memory/` contains several memory files relevant to this repo (project scope, cargo build env, several feedback notes). The index at `MEMORY.md` pre-loads into the session context automatically.
