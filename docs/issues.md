# Project issues

`hey-boss issue` is a SQLite-backed issue tracker for agents and people. It runs
on macOS and Linux without the notification daemon. `issues` is an alias for
`issue`. Issue commands do not post notifications or resume agent sessions.

## Everyday commands

```sh
hey-boss issue whoami
hey-boss issue list
hey-boss issue list --mine
hey-boss issue list --unassigned --label bug
hey-boss issue create --title 'Fix reconnect' --body 'Reconnect after waking from sleep.'
hey-boss issue view 1
hey-boss issue claim 1
hey-boss issue comment 1 --body 'Reproduced on the latest build.'
hey-boss issue resolve-comment 1 42
hey-boss issue unresolve-comment 1 42
hey-boss issue edit 1 --body '## Expected behavior
Reconnect after waking and preserve pending notifications.'
hey-boss issue close 1 --comment 'Fixed in commit abc123.'
```

Bodies and comments are Markdown text stored directly in SQLite. No Markdown
file is required. `create` allows an empty body; comments must contain text.

Issues have **Open**, **Blocked**, **Closed**, and **Deleted** views. Blocked
issues retain their content and history, release their claim, and pause worker
pickup. Blocking should be rare: make every effort to resolve the problem first,
raise questions and ask the user for help with `hey-boss ask`. Explain what needs
to change in a comment, then reopen when work can proceed:

```sh
hey-boss issue block 1 --comment 'Waiting for the user to restore access.'
hey-boss issue list --state blocked
hey-boss issue reopen 1
```

The CLI prints this guidance when blocking; the web **Block issue** action shows
it before applying the change and saves any entered comment with the transition.
Workers automatically block an issue after five unsuccessful launched attempts
in its current open cycle. Cancellations, interruptions and unlaunched
reservations do not consume that budget. Reopening starts a fresh budget, retaining
all prior agent runs. A blocked subtask still prevents its parent from pickup.
Blocking a closed issue requires reopening it first. Deleting and restoring a
blocked issue preserves its blocked state. Worker Retry requires an open issue.

Each comment has a subtle **Resolve** action. Resolved comments collapse into a
compact row; **Show comment** expands their original content, and **Unresolve**
restores the full card. Resolution persists across reloads and fleet sync without
changing the text, author, or comment count. CLI commands take the issue number
and comment ID. Resolution and unresolution appear in the activity history.

`edit` replaces only supplied fields; `--body ''` deliberately clears the body.
Titles are required on creation, nonblank, and limited to 512 UTF-8 bytes.

Use `--body -` to read up to 1 MiB of UTF-8 from stdin:

```sh
hey-boss issue create --title 'Fix reconnect' --body - <<'MD'
## Problem
Connection drops after sleep.

## Acceptance criteria
- [ ] Reconnect after waking
- [ ] Preserve pending notifications
MD
```

`--file issue.md` (alias `--markdown-file`) is an optional alternative to `--body`.
It snapshots the file's text into the database immediately. Editing, moving, or
deleting the file afterward does not change the issue. `--file -` reads stdin.
Bodies, file contents, and comments have the same 1 MiB bound. Relative assets
referenced inside Markdown are not imported.

## Web interface

```sh
hey-boss issue web
# Open http://127.0.0.1:4781/; Ctrl+C stops the server.
hey-boss issue web --port 4782 --project github.com/kamilio/hey-boss
hey-boss issue web --host devbox
hey-boss issue projects --json
```

The interface shares the CLI's database and operations. Switch projects from the
header, or create a custom named project. Search matches titles and
Markdown bodies; labels, assignee, and open/closed/deleted filters can be combined.
Lists show all matching issues on one page. Issue pages support Markdown previews, comments,
editing, claims, unassignment, close/reopen, delete/restore, and complete activity
history. Taking over another session's claim requires an explicit confirmation.

Closed list rows show when the issue was closed and who closed it; hover over
the time for the exact date and time. The assignee section offers **Unassign**
for any claimed open issue, with confirmation when releasing another session’s
claim. Unassign keeps the issue open.

Descriptions, comments, and previews share the CommonMark renderer with tables,
task lists, strikethrough, nested lists, reference links, footnotes, GitHub
callouts, and highlighted fenced code. Bare HTTP/HTTPS URLs, `www.` addresses,
and email addresses become clickable links; URLs inside code remain literal.
Table alignment and callout/code colors work in both themes. Each comment's
footnotes navigate within that comment without changing the current issue.

The web app always acts as **Boss** (`human:boss`), regardless of the session or
`--agent` that launches it. Rename Boss globally in **Settings** from the profile
badge dropdown, or with `hey-boss settings set --boss-name 'Alex'`.
`hey-boss settings show` reads the profile. The name applies to every project
in the authoritative SQLite store; use `--host HOST` for a remote store.
Existing assignments retain the same ID and issue revision. CLI session
detection remains independent. Assign with
`hey-boss issue assign-to-boss 1`; another owner's assignment or worker reservation
requires `--force`. Boss assignments are excluded from worker pickup until
unassigned. To act as Boss from the terminal, use `--agent human:boss`.

Click a label or assignee in a list row to filter the list, keeping the other
active filters. Filter selections are reflected in the URL and dropdowns;
choose Assignee or Labels to clear them. CLI supports
`hey-boss issue list --assignee boss` or an exact session ID.

