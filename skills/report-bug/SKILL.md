---
name: report-bug
description: "Capture a bug the moment it is found — write a tracked local record first, then optionally mirror it to a GitHub issue and/or a Jira ticket — so you can keep working on the current task and fix it later. Use when a bug, defect, crash, failing assertion, wrong output, or regression is discovered and should be filed without derailing the work in progress; when the user says 'file a bug', 'open an issue', 'report this', 'log this defect', 'track this for later', or 'create a ticket'; and as a side-call from the straitjacket tdd workflow (and triage) when a run surfaces a real bug it will not fix in that run. Writes a JSON ledger at .straitjacket/bugs.json structured so a later tdd (target/fix-mode) run can lift it into test work units. Supports GitHub (gh CLI, github MCP fallback) and Jira (atlassian MCP). This is the fast local-first capture path with a local-only dedupe guard; use atlassian:triage-issue instead when the goal is interactive Jira duplicate-hunting and triage rather than capture-and-continue."
---

# report-bug

## Cardinal Rule 0 — CAPTURE FAST, DON'T DERAIL

This skill exists to **catch a bug and let you keep going**. It is often invoked in the middle of another task (a feature, a refactor, a `straitjacket:tdd` run). Treat it as an interrupt that must return control quickly:

- **Write the local record FIRST**, before any network call. A bug must never be lost because a backend is unauthenticated, a repo is unconfigured, or the host workflow is interrupted.
- **Remote mirroring is opt-in.** Default behavior is local-only. Only create a GitHub issue / Jira ticket when the user asked for it, a config enables it, or the invocation requested it.
- **Never fix the bug here, and never start investigating beyond what's needed to fill the record.** Capturing ≠ debugging. If you were mid-task, capture and hand control straight back to that task.
- **Degrade silently.** An absent or unauthenticated backend → skip that destination, note it in your report, keep the local record. Never fail the capture because a remote is unavailable.

## What gets written

A single tracked ledger in the **consumer repo** (the project being worked on, not this plugin):

```
<repo_root>/.straitjacket/bugs.json
```

