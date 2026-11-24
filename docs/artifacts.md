# Project artifacts

Open **Artifacts** in the web menu to create, search and read persistent Markdown
documents in the selected project. Issue sidebars offer **Create artifact** and **Attach existing**. Both reuse the resource's
project. One document can be attached to multiple issues; its
stable URL and references survive renames, edits and archiving. Unlinking removes
only the reference.

The library shows document cards with short, readable text previews in a
responsive two-column layout (one column on phones). Previews omit Markdown
formatting, front matter, code blocks, and headings that repeat the title.
Only the first 4,096 characters are inspected; excerpts contain at most 240
characters plus an ellipsis and display up to three lines. Full documents load
when opened, keeping the library fast even with large files.
Open a document for a
focused reading view; **Edit** opens its draft, and the **More actions** menu
contains Export Markdown, Archive/Restore, and Delete permanently. Library cards
also offer quick Archive/Restore and Delete actions, including on touch screens.
Select **Archived** to browse archived documents and restore them.
Permanent deletion asks for confirmation and removes the document, its comments,
attached files, and issue references. Referring issues stay
intact. Deletion cannot be undone; Archive keeps a document for later.
**Comments** shows or hides the
conversation beside the document, or below it on smaller screens. Reply composers
open only when needed. Unused reference and discussion sections stay out of view.
Attachment pickers search available documents and disable Attach when nothing
matches. Cancel or Escape closes the picker without changing any references.

Select text and choose **Comment on selection** to quote a passage. Selecting or
copying text keeps the reading layout in place. Comments stay closed until opened;
the selection action brings the composer into view on a phone. You can also
comment on the whole document. Threads support replies, resolving and reopening.
Resolved threads collapse subtly. Edits preserve every discussion; unmatched
selection quotes appear as outdated. Quotes retain nearby text to distinguish
repeated passages.
Reply drafts survive reloads and resolving a discussion. Interrupted replies keep
their original content and request identity until delivery can be confirmed.

Saved documents open in **Read**. Choose **Read Aloud** on a shared Markdown document to start narration and turn on the microphone. Reading continues when you switch apps; **Stop** turns the microphone off. The microphone indicator changes from **Listening** to **Recording comment** when you interrupt. To type instead, select text and choose **Comment on selection**, or open **Comments** (⌘⇧M) for a document note. ⌘Return posts to the same discussion you see on the web.

The editor switches between **Preview** and **Write** without losing draft text.
Its controls remain reachable while scrolling. Ordinary drafts grow with their
text; large files use a bounded writing area with a plain editor that renders
only visible lines. Undo/redo, selections and browser drafts retain the complete
document. Its local bundle loads only when a large draft is opened.
Opening Comments on a small screen
brings the conversation into view, and closing it returns to document controls.
Large documents become readable while the rest loads. Posting comments and
resolving discussions keep the unchanged reading surface in place.
The Markdown editor saves drafts in this browser and reports draft, saving,
saved and error states. Save publishes the draft. Revision checks reject stale
edits. Load the latest revision for comparison, merge it into your draft, then
explicitly select that revision before saving. Interrupted saves retain the
original request ID and content so retrying does not create another document.
**⌘S / Ctrl+S** saves from the editor. Back from a new attached document returns
to its referring issue or map topic. Failed reads offer **Try again**.
Import accepts UTF-8 Markdown and HTML (`.html` or `.htm`), including through
`--file`. Export preserves the saved source; comments and relationships remain
in the project store, rather than the exported file.

HTML works in descriptions and documents on desktop, phones and the native
artifact reader. Ordinary tags such as `<details>`, `<summary>` and `<table>`
render alongside Markdown. For an interactive block, use a fenced block marked
`html render`; a complete document starting with `<!doctype html>` or `<html>`
renders directly. Ordinary `html` fences remain code examples.
Previews grow to their full content height and scroll with the page. **HTML
source** reveals the original text; editing and exporting preserve it exactly.
Use self-contained HTML with inline CSS/JavaScript and embedded images. Scripts
run in an isolated frame without access to the application or its storage;
external scripts, styles and resource requests are blocked. Relative HTML assets
are not uploaded automatically. Theme colors are available as CSS variables such
as `--background`, `--foreground`, `--card`, `--border` and `--accent`.

