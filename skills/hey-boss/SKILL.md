---
name: hey-boss
description: Send project updates and notifications, or ask essential questions through hey-boss.
---

Use `hey-boss`. Commands return plain text.

- `update`: short progress or report cards with full Markdown behind Read update.
- `alert`: brief notifications; `--autoclose` sets seconds on screen.
- `ask`: text or choices. Ask only when requested or an answer is essential; no optional follow-up questions.

Use --project and --title when creating an item. Keep summaries short;
put the details in Markdown. Ask only when requested or an answer is essential.

Questions: --sync waits; --async returns a Task ID. Use wait for the answer.
Save Task IDs and hide cards when they become obsolete.

Examples:
  hey-boss update --project Atlas --title Analysis 'Report ready' '# Findings'
  hey-boss alert --project Atlas --title Build 'Checks passed' --autoclose 10
  hey-boss ask --project Atlas --title Format 'Which format?' '' --option PDF --option Markdown --async
  hey-boss wait '<task_id>'
  hey-boss hide '<task_id>'

Three or more notifications form a collapsible project stack. The project × clears the stack. Read update/Open dismisses the card; history is kept.
Run hey-boss <command> --help for options and more examples.
