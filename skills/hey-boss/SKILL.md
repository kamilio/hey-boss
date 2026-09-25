---
name: hey-boss
description: Background notifications, project issues and documents, worker coordination, and secrets kept out of agent context.
---

# Hey Boss

Use `hey-boss COMMAND --help` for options. Commands default to readable text;
use `--json` when available for structured results. Issue commands default to the
current checkout's project; use `--project FULL_ID` to select another.

## Notifications and questions

Notify once for substantial background results or essential blocking decisions.
Ask yourself: should I page the user for this? Keep active chat, routine progress,
and tests in chat. One outcome sentence; brief details.

```sh
hey-boss notif alert --title Ready 'Ready for review.' --link-url PR_URL --link-label 'Merge PR'
hey-boss notif update --title Review 'Please review.' --file PATH --comments
```

Use `hey-boss notif ask --sync` to wait, or `hey-boss notif ask --async` then `hey-boss notif wait TASK_ID`. Cancellation is never approval; do not automatically re-ask. Remote `pending` does not confirm delivery.

Add `--issue NUMBER` to link the current project's issue; use
`--issue-project FULL_ID --issue-host HOST` for an explicit remote issue.

## Secrets

Use only `hey-boss notif secret`, never chat/ordinary prompts. Never inspect/screenshot the window or read, print, diff, or index the destination. Synthetic test values only.

```sh
hey-boss notif secret --field API_KEY --env-file .env
hey-boss notif secret --field API_KEY -- your-command
```

## Issues

```sh
hey-boss issue list --unassigned --json
hey-boss issue create --title 'Fix reconnect' --body 'Describe the problem' --request-id reconnect-1
hey-boss issue claim 1
hey-boss issue status 1 green --comment 'Checking what causes the reconnect failure.'
hey-boss issue comment 1 --body 'Sleep drops the connection before the retry timer starts.'
hey-boss issue close 1 --comment 'Fixed and verified'
```

While you own an issue, publish a one-line short status after claiming, when the next step
or risk changes, and at least every ten minutes during active work:
`hey-boss issue status NUMBER green --comment 'The fix passes tests. Checking the phone layout next.'`

## More workflows

Read only the reference needed for the task:

- [Issue coordination](references/issues.md): drafts, human handoffs, priority,
  blockers, subtasks, PR links, and guarded remote metadata edits.
- [Documents and attachments](references/documents.md): persistent Markdown
  artifacts, mindmaps, discussions, and file uploads.
- [Workers and agents](references/workers.md): saved workers, explicit start versus
  observation, fleet setup, and agent controls.

Use `hey-boss lookup 'URL'` to read a resource from a copied Hey Boss web link.
