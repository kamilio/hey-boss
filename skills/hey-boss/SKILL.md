---
name: hey-boss
description: Background notifications, project issues, and secrets kept out of agent context.
---

## Notifications

Notify once for substantial background results or essential blocking decisions. Keep active chat, routine progress, and tests in chat. One outcome sentence; brief details.

Notification commands infer the project from Git/directory, or inherit
`HEY_BOSS_ISSUE_PROJECT` in worker sessions. `--project` is optional and uses the
same registry as `issue projects`: full IDs, unambiguous short names, or a new
custom name. `--title` remains required.

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

Project defaults to the repository across worktrees. Fleet companions keep durable local replicas and sync automatically with their supervisor. Standalone queues require the same `--host` / `HEY_BOSS_ISSUE_HOST` to target another machine.

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

Create a draft only when the user explicitly asks for a draft. Otherwise create an ordinary issue; do not infer draft status from planning, incomplete requirements, or exploratory discussion. Interactive mode is a human-facing terminal workflow. Agents must not start it unless explicitly asked.

Use `issue create --title TITLE --draft` for a persisted draft without files or sessions, `issue edit NUMBER --draft` to draft an eligible issue, and `issue undraft NUMBER` to make it runnable. Explicit human planning uses `create --interactive` or `edit NUMBER --interactive`, optionally with `--file PATH`. File-bound issues keep syncing every ten seconds after planning ends; assigned issues continue receiving plan edits.

Claim before work; stop on conflict (exit 4). Never force another session's claim without authorization. Use stable `--agent` if needed. Workers and web discovery release open claims of verified dead local Codex/Claude processes after sixty seconds since their last recorded issue activity. Idle live agents and remote/unverifiable processes keep claims. Reclaim before resuming released work; `unassign` releases explicitly.

Reuse `--request-id` for identical uncertain retries; `view` reads current state; `edit --if-version N` protects concurrent changes. Test with separate `HEY_BOSS_ISSUE_DB` and `web --no-discovery`.

`issue reopen NUMBER --if-version N` reopens only if the issue still has the version
read with `view`, including on `--host`. A mismatch returns conflict (exit 4) before
changing state, assignment, PR links or history. Omit the guard for unconditional
reopening. Retry an identical successful `--request-id` to receive its original
result, even if the issue has changed since.

## Guarded issue triage batches

Use `issue batch --file triage.json --dry-run --json` to preview and
`issue batch --file triage.json --request-id review-head-ID --json` to apply.
`--file -` reads stdin. Input is a JSON array of up to 100 unique issues / 1 MiB:

```json
[{"number":12,"if_version":7,"expected_assignee":"codex:session","add_labels":["rework needed"],"remove_labels":["PR ready"],"assignment":"unassign"}]
```

Every entry requires a version and exact owner guard (`null` means unassigned).
`assignment` is `keep` (default), `unassign`, or `boss`; arrays default empty.
For caller-verified ready work, invert those labels and use `boss`. Guards
authorize handing off the exact expected claim, including another actor's;
newer claims and unclaimed worker reservations are protected. The entire array
is atomic within one project/authority. Any rejection blocks all changes;
CLI exit 4 retains compact JSON `accepted:false`, `applied:false`, ordered
`results` with `rejected` errors and `blocked` entries. Success returns compact
before/after labels, owner and version; each changed issue advances once.
Preview saves nothing and forbids a request ID. Applying requires one; identical
retries return the original accepted or rejected result. Changed payloads conflict;
reassess rejected guards with fresh reads and a new ID. Batch never reviews,
merges, closes issues or controls workers; the caller assesses readiness.
Use `--host SUPERVISOR` from fleet companions: replica stores reject batches
because offline row replay cannot preserve atomicity. SSH sends one group with
no local fallback; separate authorities have separate transactions.

## GitHub issue imports

`hey-boss issue drain-github --dry-run` previews issues created by the authenticated
GitHub user. Omit `--dry-run` to copy them into the current project and delete each
GitHub original after read-back verification and a source-change check. Requires
Python 3 and authenticated `gh`. Use `--author LOGIN`, `--all-authors`, `--repo
OWNER/REPO`, `--project PROJECT`, or `--state all` to override the defaults.
Retries reuse the copy; conflicts retain the original. Do not change the target
project when retrying. GitHub deletion is unconditional, so a last-moment edit can
still race the final check. See `docs/github-drain.md` in the source repository.

