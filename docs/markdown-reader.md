# Markdown reader verification

The reader goal is a controlled, reliable Markdown presentation, file input on both local and server CLI, real mouse routing checks, and extensive examples adapted from the legitimate poe-code renderer tests.

## Rendering

CommonMark/GFM parsing is provided by pulldown-cmark 0.13.4. The app controls its document renderer and stylesheet: readable native system typography, light/dark colors, heading hierarchy, lists/tasks, table alignment, footnotes, code overflow, and distinct GitHub callouts. This is a proven parser with an app-owned renderer, not a from-scratch CommonMark parser.

Raw HTML is literal text. Unsafe URL destinations are removed while labels remain. WebKit uses a nonpersistent data store, disables document JavaScript, and restricts navigation to internal anchors or explicitly opened external links. Rendering runs off the main thread through the installed CLI, with a 20-second timeout plus bounded process cleanup, 1 MiB source and 8 MiB output limits. Existing native attributed Markdown is retained as a recoverable fallback. The WebKit content process gets one reload if it terminates.

## File and server behavior

`hey-boss update --project Atlas --title 'Migration ready' 'Ready for review.' --file report.md`

The file is read before posting, not opened later by the Mac. It must be a regular UTF-8 file up to 1 MiB; missing, directory, invalid UTF-8, and oversized files return errors. The resulting Markdown text uses the existing durable transport and history format. No relative server asset files are transferred.

A local integration test invoked the server CLI against an isolated broker, queued a file while offline, deleted the file, restarted the broker, then verified byte-exact replay, summary, source stamp, and rendering of a table/callout/code block. The same check ran on real devbox using its installed Linux CLI and an isolated temporary broker/bridge; it passed and cleaned up its own files/processes. No QA notifications were sent to the production Mac.

## Examples and test evidence

`tests/markdown_poe_examples.rs` adapts 21 test groups from poe-code's terminal-markdown HTML/parser tests. Original license is preserved in tests/fixtures/poe-markdown-LICENSE.txt. Coverage includes titled links and images, inline styles, escapes/entities, hard breaks, nested lists/quotes, task lists, aligned tables and block interruption, malformed tables/Markdown, all five alert types, frontmatter, Unicode, references/footnotes, setext headings, thematic breaks, fenced/unknown-language code, empty documents, raw HTML, unsafe URLs, and large documents/long code lines.

First run passed 18 and failed frontmatter, alerts, and unsafe destinations. The implementation was corrected and all 21 passed; first/fixed logs are retained under ignored out/. The complete Rust suite passed 78 tests: 22 library, 23 binary, 2 autoconnect, 10 companion, and 21 adapted examples. Clippy warnings denied and release build passed.

The optimized native audit passed actual view hit testing for the Read update button through its card and complete notification container, and its action opened the preview and persisted dismissal. The user's reported physical click failure has not been reproduced in these synthetic tests; direct action tests alone were insufficient. The button was changed to use a regular native control and its intrinsic height rather than forcing a larger tracking cell into a 28-point frame. Physical click and current-view screenshot verification remain pending.

The native reader audit exercised the installed Rust renderer and actual WebKit DOM: heading/Unicode text, table cell count, warning callout, no active script nodes, and long code staying within the page. Compact/wide appearance checks are tracked separately below.

## Visual blocker

CUA image capture repeatedly fails with failedToCreateImageDestination, including a separate synthetic reader app with no production task data. The private accessibility fallback was independently rejected by automatic approval review. No bypass was attempted. Consequently no new reader screenshot or visual alignment claim is supported. The synthetic reader app and source are preserved under out/ for review when capture is available.

Canonical, Mac global, and devbox global skills were synced and all matched SHA-256 26fab24098565344ea8f4b5b9263d6393f4b6faec11a3e039d04cb3fa3322fa5. The real server companion is active/running with NRestarts 0 after installing the new CLI. Old pending authentication/distribution limitations were resolved for this change.

## Final layout and installation checks

The updated optimized native audit passed with exit 0 at 420-point light and 1000-point dark window sizes. It verified actual WebKit viewport widths matched the window, tables retained their cells, page overflow stayed bounded, and long code scrolled inside its block. These are DOM/layout checks, not screenshots or proof of all visual alignments.

Cargo package verification compiled the packaged sources successfully (32 files); Swift release script typecheck, canonical skill validator, format/diff checks passed. Installed Mac CLI hash matches b618fbd50509d64c9e5e49773a858448156392b05cb787691a37f82080368a26; native daemon matches e672ac996c4ddef7e114385ae1c4db8f28db679927ef88fd422e95777741b3cc. Installed overview command succeeded, native PID 20596 remained running, and connector reported connected with zero failures and no retry. The real server CLI used in the isolated test had Linux binary hash 5213842e9b86b9f0a88dabc70ce9dda5f7d7083bec040100d18ed1707d267dc8. Its service remained active/running with NRestarts 0.

The goal tracker rejected creation of a separate reader goal because an earlier unfinished goal owns this thread. This document records the requested reader goal's evidence. Screenshot/current physical-click checks remain unproven; no overall goal-completion claim is made.
