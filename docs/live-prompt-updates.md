# Editing task instructions

Open **Prompts** to edit **worker.md** (implementation), **plan.md** (planning), or a workflow or context file. Project settings links each workflow switch to its files.
Defaults live in `src/issues/prompts/*.md`.
**Restore default** removes an override. Sections without applicable context are
omitted. Preview and worker launches use the same layout. Existing saved layouts
are preserved when editing settings; the sequence has no editor.

Variables expand once: task text containing `{{number}}` is kept literal when
inserted into another section. GitHub evidence is retrieved with `hey-gh`; it
is not copied into previews, launches, or live update messages.

Saved edits apply when a task starts. To message running task agents, open **Workers → Broadcast**. GitHub watcher changes wait for a fresh thread to claim the issue.
