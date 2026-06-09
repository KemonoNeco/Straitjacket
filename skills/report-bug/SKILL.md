---
name: report-bug
description: "Capture bugs to a tracked local ledger AND bulk-publish them to a team tracker — one skill, two modes over the same .straitjacket/bugs.json ledger and gh/Jira mirror calls. CAPTURE MODE (default): catch a single bug the moment it's found — write the local record first, then optionally mirror one GitHub issue and/or Jira ticket — so you keep working and fix it later; use when a bug/defect/crash/failing assertion/wrong output/regression is found and should be filed without derailing the work, when the user says 'file a bug', 'report this', 'log this defect', 'track this for later', and as a side-call from the tdd/triage workflows. PUBLISH MODE (bulk): publish many ledger (or inline) findings to a tracker with consistent formatting, parent/child grouping for recurring defect classes, and a confirm-before-bulk gate; use on 'file these as issues', 'mirror the findings', 'create Jira tickets', or after a straitjacket audit moves bugs from the ledger to a tracker. Publish uses TWO templates by destination: an ENGINEERING bug template for GitHub (clear mechanical fix) and a TRIAGE/DECISION template for Jira (design / no-clear-fix findings needing human judgment). Supports GitHub (gh CLI, github MCP fallback) and Jira (atlassian MCP). Use atlassian:triage-issue instead when the goal is interactive Jira duplicate-hunting rather than fast capture or formatted bulk publishing."
---

# report-bug

One skill, **two modes** over one substrate — the tracked ledger at `<repo_root>/.straitjacket/bugs.json`
and the same `gh` / atlassian-MCP mirror primitives + severity→priority map.

- **Capture mode** (default) — catch **one** bug fast, local-first, return control. This is an interrupt
  that must not derail the task you were on.
- **Publish mode** — take **many** findings (the ledger, or an inline list) and publish them to a team
  tracker with consistent formatting, parent/child grouping, and a confirmation gate before bulk-create.

The two modes have **deliberately opposite cardinal rules** (fast-and-silent vs deliberate-and-confirmed).
Do **not** blend them — pick the mode first, then follow only that mode's rules.

## Choosing a mode

| Signal | → Mode |
|---|---|
| A bug was just found; "file a bug", "log this", "track this for later"; a `tdd`/`triage` surfaced-bug side-call | **Capture** |
| One bug at a time; you were mid-task and want to keep going | **Capture** |
| "file these as issues", "mirror the findings", "create Jira tickets", "open issues for the audit findings" | **Publish** |
| A batch of findings already in the ledger (e.g. after a `straitjacket:audit` run) → a tracker | **Publish** |

When genuinely ambiguous (e.g. a bare "open an issue" with a single fresh bug in hand), default to
**Capture** — it is local-first and reversible. Escalate to Publish only when the ask is plainly about
publishing a *batch* to a tracker.

---

# Capture mode (default) — one bug, fast, local-first

## Cardinal Rule 0 — CAPTURE FAST, DON'T DERAIL

This mode exists to **catch a bug and let you keep going**. It is often invoked in the middle of another task (a feature, a refactor, a `straitjacket:tdd` run). Treat it as an interrupt that must return control quickly:

- **Write the local record FIRST**, before any network call. A bug must never be lost because a backend is unauthenticated, a repo is unconfigured, or the host workflow is interrupted.
- **Remote mirroring is opt-in.** Default behavior is local-only. Only create a GitHub issue / Jira ticket when the user asked for it, a config enables it, or the invocation requested it.
- **Never fix the bug here, and never start investigating beyond what's needed to fill the record.** Capturing ≠ debugging. If you were mid-task, capture and hand control straight back to that task.
- **Degrade silently.** An absent or unauthenticated backend → skip that destination, note it in your report, keep the local record. Never fail the capture because a remote is unavailable.

## What gets written

A single tracked ledger in the **consumer repo** (the project being worked on, not this plugin):

```
<repo_root>/.straitjacket/bugs.json
```

