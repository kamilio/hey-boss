# Native Markdown reader

Document previews use a single AppKit `NSTextView`. The node model and syntax
lexer adapt poe-code's terminal Markdown renderer; pulldown-cmark parses
CommonMark/GFM in Rust. Attribution is retained in
`tests/fixtures/poe-markdown-LICENSE.txt`.

The hidden `render-markdown INPUT OUTPUT --native` command emits a structured
document with source line ranges and code tokens. Swift builds attributed text
off the main thread. A native loading indicator remains visible until the whole
document is installed once, so a later rendering pass cannot clear selection.
The reader caches parsed documents within 24 MiB, permits two background renders,
and uses noncontiguous text layout. Helper input is limited to 2 MiB, output to
32 MiB, and execution to 20 seconds with bounded process cleanup. Failures retain
the complete source as selectable native text. Excessive nesting also retains
the source instead of recursively rendering it.

Headings, nested/task lists, tables, quotes, GitHub alerts, inline formatting,
highlighted code, links, footnotes and images use native text attributes.
Frontmatter is hidden; HTML is literal text. Links open through the application's
URL handler. Code wraps visually without inserting copied line breaks.

Cmd-C and the native Copy menu export the selection as plain text and RTF,
plus RTFD when it contains attachments. Table cells use tab-separated values in
plain text. Selection crosses all blocks and retains Unicode, code indentation
and source newlines. Review comments map selection back to original source lines;
loading remote images does not replace text storage or selection.

`update --file` continues to snapshot a regular UTF-8 file up to 1 MiB before
posting. Queue/replay transports its contents rather than its local path.

Focused Rust tests cover native structures, code/source fidelity, links and deep
nesting. `HEY_BOSS_AUDIT_NATIVE_READER=1` runs the native reader, selection/copy,
large-document, image and review-comment audits against `HEY_BOSS_CLI_PATH`.
`HEY_BOSS_AUDIT_SNAPSHOT_DIR` optionally saves synthetic light/dark/loading
snapshots without exposing production notifications.

## Complete upstream regression suite

The [full poe-code Markdown suite](../tools/poe-markdown-tests/README.md) is
vendored with all 346 original tests and its supporting fixtures. Its complete
input corpus also runs through Rust's native document builder and AppKit's text
renderer/copy path. Exact, documented dialect expectations distinguish CommonMark
behavior from poe-code's parser without dropping cases. These regressions caught
BOM handling, carriage-return frontmatter, empty frontmatter, and non-leading
fences incorrectly hidden as metadata; the native builder now handles them.
