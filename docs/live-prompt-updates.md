# Editing task instructions

Open **Prompts** to edit **worker.md** (implementation), **plan.md** (planning), or a workflow or context file. Project settings links each workflow switch to its files.
Defaults live in `src/issues/prompts/*.md`.
**Restore default** removes an override. Sections without applicable context are
omitted. Preview and worker launches use the same order: task, workspace, then delivery. A newly unblocked task gets the ordinary worker prompt.
The workspace switch selects workspace-worktree.md or workspace-checkout.md; the delivery switch selects delivery-pr.md or delivery-main.md.
Retries use the normal task prompt and reuse unfinished work when available.
A leading /goal starts a native Codex goal. Workers do not send goal continuation prompts or manage the goal’s final status.

Variables expand once: task text containing `{{number}}` is kept literal when
inserted into another section. GitHub evidence is retrieved with `hey-gh`; it
is not copied into previews, launches, or live update messages.

Saved edits apply when a task starts. To message running task agents, open **Workers → Broadcast**. GitHub watcher changes wait for a fresh thread to claim the issue.
