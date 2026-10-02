---
name: hey-boss
description: Background notifications, project issues and documents, worker coordination, and secrets kept out of agent context.
---

# Hey Boss

Use `hey-boss COMMAND --help` for options. Commands default to readable text;
use `--json` when available for structured results. Issue commands default to the
current checkout's project; use `--project FULL_ID` to select another.

Change prompts only when the user explicitly requests a prompt change; task or queue instructions do not authorize changing prompts.

## Notifications and questions

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

Connected companions already have an authenticated supervisor tunnel. Run
`hey-boss fleet capabilities` to inspect supported operations and upgrade guidance;
`hey-boss fleet status` shows connectivity. No new SSH hostname or worker is needed.
Use `issue edit NUMBER --draft` for an eligible unassigned,
unreserved issue; companions route it automatically and derive a stable retry ID.
Other metadata edits use `--supervisor`.
Never claim work merely to edit metadata. See [issue coordination](references/issues.md).

```sh
hey-boss issue list --unassigned --json
hey-boss issue create --title 'Fix reconnect' --body 'Describe the problem' --request-id reconnect-1
hey-boss issue claim 1
hey-boss issue comment 1 --body 'Sleep drops the connection before the retry timer starts.'
hey-boss issue close 1 --comment 'Fixed and verified'
```

## More workflows

Read only the reference needed for the task:

- [Issue coordination](references/issues.md): drafts, human handoffs, priority,
  blockers, subtasks, PR links, and guarded remote metadata edits.
- [Documents and attachments](references/documents.md): persistent Markdown
  artifacts, mindmaps, discussions, and file uploads.
- [Workers and agents](references/workers.md): saved workers, explicit start versus
  observation, fleet setup, and agent controls.

Use `hey-boss lookup 'URL'` to read a resource from a copied Hey Boss web link.