Press **N** to create an issue, **/** to search, **Escape** to close an editor or
project picker, and **Command/Ctrl+Enter** to submit an editor or comment. The
interface supports keyboard navigation and narrow screens. Appearance follows
the system’s light/dark setting automatically, including live changes, with no
theme switcher or saved override.

On the local HTTP interface, unsaved issue and comment drafts are saved in the browser's local storage, scoped
to project and issue. They survive navigation and reloads. Retry IDs also survive
reloads, preventing duplicate writes after a lost response. Saved content lives
in SQLite; browser drafts remain on that browser profile and origin until saved
or browser storage is cleared. Changing ports changes the browser storage origin.
Do not use a shared browser profile for private drafts.

Updates are checked every five seconds while the page is visible. A changed issue
shows an update banner, preserving any comment draft. Editing uses optimistic
versions: conflicts display the latest saved text alongside the draft and require
an explicit choice to replace that version. Connection failures preserve drafts;
restarting the server under the same identity reconnects automatically.

The server binds only to `127.0.0.1`. Assets are embedded, with no external fonts,
scripts, or analytics. Browser requests use same-origin and CSRF checks. Markdown
HTML is escaped, unsafe links are blocked, and external images are not loaded.
This is a local interface, not a public multi-user hosting service. For a shared
SSH authority, the web server stays local and forwards operations through the
same SSH transport as the CLI; install matching CLI versions on both hosts.
`--port 0 --json` reports an automatically selected port for tooling.

See [web verification](issues-web-verification.md) for reproducible functional,
accessibility, browser, and performance checks.

### Mobile access without a database copy

Use the existing responsive web interface through **Tailscale Serve**. It
terminates HTTPS on the machine running the interface and proxies to its
loopback listener. The phone sends issue operations to the original SQLite
store; there is no replicated database, exported snapshot, cloud issue hub,
offline issue cache, or service worker. Reads and writes use the same ownership,
revision, and retry rules as the desktop web interface.

Install and sign in to Tailscale on that machine and your phone. Enable HTTPS
for the tailnet, and find the machine's full DNS name with `tailscale status`.
Restrict access to the serving machine's TCP port 8443 to your own devices using
your tailnet grants/ACLs. Then, in one terminal (replace the example hostname):

```sh
hey-boss issue web --port 4782 --mobile-origin https://mac.tailnet.ts.net:8443
```

In a second terminal:

```sh
tailscale serve --https=8443 http://127.0.0.1:4782
```

Use foreground Serve so Ctrl+C removes this temporary endpoint. Check
`tailscale serve status` to confirm it is **tailnet only**. Open
`https://mac.tailnet.ts.net:8443/` in Safari or Chrome on your phone while
Tailscale is connected. You can add a browser shortcut to the home screen.
Both the issue web process and Serve must remain running; disconnecting or
sleeping the serving machine makes the page unavailable. Do not enable
**Tailscale Funnel**, public port forwarding, or a public reverse proxy.

The `--mobile-origin` flag accepts one exact lowercase HTTPS origin ending in
`.ts.net`, with no path, query, credentials, or fragment. Omit `:443` for the
default HTTPS port; nondefault ports such as 8443 must match Serve. The flag
does not install/configure Tailscale or open a network listener. The HTTP server
still binds only to `127.0.0.1`; forwarded host/protocol headers are never trusted.
Serve must preserve the HTTPS origin's Host header. Local desktop access still
works. `--json` includes `mobile_url` alongside the local `url`.

For an issue store on an SSH host, add `--host devbox` to the web command. The
web process forwards operations over SSH without copying that store to the
proxy machine or phone.

Tailscale supplies authentication and encrypted transport; every device allowed
to reach this endpoint can read all projects and act as Boss, including changing
issues and controlling workers. There is no per-project access control. Host,
Origin, fetch-site, and CSRF checks remain enforced. Responses use `no-store`,
and HTTPS pages keep drafts and pending retry IDs only in page memory rather
than browser storage. Only the selected project ID is remembered across page
loads, shared by Issues, Inbox, Mindmaps, and Workers. A project in the URL
overrides that remembered selection. Drafts survive navigation and reconnects in
the same page, but disappear on reload or close. If a write's result is uncertain, reconnect in
that tab or inspect the saved issue before resubmitting after a reload. Displayed
content necessarily reaches the phone's memory; this is not a guarantee against
browser/OS snapshots or screenshots.

## Project identity

Commands infer the project from their current directory:

| Checkout | Internal identity | Display name |
| --- | --- | --- |
| Git with an origin | Normalized origin, e.g. `github.com/kamilio/hey-boss` | `hey-boss` |
| Git without an origin | Machine ID + canonical shared Git directory | Repository directory name |
| No Git | Machine ID + canonical current directory | Current directory name |

SSH and HTTPS spellings of an origin resolve to the same identity. Remote
credentials, query strings, and fragments are excluded. Worktrees share the
repository identity, including when there is no origin. Unrelated repositories
with the same final name remain separate projects. Without Git, each canonical
directory is its own project; subdirectories do not implicitly share a root.

`--project ID` selects a full project ID or an unambiguous known short name.
Ambiguous short names fail with a conflict. Outside a checkout, a full normalized
repository ID can select that repository. A new explicit short name creates a
`named:NAME` project on first successful use. `whoami` displays both the
name and full ID. Changing an origin, adding Git to a directory, or moving a
directory without an origin does not migrate existing issues; use the old full
`--project` ID to access them.

Issue numbers start at 1 per project and are never reused, even after deletion.

### Automatic projects, activity, and hiding

Projects are registered on the first successful issue command, including `list`
and `whoami`; creating an issue first is unnecessary. The local web service also
discovers repositories/directories used by running agents every 15 seconds. Git
worktrees share the same project identity. Discovery runs outside HTTP requests,
so a process scan does not block the interface. It uses live agent observations,
not a recursive scan of the filesystem or old session archives. A web service
using `--host` relies on projects registered on that SSH authority; it does not
copy local agent projects to the remote database. Use `web --no-discovery` for
isolated tests or to disable local discovery.

The project switcher sorts by the most recent agent activity or issue change.
Creation, edits, comments, claims, closure, deletion, and restoration count as
issue activity. Reading issues, polling, and rediscovering an unchanged agent do
not advance the timestamp. Equal timestamps sort by name, then full project ID.

Use the **Hide** icon beside a project to remove it from the active switcher.
**Hidden projects** lists those projects, with a **Restore** icon beside each.
Hiding preserves every issue, comment, claim, and issue number. Direct links and
CLI operations continue to work. New agent activity or new issues do not unhide
the project; restoration is explicit. Hidden state is shared by clients using
the same database, rather than being a browser-only preference.

```sh
hey-boss issue projects                 # Active projects, most recent first
hey-boss issue projects --all           # Include hidden projects
hey-boss issue hide-project --project github.com/example/old-repo
hey-boss issue restore-project --project github.com/example/old-repo
```

The schema upgrade preserves existing projects and initializes their activity
from their latest issue changes. Update all clients sharing the database; older
versions deliberately reject the upgraded schema.

## Automatic Codex workers

Run an independent worker in a checkout:

```sh
hey-boss worker --concurrency 2 --tag ready
hey-boss worker --concurrency 4                   # No tag restrictions
hey-boss worker --all-projects --concurrency 2
hey-boss worker --project poe2 --tag ready
hey-boss worker status
hey-boss worker --id WORKER_ID --json status
hey-boss worker pause WORKER_ID                  # Existing sessions continue
hey-boss worker stop WORKER_ID                   # Stop and reap its sessions
hey-boss worker --id WORKER_ID                   # Restore saved settings
hey-boss worker --host devbox --directory /home/me/poe-code --concurrency 1
hey-boss worker -C ~/Workspace/atlas -C ~/Workspace/beacon --concurrency 2
hey-boss worker --host devbox -C /work/atlas -C /work/beacon
```

Concurrency and tags belong to each worker instance. There is no global or
project cap. Repeated `--tag` requires every tag. Repeated `--project` scans
several projects; `--all-projects` scans visible projects with known checkouts.
Use `-C PATH` or `--cwd PATH` to select a checkout without changing your shell's
directory. Repeat it to pick issues from several repositories, each in its own
checkout. `--directory` remains an alias. Paths are resolved independently against
the shell's working directory; remote paths are resolved on `--host`.
Explicit `--project` filters restrict pickup to those projects; supplied checkouts
must match them. A single checkout and single project also support custom named
projects. Other selected projects use discovered checkouts. One worker can select
only one checkout per project. Saved worker IDs retain these mappings through
restart; new scope flags replace the saved scope. `--all-projects` cannot be
combined with explicit checkout paths.
Each worker displays free/busy slots, pickup pipeline, session IDs, elapsed time,
and latest Codex activity. `--json` emits status snapshots. Ctrl+C stops that
worker and reaps its Codex processes before releasing claims.

Standalone workers start from the CLI in the project directory. Managed fleet
workers also accept controls from the Workers web view. **Project settings** edits the project's
inherited prompt and PR toggle and previews the exact resulting instructions.

Pickup is an atomic reservation, leaving the issue **unassigned**. Codex must
run `hey-boss issue claim NUMBER`. Another agent cannot claim an unexpired
reservation without explicit force. `--claim-timeout SECONDS` defaults to 600
(ten minutes), starting at the first model activity. Queued model startup has a
separate fifteen-minute deadline. Saved workers retain their configured timeout;
use `--id WORKER_ID --claim-timeout 600` when starting an older saved worker to
adopt the longer window. Missed deadlines stop Codex and free the slot. Stopping or restarting a worker
reaps its agents, unassigns their unfinished issues, and makes those issues eligible
immediately. Recovery does the same for a killed worker. Failed/blocked/timed
out issues become eligible again while open and unassigned after a retry delay
that grows from 30 seconds to at most five minutes. Input and approval requests
require explicit retry; no decision is approved automatically. Claims and PR instructions are
returned by the claim command. Persistent sessions can be resumed with
`codex resume SESSION_ID`. Workers automatically resume the latest unfinished
Codex session for an eligible issue on the same machine and checkout, including
after stop/start, restart, a killed worker, or a timeout retry. The original
prompt template is rendered again so the resumed agent claims the issue before
continuing. Starting a worker with a new worker ID also resumes available sessions.
Each attempt has a new run ID while retaining its Codex session ID. Existing
claims still block pickup; completed attempts start a fresh session if reopened.
A resume failure retains the saved session ID for retry rather than discarding
the conversation. Approval requests still block for explicit manual retry.

The full prompt combines three parts, separated by blank lines:

1. Shared instructions: `Claim and implement {{issue_command}}.` (the command is wrapped in backticks).
2. Workspace: dedicated Git worktree when the worker selects it and the project allows it; existing checkout otherwise.
3. Delivery: open and attach pull requests when enabled; commit and push to main otherwise (commit only without a remote).

**Project settings** shows both branches of each choice and the assembled prompt
on the right (below the editor on small screens). The preview workspace selector
shows either worker choice without changing project policy or worker configuration.
Allowing worktrees makes both workspace prompts available; disabling them only
allows the checkout prompt. The branches used in the preview are marked.
Each branch inherits a code default; entering text creates a project override,
and **Use default** clears it. Changing an inactive branch does not change the
assembled prompt until that branch is selected. Settings apply to future jobs;
reserved jobs retain their captured configuration.

No extra cleanup or PR handoff paragraphs are appended to implementation prompts.
Put any additional agent instructions in the shared prompt or a workflow branch;
the preview, claims, and worker sessions use the same composition.

Worker completion still respects delivery mode: a completed PR-mode run keeps an
owned issue open and assigns it to Boss, preserving links and history. Boss-owned
open issues are excluded from worker pickup. Non-PR completion closes normally.
Attachment purposes distinguish fixes and prerequisites from supporting evidence.
Workers do not merge PRs.

Worktree and PR settings are off by default.
`worker --worktree` selects worktrees only when the project allows them;
`--no-worktree` or no workspace flag uses the existing checkout. Disabling
**Allow worktrees** prevents worktree use for future jobs, including saved workers
with `--worktree`. `--prs` / `--no-prs` still override project delivery choices. Worktree mode instructs
the agent to create and use a worktree before editing; the worker itself stays
in its configured checkout.

Template variables include `{{issue_command}}`, `{{title}}`, `{{body}}`,
`{{number}}`, `{{project}}`, `{{worktree_name}}`, and `{{worktree_path}}`, in shared and branch prompts. Worktree names use `<project>-<title-slug>-<number>`; the title slug is at most 15 ASCII characters, with punctuation replaced by dashes. The path is beside the worker's configured checkout, and the branch matches the directory name. The default worktree prompt tells agents to reuse an existing worktree and branch on retry or resume. Custom prompts can use either variable. The name is deterministic for the same project name, issue title, and number; renaming a project or issue changes the suggested name. The retired
`{{commit_instruction}}` variable is removed from old saved shared prompts;
delivery instructions are always assembled from the selected branch.
Prefix shared instructions with `/goal` to use a native Codex goal. A bare
`/goal` uses the default shared instructions plus selected workspace and delivery.
Issue commands use the worker’s current project automatically.

To report a problem to another project, use
`{{create_issue_command poe-code}}` in the instructions:

```text
If you encounter a safe-bash problem, report it using `{{create_issue_command poe-code}}`.
```

It expands in both the preview and claim instructions to
`hey-boss issue create --project 'poe-code' --title '<title>' --body '<markdown>'`.
The agent replaces the title and Markdown placeholders with the report.
Use an unambiguous project name or a full project ID; names with spaces are
quoted automatically. `{{create_issue_command}}` without a target creates in
the current worker project. These commands use the same authoritative issue DB.

**Project settings** edits shared instructions, worktree and PR choices, and all four branch prompts.
Worker overrides are optional; omitted values inherit project settings.

```sh
hey-boss issue settings show
hey-boss issue settings set --prs-enabled
hey-boss issue settings set --no-prs
hey-boss issue settings set --worktree
hey-boss issue settings set --no-worktree
hey-boss issue settings set --prompt 'Implement {{issue_command}}.'
hey-boss issue pr add 1 https://github.com/org/repo/pull/123
hey-boss issue pr add 1 https://github.com/org/repo/pull/124
hey-boss issue pr list 1
hey-boss issue pr remove 1 https://github.com/org/repo/pull/123
```

PR links are first-class SQLite records, support several links per issue, and
appear in CLI list/view and the issue sidebar, even when PR automation is off.
The sidebar attaches/removes links directly. Schema **4** migrates previous
project workers into independent saved workers without losing issue history.
Back up before upgrading and upgrade all clients sharing the database.

Worker API actions: `workers`, `configure_worker`, `control_worker`,
`preview_worker`, and `worker_run`. Project actions: `project_settings`,
`configure_project`. PR actions: `pull_requests`, `add_pull_request`,
`remove_pull_request`. Configuration accepts optimistic `if_version`.
Obsolete project/global worker mutation actions are rejected.

## Sessions and ownership

Identity resolution uses this order:

1. `--agent ID`.
2. `HEY_BOSS_AGENT_ID`.
3. `CODEX_THREAD_ID`, represented as `codex:THREAD_ID`.
4. A uniquely matched Codex or Claude session with a transcript held open by the
   nearest agent ancestor, or live PID-specific Claude session metadata.

If the caller's session cannot be established, commands requiring an identity
fail with instructions to supply one. They never choose the newest transcript
in a directory, borrow a parent agent's identity when a child is unresolved,
or invent a new identity on each invocation. Cached transcripts and unverified
resume targets alone are insufficient. From a terminal,
use `--agent human:NAME`, or set `HEY_BOSS_AGENT_ID` once. Agent integrations
without automatic detection should set it to a stable session identifier.

`whoami` reports the resolved ID, its source, host, available process metadata,
and project. A PID is supporting metadata, not the owner ID. Restoring a session
with the same ID preserves its claims. Nothing automatically launches or resumes
the associated agent.

| Command | Effect |
| --- | --- |
| `claim 1` | Atomically assign an open issue to this session |
| `assign-to-boss 1` | Assign an open issue to Boss |
| `unassign 1` | Clear your claim, leaving the issue open |
| `close 1` | Complete the issue, clear its claim, and record who closed it |
| `reopen 1` | Reopen the issue without assigning it |
| `reopen 1 --if-version N` | Reopen only if the issue still has revision N; conflicts exit 4 without changing the issue or history, including on `--host` |
| `delete 1` | Soft-delete the issue and clear its claim |
| `restore 1` | Recover a deleted issue in its previous open/closed state, unassigned |

An issue has at most one owner. When agents race to claim it, exactly one wins;
the others receive a conflict. Repeating your own claim or unassignment is a
successful no-op. `assign-to-myself` aliases `claim`.

`claim`, `unassign`, `close`, and `delete` require `--force` when another session
owns the claim. Forced changes are recorded in history. Editing the description,
adding labels, and commenting remain collaborative. Comments are allowed on
closed issues. Closing with `--comment` saves the comment and state change in
one transaction. Repeating `close` without a comment is a no-op; adding a closing
comment to an already closed issue conflicts (use `comment` instead).

Workers check abandoned local claims every five seconds; web discovery checks
them every fifteen seconds. A verified dead Codex or Claude process releases its
open claims once sixty seconds have passed since its last recorded issue activity.
Running agents retain claims even while idle. Remote and unverifiable processes
retain claims; a lost connection alone does not prove that a session ended.
`view` includes the assignee's saved session metadata and an advisory process
status: `running`, `stale`, or `unknown`. Process start identity prevents PID reuse
from looking like the original process. Other machines and unverifiable process
metadata report `unknown`. A stale process can still represent a resumable
session; resuming it after automatic release requires claiming the issue again.

IDs express cooperative ownership, not authentication or access control. Anyone
with access to the database or its SSH account can select an explicit identity.

## Search, labels, history, and edits

```sh
hey-boss issue create --title 'Reconnect' --label bug --label network
hey-boss issue edit 1 --label needs-review --remove-label blocked
hey-boss issue list --state closed --search reconnect
hey-boss issue list --label bug --label network --limit 10 --offset 0
hey-boss issue list --all --label bug --unassigned
hey-boss issue history 1 --limit 20 --offset 0
hey-boss issue edit 1 --body 'Updated description' --if-version 3
hey-boss issue list --state deleted
hey-boss issue restore 1
```

`list` defaults to open, nondeleted issues. `--state all` includes open, blocked, and closed
issues; `--state deleted` selects deleted ones separately. Results follow the shared queue
order. `--mine` and `--unassigned` are mutually exclusive. Repeat `--label` to
require every named label. Labels are case-sensitive, at most 64 UTF-8 bytes
each, with at most 50 labels per issue. Search is a literal substring of title
or body; case folding follows SQLite's built-in ASCII `lower()` behavior.

List summaries omit the Markdown body. `view` returns the full body and the
latest 20 comments in chronological order (fewer when they exceed the page's
16 MiB text budget). `history` exposes every event,
including complete comment text and previous description revisions. Deleted
issues remain accessible through `view` and `history`. Creation, edits, claims,
unassignment, comments, closure, reopening, deletion, and restoration are audited.
No-op commands do not add events.

`list` and `history` support `--limit` from 1 to 100 and `--offset`; JSON includes
`next_offset` when another page exists. Large history entries can cause a page to
end before `--limit`; always use the returned `next_offset`. History sorts from oldest to newest.
Concurrent new events can be picked up on the next page; filtered list pages
are snapshots of each individual invocation.

`list --all` retrieves every matching issue in queue order in one invocation;
it cannot be combined with `--limit` or `--offset`. The small copy button beside
**Queue order** in the web list copies this command with the current project,
queue host, state, search, label, and assignee filters. **Assigned to me** targets
Boss explicitly, so an agent running the copied command sees the same issues.

Every change increments an issue's `version`. `edit --if-version N` rejects
an outdated edit, preventing silent overwrites when an agent edits a description
it previously read. Omit it for an unconditional edit.

## JSON, exit codes, and retries

All public commands accept `--json`, before or after the subcommand. Successful
responses contain `ok: true`, the resolved `project`, and command-specific
`issue`, `issues`, `agent`, or `events` fields. Mutations also return `changed`.
Operational errors return `{"ok":false,"error":{"code":"...","message":"..."}}`
on stdout. Without `--json`, operational errors go to stderr. Argument-parser
errors and help use clap's normal text output, even with `--json`.

| Exit | Meaning |
| --- | --- |
| 0 | Success, including an already-completed no-op |
| 1 | Database, I/O, or transport failure |
| 2 | Invalid input or unavailable caller identity |
| 3 | Project-scoped issue not found |
| 4 | Ownership, project-name, revision, or retry-key conflict |

For operations an agent might retry after losing the response, provide a stable
request ID on the **first attempt**:

```sh
hey-boss issue create --title 'Reconnect' --body 'Details' \
  --request-id reconnect-investigation-1 --json
hey-boss issue comment 1 --body 'Tests passed.' \
  --request-id reconnect-tests-1 --json
```

Retry with the same project, actor, request ID, and operation arguments. The
database returns the original successful result, even if the issue has since
changed. Reusing that key for a different operation conflicts. Failed mutations
do not consume their keys. Request IDs are scoped to project and actor and
retained in the database. Without one, separate create/comment calls are separate
writes. Use `view` to fetch current state after replaying an older result.

## Guarded batch triage

Use `issue batch --file triage.json` (or `--file -` for stdin) to couple label
changes and ownership handoffs. The entire JSON array is **one guarded group**
within one project on one authoritative host. Every entry requires a positive
`number`, positive `if_version`, and `expected_assignee`: an exact actor ID,
`"human:boss"`, or explicit `null` for unassigned. Missing guards are invalid.

```json
[
  {
    "number": 12,
    "if_version": 7,
    "expected_assignee": "codex:session",
    "add_labels": ["rework needed"],
    "remove_labels": ["PR ready"],
    "assignment": "unassign"
  },
  {
    "number": 13,
    "if_version": 4,
    "expected_assignee": null,
    "add_labels": ["PR ready"],
    "remove_labels": ["rework needed"],
    "assignment": "boss"
  }
]
```

`assignment` accepts `keep` (default), `unassign`, or `boss`. Label arrays default
to empty; each entry must request at least one label or assignment update.
Unrelated labels, issue text, state, PR attachments and subtask relationships
are preserved. The two guards explicitly authorize changing that exact owner's
claim, even when it belongs to another actor; there is no force option. A newer
claim or an unclaimed worker reservation cannot be overridden. Boss assignment
requires an open, undrafted issue. The caller must assess PR readiness: batching
does not review or merge PRs, close issues, or control workers.

```sh
hey-boss issue batch --file triage.json --request-id review-head-abc123 --json
```

Applying requires a stable request ID. A changed issue advances once and gets one `triaged` audit event containing
its before/after labels, assignee and version. No-op entries keep their versions.

If any entry is missing, deleted, stale, invalid against current state, or
reserved, **no issues in the group change**. JSON returns `ok:true` for a processed
group, `accepted:false`, `applied:false`, and ordered `results`: offending entries
are `rejected` with an error, while other entries are `blocked`. The CLI exits 4.
Successful results use `changed`/`unchanged`. Compact `before`/`after` snapshots contain version,
assignee and labels. Check `accepted`/`applied` as well as the exit code.

Both accepted and rejected real group results are saved with the request ID in
the same transaction. Identical retries with the same project and actor return
the original result even after issues change; different payloads with that ID
conflict. To reassess a rejected group, fetch current versions/owners and use a
new ID. Invalid input and database/transport failures do not themselves save a
result; an uncertain transport outcome must retry the identical request first.

Limits are 100 unique issues and 1 MiB of input; unknown fields and conflicting
add/remove labels are invalid. `--host` / `HEY_BOSS_ISSUE_HOST` route the entire
group as one SSH RPC without local fallback. Upgrade both ends. There is no
cross-host transaction: separate calls to separate authorities commit separately.
Fleet companion replica stores refuse batches because their row-wise offline
replay cannot preserve group atomicity; use `--host SUPERVISOR` there.

## SQLite storage and remote authority

The database holds projects, issues, agent session metadata, comments, history,
and retry results. Markdown is stored as SQLite TEXT. Writes use immediate
transactions, foreign keys, WAL, and full synchronous durability. Schema version
checks reject incompatible databases. New database files use mode `0600`.

The database path is resolved in this order:

1. `HEY_BOSS_ISSUE_DB` (absolute path to the database).
2. `HEY_BOSS_STATE_DIR/issues.db` (absolute state directory).
3. `issues.db` in the installed executable's adjacent `hey-boss.state` directory.
4. macOS: `~/Library/Application Support/hey-boss/issues.db`.
5. Linux: `$XDG_DATA_HOME/hey-boss/issues.db`, or `~/.local/share/hey-boss/issues.db`.

It is separate from notification history and lives outside the checkout, so
removing a worktree does not remove its issues. Back up using SQLite's backup
facilities, or copy the database only while all writers are stopped and its WAL
has been checkpointed. Do not share a WAL database over a network filesystem.

For agents on several machines, choose one host as the authority:

```sh
export HEY_BOSS_ISSUE_HOST=devbox
hey-boss issue list
hey-boss issue claim 1 --request-id claim-reconnect-1

# One-command override:
hey-boss issue list --host devbox
```

Install the updated hey-boss CLI on that host. SSH uses existing authentication
with batch mode and no login prompts. The host resolves its own database path;
the caller supplies its project and session identity. Both machines must point
at the same normalized repository ID (or explicit shared `--project` value).
Set the authority consistently for every participating agent. Unconfigured
machines use independent local stores; there is no automatic database merge or
GitHub synchronization.

Requests and Markdown travel as JSON on SSH stdin, never interpolated into
remote shell commands. Calls have a 30-second transport deadline and the server
ignores its own routing environment while handling RPC, preventing recursion.
Disconnects never fall back to a local database or queue claims for later. A
transport failure can occur after a write committed; retry the same request ID.
Responses are limited to 64 MiB; use smaller history pages for large revisions.

## Verification

`cargo test --locked --test issues` covers the CLI lifecycle, file snapshots,
UTF-8 and size bounds, audit persistence, project isolation and worktrees,
session recovery, concurrent schema creation and issue numbering, competing
claims, optimistic edits, retry deduplication, soft deletion, and remote routing
with an SSH test double. The remote test verifies literal Markdown transport,
caller identity preservation, and no local fallback. It does not require or
contact a live SSH server.

### Project overview from the CLI

`hey-boss issue projects` lists visible projects by recent activity with open,
claimed, unassigned, closed, and deleted issue counts and their full identifiers.
Use `hey-boss issue projects --all` to include hidden projects, or
`hey-boss issue projects --json` for structured counts and activity timestamps.
In the web issue list, attached PR links are visible on each issue row and open
in a separate tab. Multiple PRs are shown individually.

### Workers while the laptop is disconnected

Run a worker on the devbox against its local authoritative issue database. Issue
claims, completion, and further pickup continue without the laptop connection.
Normal `hey-boss update` calls save to the devbox companion outbox before returning;
worker shutdown does not drain or cancel that queue. Explicit review/approval waits
still wait for a human, as requested by those commands.

The outbox replays in order and records delivery only after acknowledgment. Transport
failures and rejected valid updates remain on disk for retry. Retries use capped
exponential backoff (up to 30 seconds) with jitter; a new connection generation resets
the delay immediately. Atomic writes, file sync, and directory sync preserve the queue
across process restarts. Stable delivery IDs survive acknowledgment loss. Delivery is
at least once: interrupted acknowledgments may cause duplicate notifications, which
is preferable to losing them. Rejected malformed legacy records remain stored with an
error and do not prevent healthy notifications from being delivered. The laptop does
not overwrite the devbox’s authoritative issue database.

### Shared issue order

Drag the grip on an issue row to set the project’s queue order. An insertion line
shows where it will move; Escape cancels. Focus a grip and press Up or Down for
keyboard ordering. Touch dragging is supported, with larger mobile handles and
edge scrolling. Filters retain the shared order; moving a filtered issue relative
to another keeps the other hidden issues in their relative order. New issues
created in the UI go to the top by default; check **Add to bottom** in the editor
or Quick Add to append instead. Press **⌘⇧B / Ctrl+Shift+B** while composing to
toggle placement. Unsent drafts retain the choice; the next issue defaults to top.
CLI-created issues append to the end. Creating
in the UI keeps you on the list and highlights the new row for four seconds.
Matching filters remain; filters that exclude the new issue reset to reveal it. Closing,
deleting, reopening, and restoring retain position.

Web issue lists show all matching issues on one page, without pagination.
The same order applies to web lists, CLI lists (including filtered/paginated
output), and worker pickup and previews. Every worker reservation reads the
current queue in a fresh SQLite write transaction; no preloaded issue queue is
used. Reordering pending work while a worker is busy changes its next pickup.
Eligibility tags, existing claims, and reservation locks still apply.

```sh
hey-boss issue move 12 --before 3
hey-boss issue move 12 --after 7
hey-boss issue move 12             # Move to the end
hey-boss issue list
```

Concurrent browser reorders use a project order revision. A stale drag is rejected
and the list refreshes, preserving the other change. Moves are atomic, audited,
and can be deduplicated with the ordinary request ID. Issue content, comments,
and PR attachments remain attached to their issue numbers. Schema 6 adds the
Boss display name while preserving project instructions and assignments. Schema 5 initializes
existing queues in issue-number order.


## Automatic worker fleet

**Supervisor → Worker → Agent** is the execution hierarchy. The supervisor
coordinates the fleet and shared queue. Workers pick issues and manage the
lifecycle and retries of their Codex agents. Each agent is a coding session
implementing one issue. Machine companions synchronize replicas and apply
worker controls; they are background services, not coding agents.

`hey-boss fleet setup --source /path/to/hey-boss` installs a persistent supervisor
using the existing SSH inventory. It installs durable companions automatically and
distributes desired worker settings from `~/.hey-boss/fleet.json`. Each machine
keeps its own SQLite replica; do not mount one SQLite database over the network.
The supervisor pulls changes every five seconds, after accepting local journals,
and redeploys software when the source fingerprint changes. Active work drains
before a worker reloads an updated executable.

Fresh replicas receive compressed snapshots in bounded chunks, including rows
larger than the ordinary 16 MiB frame limit. The companion verifies the complete
transfer before applying it in one transaction. Disconnects leave its previous
data and cursor intact; local edits made during the download remain pending.
Older companions must be upgraded if their initial snapshot exceeds the limit.

The supervisor retains the latest 10,000 journal entries. Machines that fall
behind this history receive a new snapshot; their offline edits and replay
receipts remain durable. Pending local updates preserve their changed fields
while unrelated fields receive canonical updates. Cleanup frees database pages
for reuse without replacing the live database file.

The **Workers** view at `/workers` shows every configured machine, heartbeat,
connection, configuration, build, slots, tasks, events, and saved conflicts.
Pause preserves active jobs, resume enables pickup, stop cancels owned jobs, and
restart waits for the old worker to stop before restoring its stable ID.
Signals are durable and remain pending while their machine is disconnected.
`hey-boss fleet signal HOST WORKER_ID pause` provides the same control in the CLI.

Companions continue previously allocated work offline. Allocations never expire
merely because a machine disconnects; other workers cannot pick up that work.
New offline issues use reserved number ranges. Transaction journals survive
restarts and sync on reconnect with replay receipts. Different-field edits merge;
same-field conflicts and completions against changed requirements are saved for
review. Bootstrap backs up existing companion queues and flags number collisions.
This supports one supervisor, with independently operating companions.

The versioned bidirectional JSON protocol and invariants are documented in
[the fleet specification](specs/worker-fleet.md).

## Subtasks

Subtasks are ordinary issues linked to one parent in the same project. Each keeps
its own Markdown, assignee, labels, PR links, lifecycle and position in the queue.
Use **Add subtask** beside Edit, or press **Shift+N** while viewing an issue, to
create a child. The shortcut stays inactive while typing or using a dialog.
The **Subtasks** card appears only when children are linked; use **Add existing**
there to link another issue.
Children link back to their parent; the issue list shows completion progress.
Unlinking keeps the issue and its subtree. Closing, deleting or reopening a parent
does not change its children. Deleted children are excluded from progress but can
still be unlinked. Nesting supports eight levels and 100 direct children per parent;
cycles, including through deleted issues, are rejected transactionally.

```sh
hey-boss issue subtask create 12 --title 'Implement the API' --body '## Requirements'
hey-boss issue subtask add 12 15
hey-boss issue subtask list 12
hey-boss issue subtask list 12 --all --json
hey-boss issue subtask remove 12 15
```

`create` accepts Markdown stdin or `--body-file`, labels, version checks and the
usual `--request-id`. Link/unlink accept `--if-version` for the parent and
`--if-child-version` for the child. A child is created and linked atomically; failed
link validation rolls back its number, queue position, revisions and history.
Web-created children prepend by default; **Add to bottom** appends instead.
CLI-created children append. Dragging or using the
arrow keys on a child handle updates the shared project queue, which governs
web, CLI and worker order.

Automatic worker pickup waits for all reachable open descendants, including those
under a closed intermediate issue. Deleted subtrees are excluded. Manual claims
remain available for coordination. Fleet allocation uses the same readiness view,
so blocked parents do not strand their leaves. Offline graph edits use durable
journals and receipt replay. Conflicts preserve the attempted payload and restore
canonical relationships; temporarily blocked canonical links remain durable across
restarts and prevent premature parent pickup until they can be reconciled.

Schema 8 adds graph guards and readiness; schema 9 adds durable graph receipts and
deferred canonical relationships. Both preserve existing issue data and global
profile settings. Older binaries reject the newer schema.

## Drafts and terminal planning

Create a draft only when the user explicitly requests one. Ordinary issue
creation remains runnable; planning discussions alone do not imply draft status.

```sh
hey-boss issue create --title 'Feature' --draft
hey-boss issue edit 12 --draft
hey-boss issue undraft 12
hey-boss issue settings set --no-drafts
hey-boss issue settings set --drafts-enabled --plan-template 'plans/{timestamp}-{number}.md'
```

Drafts are ordinary persisted issues, visible in lists and details, excluded from
worker reservation and manual claims. Drafting requires an open, unassigned,
unreserved issue. Disabling drafts rejects new drafts and interactive planning;
it preserves existing drafts and allows explicit undrafting. The web creation
form offers a Save as draft choice that explains agent pickup and updates the
save action. Lists distinguish drafts with a badge and a pencil icon. Open draft
details explain their status and offer Mark ready; issue properties offer Move
to draft with the eligibility restriction when unavailable. Existing drafts can
still be marked ready after drafts are disabled. Linked plans display their file
and host, with a reminder that marking ready first syncs the latest plan.

Interactive planning is a human-terminal workflow. Agents must not start it
unless explicitly asked:

```sh
hey-boss issue create --title 'Feature' --interactive
hey-boss issue edit 12 --interactive
hey-boss issue edit 12 --draft --interactive --file plans/existing.md
```

Creation with `--interactive` implies `--draft`. Without interactive mode,
`--file` remains a one-time Markdown body import. Interactive mode binds one
existing file inside the checkout, or seeds a new Markdown file with `# Title`
and the issue body. Its first level-one heading supplies the issue title; the
remaining Markdown supplies the body. Links to sibling documents stay as links.

The project template defaults to `plans/{timestamp}-{number}.md`; `{timestamp}`
is a UTC timestamp, and `{year}`, `{month}`, `{day}`, `{hour}`, `{minute}`, and
`{second}` are also supported. Missing directories are created. The resolved
repository-relative path, checkout, machine, and host are saved once; resuming
uses that location and changing the title does not rename the file.

Before launching Codex, existing file content is compared with the issue. Matching
content prompts for nothing. Differences display both versions and require the
human to choose `hey-boss` (write the issue to the file) or `file` (sync the file
to the issue). A blank answer cancels without overwriting either version or
launching Codex. An existing sync owner pauses during reconciliation and remains
paused on cancellation until reconciliation or explicit undrafting succeeds.

Codex starts in the checkout with exactly `We are planning in <path>`. One
local detached process syncs file edits every ten seconds, without creating
mutations for unchanged content. Normal Codex exit syncs the file and undrafts;
an unsuccessful exit or failed final sync leaves the draft intact. Manual and
web undrafting also require a successful final sync from the owning machine.
A missing file or unreachable machine leaves the issue drafted with an error.

Sync continues after Codex exits, after undrafting, and after assignment. Assigned
file-bound issues display a warning because plan edits continue updating them.
If sync is interrupted, resume interactive planning on the original machine and
checkout; an already runnable bound issue retains its status. Workers receive
the plan path as context and implement from the synced body. This flow neither
commits nor transfers the plan document.

## Progress status

An owner can publish green, orange or red progress updates with
`hey-boss issue status NUMBER green --comment 'The fix passes tests. Checking the phone layout next.'`.
The issue list and read-only web viewers show the current message; its separate
history starts collapsed. Use comments for lasting findings and final verification.
See [Issue progress](issue-status.md) for ownership rules, periodic updates and sync.
