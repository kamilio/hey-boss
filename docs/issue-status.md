# Issue progress

Each issue can have a current progress status and a short comment. This is
separate from its lifecycle (Open, Blocked, Closed, Draft) and ordinary comments.
Green means **On track**, orange means **At risk**, and red means **In trouble**.
Issues without an update have no color in the list.

Claim the issue, then publish a single line of plain language:

```sh
hey-boss issue claim 70
hey-boss issue status 70 green --comment 'The fix passes tests. Checking the phone layout next.'
hey-boss issue status 70 orange --comment 'One device is offline. Retrying the installation.'
hey-boss issue status-history 70 --limit 20 --offset 0
```

Only the current owner of an open, non-draft issue can publish updates. The
comment must contain 1–500 characters and no line breaks or control characters.
The command supports the usual project, host, agent, JSON and request-ID options.
Use the same request ID and payload when retrying an uncertain update.
Set the final status before closing the issue or handing it to Boss.

Publish an update after claiming, at meaningful changes, and at least every ten
minutes while actively working. Describe what is happening and what comes next;
skip command logs and technical lists. Keep lasting findings, decisions,
questions and final verification in ordinary comments.

The issue list shows the current color, label and message. The detail page shows
the full message, author and update time. Status history starts collapsed and
loads only when opened, in pages of 20, newest first. The web and paired-device
viewers are read-only; use the CLI to publish. Status refreshes in an open desktop
issue without replacing a comment draft or its selection. An expanded history is
a reading snapshot; new updates cannot shift or repeat its older pages.
**Refresh history** starts a new snapshot with the latest updates.
The paired-device viewer checks for a fresh status every 15 seconds while visible,
using a small response that does not transfer the issue description or history.

Status updates do not change the issue revision or edit time, or create ordinary
comments/activity events. History survives reopening, deletion/restoration,
project moves and fleet synchronization, including offline updates. The latest
update remains visible after ownership changes, marked **Previous owner** when
the assigned owner differs; its timestamp always shows its age.