## Issue workers

Use **Supervisor → Worker → Agent** consistently: the supervisor coordinates the
fleet, workers pick issues and manage agent lifecycle/retries, and agents are
Codex coding sessions. A machine companion synchronizes its replica and applies
worker controls. `fleet supervisor` and `fleet companion` run these background
services; `fleet setup` manages their installation.

`hey-boss worker --concurrency 2 --tag ready` runs an independent worker; omit
`--tag` for unrestricted pickup. Standalone queues are per machine. `hey-boss fleet setup --source /path/to/hey-boss` enables automatic configuration, software deployment, and replica sync for the saved SSH inventory. Agents continue allocated work offline and replay durable changes on reconnect. `/agents` groups tasks by project with device labels and separate live conversation pages. Saved requests, replies and tool activity load in pages; disconnected tasks show their last known state. Paired web devices read through the authenticated supervisor bridge. Manage devices contains service controls; `/workers` remains an alias; `fleet status` shows the same fleet in the CLI. The terminal shows the
queue host and database.
In a live Agents conversation, **Take over** stops only that agent and assigns
its issue to Boss. Once stopped, **Copy command** provides a terminal resume
command with SSH and the checkout directory for remote sessions. Paired devices
support the same action. Other agents and worker pickup settings are preserved.
Use `worker --host HOST --directory /remote/checkout` (or set `HEY_BOSS_ISSUE_HOST`)
to run the worker and its Codex sessions on the authoritative SSH host. New remote
workers require a remote checkout path (or `--all-projects` to discover known
checkouts); remote status/stop/pause also work. Every worker owns its slots and
filters, without shared project/global caps. `worker status` shows slots, pipeline, runtimes and
activity. Active agents and recent history are separate; `--history 0` hides
finished attempts. Open, unassigned unsuccessful issues retry automatically after
a delay of 30 seconds to five minutes. Approval/input requests require explicit
retry. Stopping/restarting workers stops owned agents and releases unfinished claims for immediate pickup; killed workers are recovered the same way. Pickup resumes the latest unfinished Codex session on the same machine and checkout, even with a new worker ID, and sends the original prompt template again so the agent claims before continuing. Timeout retries also resume; completed attempts start fresh if reopened, and existing claims block pickup. Resume failures retain the saved session ID for retry. Workers drain active sessions and reload after a CLI replacement.
`worker pause ID` drains, `worker stop ID` stops its sessions, and
`worker --id ID` restores settings. Standalone workers launch from the CLI. Fleet controls persist desired intent and can resume or restart managed workers through the web app. Use `worker restart ID` locally or `worker --host HOST restart ID` from the supervisor machine; `fleet signal HOST ID restart` also works. These queue durable signals, retain the worker ID/settings, and keep the supervisor and companion running. Acknowledgment requires the replacement to register. Failed restarts retry with backoff; a new stop supersedes unfinished restart intent.
Pickup reserves an unassigned issue; the default ten-minute manual claim window (`--claim-timeout`) starts with the first model activity. Saved workers retain their configured timeout; start with `--id ID --claim-timeout 600` to update an older two-minute setting. Model startup has a separate fifteen-minute bound;
**the agent must claim manually** using `hey-boss issue claim NUMBER`.
Claim output includes project instructions and PR attachment commands.
PRs are disabled by default; project settings or `worker --prs` enable them.
Attach one or more with `hey-boss issue pr add NUMBER URL`; list/remove through
`issue pr list/remove`. They remain visible in CLI and UI.
Record a link's role with `issue pr add NUMBER URL --purpose fix|prerequisite|supporting-evidence|unspecified`.
Use `issue pr classify NUMBER URL --purpose PURPOSE` to change an existing link
without reattaching it. Older links are `unspecified`; adding an existing URL
preserves its purpose. `view` and `pr list` JSON expose `purpose`. Review every PR,
using purpose to distinguish merge requirements from supporting material;
classification alone never closes an issue or assigns it to Boss.
In PR workflow mode, keep the issue open until its actual fix PR is merged.
Passing CI or a ready-for-review handoff is not a merge. Continue the existing
session through required reviews, feedback, findings and conflicts. Only when
fully merge-ready, comment with verification and the remaining merge step, then
`issue assign-to-boss NUMBER` and report completed; Boss ownership prevents worker
pickup. If blocked, report blocked; do not hand incomplete work to Boss. Worker
completion in PR mode preserves an open issue and assigns it to Boss rather than
closing it. Custom PR prompts inherit these lifecycle rules. Explicit source/group
closure and non-PR completion can still close normally. Supporting evidence PRs
do not all need to merge. No automatic merging is performed.
Prefix the prompt with `/goal` to enable native Codex goals; no toggle is needed.
A bare `/goal` uses the default instructions. Issue commands inherit the worker project. The shared prompt defaults to `Claim and implement` followed by the backtick-wrapped
`{{issue_command}}`. Project settings assembles shared instructions + selected
workspace branch + selected delivery branch and previews the exact result.
Worktree and PR modes default off. Each branch has a code default and an optional
project override; **Use default** clears an override. `worker --worktree` /
`--no-worktree` and `--prs` / `--no-prs` override project choices. Worktree mode
instructs the agent to create a dedicated Git worktree before editing.
Without PRs, delivery commits and pushes main when a remote exists (commit only
otherwise); PR mode opens and attaches every PR. `{{commit_instruction}}` is
retired and stripped from legacy shared templates. A bare `/goal` uses default
shared instructions plus selected branches.
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
or omit the anchor to move to the end. UI-created issues go to the top by default;
the editor and Quick Add offer **Add to bottom** (⌘⇧B / Ctrl+Shift+B).
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
Mindmaps are not replicated to fleet companions; use `--host SUPERVISOR` (or
`HEY_BOSS_ISSUE_HOST`) there for authoritative reads/edits and the web viewer.

