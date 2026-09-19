# Project artifacts

Open **Artifacts** in the web menu to create, search and read persistent Markdown
documents in the selected project. Issue sidebars and selected mindmap topics
offer **Create artifact** and **Attach existing**. Both reuse the resource's
project. One document can be attached to multiple issues and map nodes; its
stable URL and references survive renames, edits and archiving. Unlinking removes
only the reference. Removed map topics remain identified in document backlinks.

Select text in the reading surface before commenting to quote that selection, or
comment on the whole document. Threads support replies, resolving and reopening.
Resolved threads collapse subtly. Edits preserve every discussion; unmatched
selection quotes appear as outdated. Quotes retain nearby text to distinguish
repeated passages.

The Markdown editor saves drafts in this browser and reports draft, saving,
saved and error states. Save publishes the draft. Revision checks reject stale
edits. Load the latest revision for comparison, merge it into your draft, then
explicitly select that revision before saving. Interrupted saves retain the
original request ID and content so retrying does not create another document.
Import and Export use UTF-8 Markdown; comments and relationships remain in the
project store, rather than the exported file.

```sh
hey-boss artifact create --title 'Migration plan' --file plan.md --issue 12
hey-boss artifact list --query migration --json
hey-boss artifact view a-ID --json
hey-boss artifact edit a-ID --file updated.md --if-version 1
hey-boss artifact link a-ID --issue 15
hey-boss artifact link a-ID --node release
hey-boss artifact links --node release
hey-boss artifact comment a-ID --body 'Check rollback' --quote 'Rollback'
hey-boss artifact comment a-ID --body 'Covered' --parent 1
hey-boss artifact resolve a-ID 1
hey-boss artifact resolve a-ID 1 --reopen
hey-boss artifact export a-ID > plan.md
hey-boss artifact archive a-ID --if-version 2
hey-boss artifact list --archived
hey-boss artifact restore a-ID --if-version 3
hey-boss artifact unlink a-ID --issue 12
```

`--project`, `--host`, `--agent`, `--request-id` and `--json` follow issue CLI
conventions. Mutation retries must retain the identical request ID and payload.
Edits and archive/restore require a current `--if-version`. List pages contain
50 documents, ordered by recent activity; use `--offset` for additional pages.
Documents have a 1 MiB Markdown limit, comments a 1 MiB limit, and selection
anchors an 8 KiB limit. Each document supports 1,000 comments within an aggregate
8 MiB source budget. The response budget also limits rendered document size.

Artifacts use the same private SQLite project store, local web CSRF protection
and SSH transport as issues. Like mindmaps, they are authored on the authoritative
machine rather than replicated into fleet issue queues. Use `--host SUPERVISOR`
from a companion to work with shared documents. The paired Fly application
forwards artifact and referring-resource reads and document mutations through
the existing supervisor phone bridge. Its durable queue stores requests and
delivery results, never an independent authoritative document collection.
Offline saves remain pending until the supervisor reconnects. Pairing, project
registry validation, origin checks and device isolation protect this transport.
