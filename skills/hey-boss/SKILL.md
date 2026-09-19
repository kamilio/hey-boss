---
name: hey-boss
description: Background notifications, project issues, and secrets kept out of agent context.
---

## Notifications

Notify once for substantial background results or essential blocking decisions. Keep active chat, routine progress, and tests in chat. One outcome sentence; brief details.

```sh
hey-boss alert --project Atlas --title Ready 'Ready for review.' --link-url PR_URL --link-label 'Merge PR'
hey-boss update --project Atlas --title Review 'Please review.' --file PATH --comments
```

Use `ask --sync` to wait, or `ask --async` then `wait TASK_ID`. Cancellation is never approval; do not automatically re-ask. Remote `pending` does not confirm delivery.

## Web Inbox and issue relationships

`hey-boss inbox` opens the shared web Inbox; `inbox --json` lists notices.
The menu-bar Inbox opens the same page. The native Inbox window is removed;
notification banners, questions, and document previews still work.
Ordinary notices become read when opened. Questions and document reviews stay
pending until answered, finished, or cancelled; cancellation is never approval.

Add an optional issue relationship when creating a notice:

```sh
hey-boss alert --project poe2 --title Ready 'Ready for review.' --issue 123
```

`--issue` defaults to the current Git/directory issue project; use
`--issue-project FULL_ID --issue-host HOST` for an explicit remote issue.
`HEY_BOSS_ISSUE_HOST` supplies the issue host when configured.
Existing notices can be linked/unlinked in the web Inbox, with backlinks on issues.
The relationship never claims/closes/reopens an issue, changes its content/revision,
or completes a notice. Linked creations retain ordinary offline queue behavior.

## Secrets

Use only `secret`, never chat/ordinary prompts. Never inspect/screenshot the window or read, print, diff, or index the destination. Synthetic test values only.

```sh
hey-boss secret --field API_KEY --env-file .env
hey-boss secret --field LOGIN --field PASSWORD --login --env-file .env
hey-boss secret --field API_KEY -- python3 app.py
(set -C; umask 077; hey-boss secret --field API_KEY --stdout > .env)
```

Secrets bypass history/mobile/offline queues. Child output is suppressed; consumers must not log credentials. Never carry values through tool arguments, `$(...)`, `tee`, or tracing. Check only exit status/non-secret metadata. Keep files out of Git/context.

Files must be private; existing keys, links, and tracked files are refused. Redirection requires a new private file; cancellation may leave it empty. File quoting is POSIX; use child environment mode for other dotenv parsers. Remote secrets need a live updated companion; destinations belong to the caller. Do not automatically retry cancellation.

## Issues

Project defaults to the repository across worktrees. Fleet agents keep durable local replicas and sync automatically with their controller. Standalone queues require the same `--host` / `HEY_BOSS_ISSUE_HOST` to target another machine.

```sh
hey-boss issue list --unassigned --json
hey-boss issue create --title 'Fix reconnect' --body 'Describe the problem' --request-id reconnect-1
hey-boss issue claim 1
hey-boss issue comment 1 --body Investigating
hey-boss issue close 1 --comment 'Fixed and verified'
hey-boss issue web
```

The web app always acts as Boss (`human:boss`). Assign human work with
`hey-boss issue assign-to-boss NUMBER`; list it with `issue list --assignee boss`.
Rename Boss globally with `hey-boss settings set --boss-name NAME`, or open
Settings from the web profile badge. Use `hey-boss settings show` to read it.
The profile is shared across projects in the issue store; the stable ID and
assignments remain unchanged. Boss-assigned issues
are excluded from worker pickup. Web list labels and assignees are clickable filters.

Claim before work; stop on conflict (exit 4). Never force another session's claim without authorization. Use stable `--agent` if needed. Exit/disconnect does not release claims; `unassign` does.

Reuse `--request-id` for identical uncertain retries; `view` reads current state; `edit --if-version N` protects concurrent changes. Test with separate `HEY_BOSS_ISSUE_DB` and `web --no-discovery`.

## Issue workers