```sh
hey-boss mm add 'Release' --id release
hey-boss mm issue 12 --under release
hey-boss mm issue 12 --under release --title 'Ship API' --if-version 5 --request-id ship-api
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
`mm batch --file edits.json` (or `--file -` for stdin) atomically applies a JSON
array of `edit` (`node`, `title` or `clear_label`), `alias` (`node`, `alias`),
`move` (`node`, optional `under`/`before`/`after`), and `link` (`from`, `to`, `kind`,
optional `description`) entries. Each object requires the
exact `command` discriminator, not `action`; `mm batch --help` shows the complete
format and copy-ready examples. For existing selectors, save this as `edits.json`:

```json
[
  {"command":"edit","node":"issue:12","title":"Keep replies safe"},
  {"command":"alias","node":"followup","alias":"reply-followup"},
  {"command":"move","node":"followup","under":"existing-topic"},
  {"command":"link","from":"followup","to":"issue:12","kind":"depends-on","description":"Replies need this fix"}
]
```

```sh
hey-boss mm batch --file edits.json --dry-run --if-version 42 --json
hey-boss mm batch --file - --dry-run --if-version 42 --json < edits.json
hey-boss mm batch --file edits.json --if-version 42 --request-id organize-replies --json
```

Replace 42 with the map version from `mm show --json`. Preview with `--dry-run`
and guard the whole batch with `--if-version`. All selectors bind before edits;
use the original alias or stable node ID in later entries, never a new alias
introduced by an earlier entry. Omitted/null `alias` clears it; omitted/null
`under` moves to the root; omitted sibling anchors append. Use only one of
`before`/`after`. `clear_label:true` restores an issue's title and excludes `title`.
Unknown fields/commands and body edits are rejected. Limits: 1 MiB/10,000 entries.
Invalid nodes, duplicate requested labels/aliases, cycles and stale versions roll
back everything. Link entries follow `mm link`: cross-project endpoints and missing
typed resource references are supported; missing aliases/IDs and self-links fail.
Required `kind` is nonblank, at most 64 bytes, without control characters;
`pull-request` is reserved. `description` is at most 16 KiB; null, omission or
blank text clears it. Invalid descriptions/kinds reject the whole transaction.
JSON returns compact `changed_nodes` and `changed_links` before/after metadata
(stable `from`/`to`/`kind`, description objects; null before means a new link),
one resulting version and `affected_projects`. Each affected map advances once;
the single guard applies to the selected map. Empty/net no-op batches do not
advance versions. Identical
`--request-id` retries are safe after alias changes; dry runs cannot use request IDs.

`mm move NODE --under PARENT` rehomes a node; `--before/--after` reorder siblings.
`mm alias NODE NAME` changes a readable alias; `--clear` removes it without changing
the generated node ID or links. Aliases must remain unique within their project.
`mm pr URL --title LABEL` gives a PR a readable label; `mm edit NODE --title LABEL`
changes it while preserving URL/attachments/dependencies. Issue and notice text stays
live; issue and PR nodes accept title edits only. `mm edit issue:12 --title LABEL`
sets a map-only issue label; `mm edit issue:12 --clear-label` restores its live
title. Details retain the original title and search matches both titles.
`mm issue NUMBER --title LABEL` sets an initial map-only label atomically with
creation and nesting, using one version guard/increment. On an existing reference,
it updates only the label; supplied alias/parent must match (use alias/move to
change them). Omitted placement is preserved. Matching labels are no-ops;
identical request-ID retries return the original result. Cross-project references
support the same labels. Without --title, duplicate references remain errors.
`view` prints the PR URL and export links its label.
`view NODE --bodies preview|none|full` controls focused text reads (default full).
Live issue assignments appear in the viewer and `view`; viewer search matches
assignment names/IDs and state. Show less fetches a fresh preview after a full read.
The web viewer opens a branching map with pan/zoom, collapsible branches and a
clickable overview. Select cards for live details and relationships; Outline
switches to nested reading. Search reveals matches without moving keyboard focus.
Previous/next search buttons and Enter/Shift+Enter cycle through matches.
`mm remove NODE --recursive` removes a subtree and its graph links, preserving resources.
Terminal outlines omit bodies by default; JSON defaults to full bodies. Use
`mm view NODE` for one full live node, `show --bodies preview|none|full` to control
map body size, and `export` for complete Markdown. The viewer loads 512-character
previews and fetches full bodies on demand; search covers previews and loaded text.
Reads have a 32 MiB response budget. If a complete map exceeds it, use preview/none
bodies, `view NODE` for text, or `links NODE` for a focused read with bodies omitted.
Mutations retain all saved content regardless of read size.

## Artifacts

`hey-boss artifact` manages persistent project Markdown documents in the issue store.
Use `create --title TITLE --file plan.md` (or `--body`, '-' for stdin), `list --query TEXT`,
`view ID --json`, `edit ID --file updated.md --if-version N`, and `export ID` for Markdown stdout.
`--issue NUMBER` or `--node ALIAS_OR_ID` on create/link attaches the same document;
`unlink ID --issue NUMBER` or `--node NODE` preserves it. `links` reads resource attachments.
`comment ID --body TEXT --quote SELECTED_TEXT` or `--parent COMMENT_ID` adds discussions.
`resolve ID COMMENT_ID` and `--reopen` retain thread history. `archive/restore ID --if-version N`
retain stable references. Project/host/agent/request IDs follow issue conventions.
Use the authoritative supervisor host from companions; artifacts, like mindmaps, are not
fleet issue replicas. Web/Fly use existing project authentication and the supervisor bridge.
Test using a separate `HEY_BOSS_ISSUE_DB`. Interrupted mutations must retry the same request ID
and payload; stale document revisions must be merged explicitly rather than overwritten.

## File attachments

`hey-boss attachment upload PATH --issue NUMBER` stores any regular file up to
10 MiB on the authoritative host. Use `--node SELECTOR` or `--artifact ID` instead
for mindmap nodes and artifacts. `list --issue NUMBER --json` exposes file IDs,
filenames, sizes and SHA-256. `download FILE_ID` materializes a private temporary
copy on the caller's machine; `--output PATH` selects a filename or existing
directory. Existing files are never overwritten. `remove FILE_ID` deletes the file.
Project/host/actor/request-ID conventions match issues. From fleet companions use
`--host SUPERVISOR`; file contents are not part of issue replica row sync. Reuse
identical request IDs for uncertain upload/removal retries. The web resource views
support file selection, drag-and-drop upload, download and confirmed removal.