Shape: `{ "bugs": [ BugRecord, ... ] }`. The record schema is `schemas/bug-record.schema.json` (this plugin). **This file is meant to be committed** — it is the durable bug log and the test-context source. Do NOT add it to `.gitignore` (note: per-run state under `.straitjacket/<run_id>/` is gitignored via the scoped `.straitjacket/*/` pattern; `.straitjacket/bugs.json` is a top-level file in the same dir that the scoped pattern deliberately does NOT match — don't widen the ignore to a bare `.straitjacket/`, which would sweep it up).

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

## Remote body template (capture mode)

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

> Capture mode's single-mirror body stays **deliberately simple** — one flat template. The richer
> engineering / triage templates and parent/child grouping live in **Publish mode** below, for
> deliberate bulk publishing.

## Configuration (optional)

`<repo_root>/.straitjacket/report-bug.config.json` — all keys optional; shared by both modes:

```json
{
  "default_destinations": ["local"],          // subset of ["local","github","jira"]; "local" is implicit (capture mode)
  "github_repo": "owner/name",                // pin instead of inferring from cwd (both modes)
  "jira_project_key": "PROJ",                 // skip the project prompt (both modes)
  "severity_priority_map": { "critical": "Highest", "high": "High", "medium": "Medium", "low": "Low" }
}
```

Absent config → local-only (capture), infer GitHub repo from cwd, ask for the Jira project when Jira is requested.

## Relationship to other skills

- **`atlassian:triage-issue`** does heavyweight Jira *duplicate hunting* and interactive triage. `report-bug` capture mode is the lightweight fast-capture path — local-first, dedupe is local-only, and it doesn't interrupt a host workflow. Use `triage-issue` when the goal *is* duplicate-hunting triage; use `report-bug` when the goal is "log it and move on" (capture) or "publish these findings consistently" (publish).
- **`straitjacket:tdd`** and **`triage`** invoke capture mode from their `surfaced_bug` branch when a run finds a real bug outside the current scope, then keep running (the write side). The records are *structured so* a later run can lift an `open` record into a test work unit via the three bridge fields above: in **fix mode**, the `coverage-reviewer` is seeded from the ledger and treats `intended_behavior_seed` as the authoritative contract (see [`docs/STAGES.md`](../../docs/STAGES.md)).
- **`straitjacket:audit`** files confirmed defects into the ledger (capture mode, per-finding, with its capture gate). Once they are in the ledger, **publish mode** (below) is how you bulk-publish them to a tracker.

---

# Publish mode — ledger → tracker, in bulk

Publish findings as **consistently-formatted** issues. The destination decides the template:
**GitHub gets the engineering bug template** (the finding has a clear mechanical fix); **Jira gets
the triage/decision template** (the finding needs human judgment — there is no single correct fix
yet). Publish mode owns the *format*, the *structure* (parent/child grouping), routing, and the ledger
mirror. It does not fix bugs and does not diagnose — it reports. Source of findings is usually the
straitjacket bug ledger (`<repo>/.straitjacket/bugs.json`) but can be an inline list.

## Cardinal rules (publish mode)

1. **Template by destination — never mix.** GitHub → engineering bug template. Jira → triage
   template. Within a destination the template is used consistently for every issue. Ad-hoc
   formatting is the thing this mode exists to prevent.
2. **Route by fixability.** A finding with a clear, mechanical fix → **GitHub**. A finding that is
   design-based / has no clear logical fix / needs product-security-QA judgment (secrets storage,
   intended RBAC model, password-crypto migration, missing algorithm spec) → **Jira** (it needs
   human triage, so it gets the decision template, not a "Suggested fix").
3. **Recurring → parent + children. One-off → standalone.** A defect class recurring in ≥2
   files/scopes becomes a PARENT (GitHub parent issue / Jira Epic); each instance is a CHILD
   (GitHub sub-issue / Jira story). A distinct one-off is a single standalone issue.
4. **Mirror back.** Write each finding's tracker id into its ledger record — GitHub:
   `remote.github` (`repo`/`number`/`url`); Jira: `remote.jira` (`key`/`url`); set `status: "mirrored"`.
   Mark anything intentionally withheld with a `routed: <reason>` note.
5. **Reconcile.** Σ(GitHub-filed) + Σ(Jira-filed) + Σ(withheld) == ledger count, no overlap. Prove it.
6. **Confirm before bulk.** Creating dozens of issues in a team tracker is outward-facing and hard
   to undo. Present the plan + counts **via plan mode (EnterPlanMode → ExitPlanMode), not a local
   plan file**, and for large batches pilot ONE group first. Honor any standing hold until the user
   explicitly approves.

## Title conventions

- GitHub child / standalone: `[<Area or Symbol>] <concise what-failed + where>` — no trailing period.
- GitHub parent: `[P<n>] <pattern name> — recurring (<N> instances)`.
- Jira triage item: `[Decision] <the choice to be made + area>` (e.g. `[Decision] Authorization model — re-enable role checks`).
- Jira Epic: `<decision theme>` (e.g. `Authentication & authorization model`).

## GitHub — engineering bug templates (clear fix)

### Child / standalone

```markdown
## Summary
<one or two sentences: what is wrong>

| | |
|---|---|
| **Severity** | <emoji> <Critical/High/Medium/Low> |
| **Type** | <defect class> · lens: <lens> |
| **Component** | <project/area> |
| **Location** | `<path/File.cs>` → `<Namespace.Symbol>` |
| **Pattern** | Part of #<parent> · `[P<n>] <slug>`   ← OMIT this row for a one-off/singleton |
| **Ledger** | `<bug-id>` · <audit-run-id> |

## Expected behavior
<the intended behavior, stated as the correct behavior>

## Actual behavior
<what it actually does + the consequence>

## Evidence
<file/line + the defective construct; how/when it triggers>

## Suggested fix
<the mechanical fix direction — a seed, not a mandate>

---
<sub>Surfaced by Claude Code with KemonoNeco/Straitjacket <code>straitjacket:audit</code></sub>
```

### Parent (pattern)

```markdown
## Pattern summary
<the recurring mechanism, in 1–2 sentences; how many sites>

| | |
|---|---|
| **Pattern** | P<n> — <slug> |
| **Severity (max)** | <emoji> <level> |
| **Lens** | <lenses present> |
| **Instances** | <N> (tracked as sub-issues ↓) |
| **Audit** | <audit-run-id> |

## Shared remediation
<the common fix approach for this whole class>

## ⚠️ Duplicated forks
<`#A` ≡ `#B` — identical code in two files; fix both or de-duplicate. Omit if none.>

## Affected components
<subsystems / files touched>

---
<sub>Surfaced by Claude Code with KemonoNeco/Straitjacket <code>straitjacket:audit</code>. Sub-issue list & rollup progress render natively below.</sub>
```

The parent's instance list/progress renders **natively** from sub-issue links — never hand-maintain
a checklist in the body (it drifts).

## Jira — triage / decision templates (needs human judgment)

These findings have **no single correct fix** — the right action depends on a product/security/policy
decision. The template is decision-oriented: it replaces "Suggested fix" with **Decision required**
and **Options & trade-offs**, giving the human triager something concrete to choose between.

### Triage item (story)

```markdown
## What we found
<the risky behavior / gap, in plain terms>

| | |
|---|---|
| **Priority** | <Highest/High/Medium/Low> |
| **Type** | <security policy / auth model / data migration / missing spec / …> |
| **Component** | <area> |
| **Location** | `<path/File.cs>` → `<Symbol>` |
| **Epic** | <Epic key — if part of a grouped decision theme> |
| **Ledger** | `<bug-id>` · <audit-run-id> |

## Why this needs human triage
<the judgment required; *why there is no mechanical fix* — depends on intended RBAC / secret-storage
strategy / migration choice / a product call>

## Current behavior & risk
<what the code does today + the exposure/impact if left as-is>

## Decision(s) required
- [ ] <the specific question a human must answer before any fix is valid>
- [ ] <second question, if any>

## Options & trade-offs
1. <option A> — <pro / con>
2. <option B> — <pro / con>

## Impact if unaddressed
<who/what is affected, urgency>

---
Surfaced by Claude Code with KemonoNeco/Straitjacket `straitjacket:audit`
```

### Epic (decision theme)

Group related decisions (not code mechanisms) — e.g. all auth findings under one "authorization
model" Epic, all secrets under "secrets management".

```markdown
## Decision theme
<the cross-cutting policy/design area to resolve>

| | |
|---|---|
| **Type** | <policy/design area> |
| **Priority (max)** | <level> |
| **Findings** | <N> (stories ↓) |
| **Audit** | <audit-run-id> |

## Why grouped
<these findings all hinge on the same decision>

## Key questions for the team
- <overarching decision 1>
- <overarching decision 2>

---
Surfaced by Claude Code with KemonoNeco/Straitjacket `straitjacket:audit`
```

## Labels & fields

**GitHub** — set the native **issue type** (NOT a `bug` label): `Bug` on findings (children +
singletons), `Task` on pattern parents. `gh` (≤2.89) has no flag for it — use REST:
`gh api --method PATCH repos/<o>/<r>/issues/<n> -f type=Bug` (or `type=Task`). The org must have the
types defined (`gh api orgs/<org>/issue-types`; defaults: Bug/Task/Feature). Then create/apply labels
(idempotent `gh label create … --force`):
- `audit` (if from an audit run)
- `severity:critical|high|medium|low` — child = its severity; parent = max of its children
- `lens:<lens>` — one per distinct audit lens present
- **No `bug` label** — the native issue **type** conveys it. **No `pattern:*` label** — pattern
  membership is tracked structurally via the parent/sub-issue link. (The child's metadata "Pattern"
  row still names + links its parent.)
- Colors: severity critical `B60205` / high `D93F0B` / medium `FBCA04` / low `0E8A16`;
  `audit` `5319E7`; `lens:*` `1D76DB`.

**Jira** — issue **type** = Epic (parent) / Task (finding); Severity → **Priority** (reuse the
`severity_priority_map` from config); type/lens → **labels/components**; pattern/theme → **Epic link**
(`customfield_10014` on classic projects).

## Structure maps across trackers (templates do not)

| Concept | GitHub | Jira |
|---|---|---|
| Recurring group (parent) | parent issue, type **Task** | **Epic** |
| Member (child) | sub-issue, type **Bug** | **Task** under the Epic |
| One-off | standalone issue, type **Bug** | standalone Task |
| Issue type | native type (Bug / Task) | issuetype (Bug / Task / Epic) |
| Severity | `severity:*` label | **Priority** field |
| Membership | parent/sub-issue link | Epic link |
| Template | **engineering bug** | **triage / decision** |
| Mirror id | `remote.github` | `remote.jira` |

## Parent/child mechanics — GitHub (gh CLI has no native sub-issue command — use the REST API)

1. Create the child issue → capture its **number** from the returned URL.
2. Resolve the child's **internal database id** (NOT the number, NOT node_id):
   `gh api repos/<owner>/<repo>/issues/<child_number> --jq .id`
3. Link it under the parent:
   `gh api --method POST repos/<owner>/<repo>/issues/<parent_number>/sub_issues -F sub_issue_id=<internal_id>`
   (use `-F` for the integer; `-f` sends a string and fails). Expect HTTP 201.
- Limits: ≤100 sub-issues per parent, ≤8 nesting levels.
- Throttle creation (~700 ms between `gh issue create` calls) to dodge GitHub's secondary
  content-creation rate limit; in Node use `Atomics.wait` (the Bash tool blocks foreground sleep).

## Epic/story mechanics — Jira (atlassian MCP)

1. Confirm the **project key** + issuetype scheme first (`getJiraProjectIssueTypesMetadata` /
   `getVisibleJiraProjects`) — Epic/Story availability varies per project.
2. Create the **Epic** with `createJiraIssue` (issuetype Epic), description = Epic template.
3. Create each finding as a **Story** with `createJiraIssue`, Epic-link set, description = triage
   template; set **Priority** from severity, add type/lens labels.
4. Relate duplicate-fork "≡" findings with `createIssueLink`.
5. Mirror the returned **key** back into the ledger (`remote.jira`).

## Procedure (publish mode)

1. **Load findings** + the routing call (which → GitHub, which → Jira triage). Source is the ledger
   (`open`/`mirrored` records) or an inline list.
2. **Reconcile** the split to the ledger total before creating anything.
3. **Confirm** the plan + counts via plan mode (Cardinal rule 6) — pilot ONE group for large batches.
4. **GitHub:** create labels; create children (engineering template) + parents, link sub-issues.
5. **Jira:** create Epics (decision themes) + stories (triage template) via the atlassian MCP.
6. **Mirror** ids back into each ledger record (`remote.*`, `status: "mirrored"`); reconcile again.
7. **Verify** a sample of each (labels/fields + body + parent link).

## Notes (publish mode)

- Static audit findings have no runtime "steps to reproduce" — GitHub's **Evidence** field replaces
  it (code location + defective construct).
- Don't put "Suggested fix" on a Jira triage item — the absence of a clear fix is *why* it's on
  Jira. Give **Decision required** + **Options** instead.
- A `.github/ISSUE_TEMPLATE/bug_report.yml` (GitHub) and/or a Jira request-type matching these
  templates keep future hand-filed issues consistent; offer, but only create on request.
