---
name: hey-boss
description: Background notifications, project issues, and secrets kept out of agent context.
---

## Notifications

Notify once for substantial background results or essential blocking decisions. Ask yourself - should I page user for this? Keep active chat, routine progress, and tests in chat. One outcome sentence; brief details.
```sh

hey-boss alert --title Ready 'Ready for review.' --link-url PR_URL --link-label 'Merge PR'
hey-boss update --title Review 'Please review.' --file PATH --comments
```

Use `ask --sync` to wait, or `ask --async` then `wait TASK_ID`. Cancellation is never approval; do not automatically re-ask. Remote `pending` does not confirm delivery.

```sh
hey-boss alert --project poe2 --title Ready 'Ready for review.' --issue 123
```

`--issue` defaults to the current Git/directory issue project; use
`--issue-project FULL_ID --issue-host HOST` for an explicit remote issue.

## Secrets

Use only `secret`, never chat/ordinary prompts. Never inspect/screenshot the window or read, print, diff, or index the destination. Synthetic test values only.

```sh
hey-boss secret --field API_KEY --env-file .env
hey-boss secret --field LOGIN --field PASSWORD --login --env-file .env
hey-boss secret --field API_KEY -- python3 app.py
(set -C; umask 077; hey-boss secret --field API_KEY --stdout > .env)
```

## Issues

```sh
hey-boss issue list --unassigned --json
hey-boss issue list --all --label ready --unassigned --json
hey-boss issue create --title 'Fix reconnect' --body 'Describe the problem' --request-id reconnect-1
hey-boss issue claim 1
hey-boss issue status 1 green --comment 'Checking what causes the reconnect failure.'
hey-boss issue comment 1 --body 'Sleep drops the connection before the retry timer starts.'
hey-boss issue close 1 --comment 'Fixed and verified'
```

Assign human work with
`hey-boss issue assign-to-boss NUMBER`; list it with `issue list --assignee boss`.

Use `issue create --title TITLE --draft` for a persisted draft without files or sessions, `issue edit NUMBER --draft` to draft an eligible issue, and `issue undraft NUMBER` to make it runnable. 

While you own an issue, publish a one-line short status after claiming, when the next step
or risk changes, and at least every ten minutes during active work:
`hey-boss issue status NUMBER green --comment 'The fix passes tests. Checking the phone layout next.'`

`issue block NUMBER --comment REASON` moves an open issue to Blocked and releases
its claim; `list --state blocked` finds paused work. Add repeatable `--by NUMBER`
to link the issues that must close first. `issue blocked-by NUMBER BLOCKER...`
sets these links on an existing issue; omit BLOCKERs to remove them. Blocking should be rare:
make every effort to resolve the issue, raise questions via `hey-boss ask`, and
ask the user for help before giving up. Explain the blocker and what enables
progress. `issue reopen NUMBER` resumes eligibility


## Issue workers

`hey-boss worker --concurrency 2 --tag ready` runs an independent worker; omit
`--tag` for unrestricted pickup. Standalone queues are per machine. `hey-boss fleet setup --source /path/to/hey-boss` enables automatic configuration, software deployment, and replica sync for the saved SSH inventory.

`hey-boss worker --json --id WORKER_ID --history 0 status` shows only active or
pending attempts. Live paused or approval-waiting attempts remain visible;
`finished_at` identifies terminated attempts, regardless of their state label.
Use `--history 20` to include up to twenty recent finished attempts. The same
limit applies to watch and streaming JSON output; the terminal dashboard's
History tab remains available.

## Issue priority order

 Use `hey-boss issue move NUMBER --before OTHER`, `--after OTHER`,


`hey-boss COMMAND --help`; 

## Subtasks

`issue subtask create PARENT --title TITLE --body MARKDOWN` creates and links an
ordinary issue atomically. Use `subtask add PARENT CHILD`, `list PARENT [--all]`,
or `remove PARENT CHILD` to link, inspect or unlink. Unlinking preserves the issue.

## Mindmaps

`hey-boss mm` shows a project's nested outline. Author from the CLI;

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

## Artifacts

`hey-boss artifact` manages persistent project Markdown documents in the issue store.
Use `create --title TITLE --file plan.md` (or `--body`, '-' for stdin), `list --query TEXT`,
`view ID --json`, `edit ID --file updated.md --if-version N`, and `export ID` for Markdown stdout.
`--issue NUMBER` or `--node ALIAS_OR_ID` on create/link attaches the same document;
`unlink ID --issue NUMBER` or `--node NODE` preserves it. `links` reads resource attachments.
`comment ID --body TEXT --quote SELECTED_TEXT` or `--parent COMMENT_ID` adds discussions.
`resolve ID COMMENT_ID` and `--reopen` retain thread history. `archive/restore ID --if-version N`
retain stable references. 

## URL lookup

`hey-boss lookup 'URL'` reads the resource identified by a copied web link

## File attachments

`hey-boss attachment upload PATH --issue NUMBER` stores any regular file up to
10 MiB on the authoritative host. Use `--node SELECTOR` or `--artifact ID` instead
for mindmap nodes and artifacts. `list --issue NUMBER --json` exposes file IDs,
filenames, sizes and SHA-256. `download FILE_ID` materializes a private temporary
copy on the caller's machine; `--output PATH` selects a filename or existing
directory.