Shape: `{ "bugs": [ BugRecord, ... ] }`. The record schema is `schemas/bug-record.schema.json` (this plugin). **This file is meant to be committed** — it is the durable bug log and the test-context source. Do NOT add it to `.gitignore` (note: `.claude-regression/` is gitignored per-run state; `.straitjacket/bugs.json` is a deliberately different, tracked location — don't let a gitignore edit sweep it up).

### The three consumption-bridge fields

The user keeps this ledger so a **later `straitjacket:tdd` (target/fix-mode) run can turn a bug into a test without re-deriving it from prose**. Fill these so the bridge is real, not nominal — they map directly onto what `coverage-reviewer` ingests:

| BugRecord field | → maps to `WorkUnit` field | what to write |
|---|---|---|
| `suspect_files[]` | `target_file` | repo-relative path(s) you believe contain the defect |
| `suspect_symbol` | `target_symbol` | language-qualified fn/method/type (`Parser::parse_header` / `Parser.ParseHeader`) |
| `intended_behavior_seed` | `intended_behavior` | a **contract sentence** for the *correct* behavior, derived from expected-vs-actual (e.g. "parse_header rejects a header > 4 KiB with `HeaderTooLong` instead of truncating") |

If you can only guess, guess and say so in `notes` — a weak seed still beats prose-parsing later.

## Procedure

### Step 1 — Gather the record (fast)

Assemble a `BugRecord` from what you already know. If invoked mid-workflow, you usually have most of it from context (the failing test, the stack trace, the file you were in) — **use that; don't re-investigate**. Set `discovered_during` to the provenance (e.g. `"straitjacket:tdd run <run_id>"`, `"manual"`, `"code review"`).

Minimum to proceed: `title`, `severity`, `summary`, `expected`, `actual`, plus a best-effort `suspect_files` + `intended_behavior_seed`. If `expected`/`actual` are genuinely unknown, ask **one** tight question rather than guessing the oracle — everything else can be filled best-effort or left empty.

Generate `id` as `bug-<created>-NN`: take the current date (`created`, ISO `YYYY-MM-DD` from the session environment), find the highest `NN` already used for that date in the ledger, and use the next integer, zero-padded to 2 digits (`bug-2026-05-28-01`, `-02`, …).

### Step 2 — Cheap local dedupe guard

Read `.straitjacket/bugs.json` if it exists (create the dir + an empty `{ "bugs": [] }` ledger if not). Before writing, scan **open** records (`status` in `open`/`mirrored`) for a likely match against the new bug — compare on `title` similarity, overlapping `suspect_files`, and `error_signature`. This is a **local-only** match — do NOT run a remote duplicate search (that's `atlassian:triage-issue`'s job and would derail).

- **Clear match** → don't create a second record. Append the new context to the existing record's `notes` (and add a remote comment in Step 4 if it's already mirrored). Report which record you updated.
- **No / weak match** → proceed to Step 3.

### Step 3 — Write the local record (ALWAYS, before any network)

Append the `BugRecord` to `bugs.json` with `status: "open"` and no `remote` block yet. Validate against `schemas/bug-record.schema.json`. This write is the point of the skill — once it lands, the bug is safe.

### Step 4 — Mirror to remotes (OPT-IN only)

Determine the destination set from, in order: the explicit invocation request → `.straitjacket/report-bug.config.json` (`default_destinations`, `github_repo`, `jira_project_key`) if present → **default: none** (local-only). Mirror only to the resolved destinations.

#### GitHub — `gh` CLI primary, github MCP fallback

1. Probe: `gh auth status` (and `gh repo view --json nameWithOwner` to resolve the repo, unless config pins `github_repo`). Not authenticated / `gh` absent → fall back to a github MCP server if one is connected (e.g. `mcp__plugin_github_github__issue_write` with `method: "create"`, owner/repo/title/body); MCP also unavailable → skip GitHub, note it, keep going.
2. Create the issue. Title = record `title`. Body = the rendered template (below — the body carries severity, so the issue is fully self-describing even with no labels).
   ```bash
   gh issue create --title "<title>" --body-file <tmp> [--label <existing-label> ...]
   ```
   Write the body to a temp file under `$CLAUDE_JOB_DIR/tmp` (or an OS temp path) — never inline a multi-line body on the command line.

   **Labels are best-effort and must NEVER fail the mirror.** `gh issue create` errors out if a passed label does not already exist in the repo (it does not auto-create labels) — a fresh repo ships `bug` but almost never `severity:high`. So: list existing labels first (`gh label list --json name -q '.[].name'`), pass `--label` only for names that already exist (plus any record `labels` that exist), and **omit `--label` entirely if unsure**. Optionally create missing ones first (`gh label create "severity:<sev>" 2>/dev/null` — ignore failure). If a create call still fails citing a label, retry once with no `--label` flags. Never drop the whole GitHub mirror over a label.
3. Capture the returned issue URL + number into `remote.github`.

#### Jira — atlassian MCP

Reuse the proven call ordering (same as `atlassian:triage-issue`):
1. `getAccessibleAtlassianResources` → take `cloudId` (skip Jira with a note if this returns nothing — unauthenticated).
2. Resolve the project: config `jira_project_key`, else `getVisibleJiraProjects` and ask the user which project (one question) if ambiguous.
3. `getJiraProjectIssueTypesMetadata(cloudId, projectIdOrKey)` → pick `Bug` if available, else first non-Epic/non-Subtask type.
4. `createJiraIssue(cloudId, projectKey, issueTypeName, summary=<title>, description=<rendered body>, additional_fields={ "priority": { "name": <map severity→Highest/High/Medium/Low> } })`. If it fails on a required field, `getJiraIssueTypeMetaWithFields`, ask for the value, retry.
5. Capture the returned key + browse URL into `remote.jira`.

### Step 5 — Link back & report

If any remote was created, update the local record: set `status: "mirrored"`, fill `remote.github` / `remote.jira`. Re-write `bugs.json`.

Then give a **one-screen** report and return control to whatever you were doing:

```
🐞 Captured bug-2026-05-28-01 — [Parser] header > 4 KiB truncates instead of erroring  (severity: high)
  local:  .straitjacket/bugs.json
  github: https://github.com/owner/repo/issues/42
  jira:   PROJ-123  (https://…/browse/PROJ-123)
  seed:   parse_header rejects a header > 4 KiB with HeaderTooLong   ← test-context for a future tdd fix-mode run
```

Omit any destination that was skipped, and say *why* (e.g. "jira: skipped — not authenticated"). If you were invoked mid-task, end by resuming that task.

## Remote body template

Render this for both the GitHub issue body and the Jira description:

```markdown
**Severity:** <severity>

## Summary
<summary>

## Expected
<expected>

## Actual
<actual>

## Steps to Reproduce
1. <step>
2. <step>

## Suspect location
- `<suspect_file>` — `<suspect_symbol>`

## Error signature
```
<error_signature>
```

## Environment
<environment>

---
Local id: `<id>` · discovered during: <discovered_during> · captured via straitjacket:report-bug
```

Drop any section whose field is empty.

## Configuration (optional)

`<repo_root>/.straitjacket/report-bug.config.json` — all keys optional:

```json
{
  "default_destinations": ["local"],          // subset of ["local","github","jira"]; "local" is implicit
  "github_repo": "owner/name",                // pin instead of inferring from cwd
  "jira_project_key": "PROJ",                 // skip the project prompt
  "severity_priority_map": { "critical": "Highest", "high": "High", "medium": "Medium", "low": "Low" }
}
```

Absent config → local-only, infer GitHub repo from cwd, ask for the Jira project when Jira is requested.

## Relationship to other skills

- **`atlassian:triage-issue`** does heavyweight Jira *duplicate hunting* and interactive triage. `report-bug` is the lightweight fast-capture path — local-first, dedupe is local-only, and it doesn't interrupt a host workflow. Use `triage-issue` when the goal *is* triage; use `report-bug` when the goal is "log it and move on."
- **`straitjacket:tdd`** (and, once it lands, **`triage`**) invoke this skill from their `surfaced_bug` branch when a run finds a real bug outside the current scope, then keep running (the write side). The records are *structured so* a later run can lift an `open` record into a test work unit via the three bridge fields above: in **fix mode**, the `coverage-reviewer` is seeded from the ledger and treats `intended_behavior_seed` as the authoritative contract (see [`docs/STAGES.md`](../../docs/STAGES.md)). The retired `regression` skill's diff mode never auto-read the ledger — that judgment call (how a non-diff entry enters a diff-oriented coverage phase) is left to the user.