`hey-boss worker --concurrency 2 --tag ready` runs an independent worker; omit
`--tag` for unrestricted pickup. Standalone queues are per machine. `hey-boss fleet setup --source /path/to/hey-boss` enables automatic configuration, software deployment, and replica sync for the saved SSH inventory. Agents continue allocated work offline and replay durable changes on reconnect. `/workers` shows live connections, sync, tasks, and worker controls; `fleet status` shows the same fleet in the CLI. The terminal shows the
queue host and database.
Use `worker --host HOST --directory /remote/checkout` (or set `HEY_BOSS_ISSUE_HOST`)
to run the worker and its Codex sessions on the authoritative SSH host. New remote
workers require a remote checkout path (or `--all-projects` to discover known
checkouts); remote status/stop/pause also work. Every worker owns its slots and
filters, without shared project/global caps. `worker status` shows slots, pipeline, runtimes and
activity. Active sessions and recent history are separate; `--history 0` hides
finished attempts. Open, unassigned unsuccessful issues retry automatically after
a delay of 30 seconds to five minutes. Approval/input requests require explicit
retry. Workers drain active sessions and reload after a CLI replacement.
`worker pause ID` drains, `worker stop ID` stops its sessions, and
`worker --id ID` restores settings. Standalone workers launch from the CLI. Fleet controls persist desired intent and can resume or restart managed workers through the web app. Use `worker restart ID` locally or `worker --host HOST restart ID` from the controller machine; `fleet signal HOST ID restart` also works. These queue durable signals, retain the worker ID/settings, and keep the controller and companion running. Acknowledgment requires the replacement to register. Failed restarts retry with backoff; a new stop supersedes unfinished restart intent.
Pickup reserves an unassigned issue; the 120-second manual claim window (`--claim-timeout`) starts with the first model activity. Model startup has a separate fifteen-minute bound;
**the agent must claim manually** using `hey-boss issue claim NUMBER`.
Claim output includes project instructions and PR attachment commands.
PRs are disabled by default; project settings or `worker --prs` enable them.
Attach one or more with `hey-boss issue pr add NUMBER URL`; list/remove through
`issue pr list/remove`. They remain visible in CLI and UI.
Prefix the prompt with `/goal` to enable native Codex goals; no toggle is needed.
A bare `/goal` uses the default instructions. Issue commands inherit the worker project. The editable prompt is
`Assign and implement \`{{issue_command}}\`. {{commit_instruction}}`.
The project Instructions UI previews the exact two sentences: without a remote commit only; with a
remote push main, or open/attach PRs when enabled. No hidden block is appended.
Use `{{create_issue_command poe-code}}` in custom instructions to expand a create
command targeting poe-code explicitly, including title/Markdown placeholders.
Replace those with the report. Full project IDs and names with spaces work;
omit the target to create in the current project. Preview and claim output share
this expansion and the worker's authoritative issue DB.
Devbox workers use their local authoritative issue DB and continue while the laptop is
disconnected. Ordinary updates are durably queued by the companion and do not delay
completion, pickup, or shutdown. Replay is FIFO with acknowledgments and exponential
backoff; duplicates after acknowledgment loss are acceptable. Explicit human review
or approval waits still require a reply.
Test against an isolated DB. Approval requests block for manual resumption.

## Issue priority order

Web drag-and-drop, filtered/paginated CLI lists, and worker pickup share a persistent
project queue order. Use `hey-boss issue move NUMBER --before OTHER`, `--after OTHER`,
or omit the anchor to move to the end. UI-created issues go to the top;
CLI-created issues append. Lifecycle changes keep
position. Each worker reserves from fresh ordered SQLite data, respecting tags
and claims. Concurrent browser moves reject stale queue revisions and refresh.

## Reference

`hey-boss upgrade` updates this installation and every registered SSH companion.
Use `--source /path/to/hey-boss` to install and remember a development checkout;
without a configured checkout it fetches upstream main. `--check` only reports
build mismatches; `--local-only` or repeatable `--host HOST` limits targets.
Matching source build IDs skip compilation. Failed hosts are reported separately;
rerun after reconnecting. Python 3 and Rust must be available on each target.

`hey-boss COMMAND --help`; companions may need `~/.local/bin/hey-boss`.
Edit only the repository skill; rebuild/install the Mac CLI, then `hey-boss companion sync-skill`. Existing chats must reload.

## Subtasks

`issue subtask create PARENT --title TITLE --body MARKDOWN` creates and links an
ordinary issue atomically. Use `subtask add PARENT CHILD`, `list PARENT [--all]`,
or `remove PARENT CHILD` to link, inspect or unlink. Unlinking preserves the issue.
A child has one parent in the same project; cycles are rejected. Relationships
support eight levels and 100 direct children. Each issue keeps its Markdown,
labels, ownership, PRs and lifecycle; changes never cascade. Siblings follow the
shared queue order. Workers wait for unfinished reachable descendants, even
through closed children, while deleted subtrees do not block pickup. Manual claims
remain available. Use parent/child version checks and identical request IDs for
uncertain retries. Fleet journals retain offline edits and conflicting payloads.

## Mindmaps

`hey-boss mm` shows a project's nested outline. Author from the CLI; `mm web` is a
read-only viewer. `--project`, `--host`, `--agent`, `--json`, `--request-id` and
mutation `--if-version` follow issue conventions. Use an isolated issue DB in tests.

```sh
hey-boss mm add 'Release' --id release
hey-boss mm issue 12 --under release
hey-boss mm link issue:12 Platform::api --kind depends-on --why 'API must land first'
hey-boss mm link pr:https://github.com/org/repo/pull/2 pr:https://github.com/org/repo/pull/1 --kind depends-on
hey-boss mm show
hey-boss mm view release
hey-boss mm show --bodies preview --json
hey-boss mm web
```

Aliases are project-local; qualify cross-project selectors as `PROJECT::alias`.
Links add missing typed `issue:NUMBER`, `pr:URL` or `notice:TASK_ID` references atomically.
Descriptions are optional; A depends-on B means A waits for B. Nesting stays within
one project; cycles are rejected. Automatic issue→PR relationships come from
`issue pr add/remove`. Only confirmed pending Inbox notices appear; an unavailable
Inbox is reported explicitly. Map reads never complete notices or change issues.
`mm move NODE --under PARENT` rehomes a node; `--before/--after` reorder siblings.
`mm alias NODE NAME` changes a readable alias; `--clear` removes it without changing
the generated node ID or links. Aliases must remain unique within their project.
`mm remove NODE --recursive` removes a subtree and its graph links, preserving resources.
Terminal outlines omit bodies by default; JSON defaults to full bodies. Use
`mm view NODE` for one full live node, `show --bodies preview|none|full` to control
map body size, and `export` for complete Markdown. The viewer loads 512-character
previews and fetches full bodies on demand; search covers previews and loaded text.
Reads have a 32 MiB response budget. If a complete map exceeds it, use preview/none
bodies, `view NODE` for text, or `links NODE` for a focused read with bodies omitted.
Mutations retain all saved content regardless of read size.