```sh
hey-boss artifact create --title 'Migration plan' --file plan.md --issue 12
hey-boss artifact create --title 'Interactive page' --file page.html
hey-boss artifact list --query migration --json
hey-boss artifact view a-ID --json
hey-boss artifact edit a-ID --file updated.md
hey-boss artifact link a-ID --issue 15
hey-boss artifact link a-ID --issue 12
hey-boss artifact links --issue 12
hey-boss artifact comment a-ID --body 'Check rollback' --quote 'Rollback'
hey-boss artifact comment a-ID --body 'Covered' --parent 1
hey-boss artifact resolve a-ID 1
hey-boss artifact resolve a-ID 1 --reopen
hey-boss artifact export a-ID > plan.md
hey-boss artifact archive a-ID
hey-boss artifact list --archived
hey-boss artifact restore a-ID
hey-boss artifact unlink a-ID --issue 12
hey-boss artifact delete a-ID
```

`--project`, `--host`, `--agent`, `--request-id` and `--json` follow issue CLI
conventions. Mutation retries must retain the identical request ID and payload.
The CLI captures the current revision for edits, archive/restore, and deletion.
The CLI's `delete` command is explicit and does not prompt; check the ID before running it. Interrupted web actions retain their request identity
for retry. List pages contain
50 documents, ordered by recent activity; use `--offset` for additional pages.
Documents have a 1 MiB Markdown limit, comments a 1 MiB limit, and selection
anchors an 8 KiB limit. Each document supports 1,000 comments within an aggregate
8 MiB source budget. The response budget also limits rendered document size.

Artifacts use the same private SQLite project store, local web CSRF protection
and SSH transport as issues. They are authored on the authoritative
machine rather than replicated into fleet issue queues. Use `--host SUPERVISOR`
from a companion to work with shared documents. Phones use the same local web interface on explicitly allowed Wi-Fi networks; see [local mobile access](mobile.md).

## Publish to Notion

**Share** opens configured destinations. **Sync now** uploads saved Markdown;
manual publications keep their link and show **Changes pending** after edits.
Enable **Automatically sync saved edits** for a publication to upload on save.
The native editor’s Share button opens these same controls in the web reader.
Comments stay in Hey Boss. Deleting an artifact leaves its remote document intact.
Private attachment links must be replaced with externally accessible URLs first.

Configure Markdown destinations alongside Pages in `~/.hey-boss/publishing.yaml`:

```yaml
notion:
  content: artifact
  auto_sync: false
  command: node /absolute/path/to/hey-boss/tools/notion-publisher.mjs
  audience: private
  create: [create, '{source}', '{name}']
  read: [read, '', '', '{id}']
  update: [update, '{source}', '{name}', '{id}', '{version}']
  fields: {id: /id, url: /url, version: /version}
```

The adapter uses the installed `wire` CLI and its Notion login. It uploads with
`wire sync`, retains a stable URL, and rejects updates if the remote Markdown
changed. Review remote edits before using **Existing link** to replace them.
A failed first upload is never created again automatically; recover its Notion
link and enter it there. No Notion credentials are stored in publisher config.
Configure this on the artifact authority; clients and phones route there.

```sh
hey-boss artifact providers
hey-boss artifact share ID --provider notion
hey-boss artifact sync ID
hey-boss artifact share ID --provider notion --auto-sync true
```

`content` defaults to `page`. New page publications default to automatic sync;
new Markdown publications default to manual. `auto_sync` in config overrides
that default, while a saved publication keeps its own setting. Provider commands
use the [same contract as Pages](publishing.md#external-publishing-providers),
with `{source}` pointing to a Markdown file and `{name}` to the artifact title.
Links, settings, and upload receipts replicate; commands and login remain local.
