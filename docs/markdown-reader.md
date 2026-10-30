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

In a saved Markdown artifact, choose **Read → Play review**. Reading starts at
the selection and highlights each spoken word. Speak over it to leave a comment;
after a short pause, the reader transcribes on this Mac, saves the quote and
source line to the artifact's discussion, and continues. **Comments** shows
the same discussion as the web. **Stop**, switching to Edit, or leaving the
window closes the microphone. An interrupted save offers **Retry comment**
with the same request ID. Local files and HTML previews do not offer voice review.

Narration plays through the microphone's echo-cancelling audio graph. WebRTC
analyzes 20 ms frames: bookmark the first voiced frame, pause after 160 ms,
keep 400 ms of pre-roll, and finish after 800 ms of quiet plus WebRTC hangover.
For comparison, [LiveKit](https://github.com/livekit/agents/blob/main/livekit-plugins/livekit-plugins-silero/livekit/plugins/silero/vad.py)
uses 50/550 ms and [Pipecat](https://github.com/pipecat-ai/pipecat/blob/main/src/pipecat/audio/vad/vad_analyzer.py)
uses 200/200 ms. The longer pause here lets a reviewer think mid-sentence.

Run `tools/spoken_review_checks.sh /path/to/hey-boss /path/to/hey-boss-test`
to test with synthetic microphone audio, muted playback and a disposable project.
`HEY_BOSS_AUDIT_REVIEW_ECHO=1` on a signed audit app additionally checks real
speaker bleed in a quiet room, only with previously granted microphone access.
