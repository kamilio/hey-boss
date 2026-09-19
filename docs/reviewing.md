# Document review

`hey-boss update --project Atlas --title 'Review ready' 'The proposal needs your review.' --comments --file report.md --json`

Files are read immediately and included in the durable request. Markdown remains Markdown; source/text is fenced and syntax highlighted; PNG/JPEG/GIF/WebP is encoded as an image snapshot. Server paths never need to exist on the Mac. Relative linked resources are not bundled.

`status ID` returns available comments automatically. `status ID --sync` or `wait ID` waits for the reader to close or cancellation. JSON includes `comments`, `review_status`, and `document_name` when applicable. Rust clients can use `with_comments(true)` or `try_review_file` and receive typed response fields.

The native reader offers a collapsible summary-only sidebar and a selection-driven Add comment action. Writing happens in an opaque floating editor beside the selection, rather than over blurred document text. The Send button or Cmd+Enter saves, clears the input, and closes the editor. Typing auto-saves after a short pause; continued edits update the same comment. Saved passages are highlighted and clickable. Comments are available before sending; there is no Finish review step. Closing saves pending edits before closing the review. Image notes apply to the document. Comments are persisted before acknowledgment; completed and cancelled reviews preserve existing comments. Ordinary updates omit review fields.

Text input is limited to 1 MiB, images to 4 MiB, comments to 200 with a 16 KiB per-comment bound. Native image decoding also enforces dimension, frame, and aggregate pixel bounds. Unsupported highlighting remains escaped plain text. Raw HTML is literal, links are filtered, and document scripts are disabled.

## Verification

- Rust full suite: `out/review-final-tests.log`.
- Server transport: `out/review-transport-final.log`; Markdown/source/image replay after source deletion and broker restart, comment retrieval and offline cache durability.
- Native reader: `out/review-selection-final-audit.log`; real WebKit tables/callouts/Unicode/highlighting, resizing, selection action, collapsible panel, automatic comment persistence, immediate open-review status, edits without duplicates, anchored highlighting, close saves remaining edits, image rendering and malformed-image rejection.
- Poe-code Markdown fixtures and MIT attribution remain under `tests/fixtures`.

Screenshot capture has previously failed in CUA with `failedToCreateImageDestination`. Automated geometry/DOM checks do not establish visual polish; screenshot review remains outstanding unless a subsequent capture succeeds.

## Installed verification

Mac CLI/native binaries match the verified builds. Devbox installation and live Markdown offline replay passed (`out/review-live-server-check.log`). The server replay preserved Markdown, filename, comment opt-in, and server source label after the source was deleted and its broker restarted. Canonical and Mac global skill hashes match.

The final synthetic screenshot preview launch was rejected by automatic approval review as locally generated software from an unrecognized source requiring confirmation. No capture workaround was used; visual review awaits permission.

Agent retrieval was additionally verified through the actual CLI: default `status`, `--async`, and `--sync` all print comments and selected quotes automatically from the durable terminal cache. The public Rust `Client::status` returns the same typed comments. Evidence: `out/review-agent-retrieval-test.log`; Clippy passes with warnings denied.

Multiline selections carry exact original source lines and line numbers under `comments[].selection`; human status output prints the same Lines and Source automatically. Markdown syntax, indentation, and newlines are preserved. The source file display fence is excluded from agent line numbers.

Reference inspected read-only: devbox `~/poe-tooling/apps/git-shelf` and `packages/review`, particularly line-range comment locations and summary navigation. No source changes were made there.

Latest evidence: `out/review-floating-native-final.log` verifies opaque floating editor, summary-only sidebar, Cmd+Enter, exact multiline source lines, missing final-newline handling, and failed-save recovery. `out/review-floating-live-server.log` verifies exact selected source lines in immediate open-review status and durable offline cache after server restart. The installed Mac and devbox skills are synced. Visual capture of the already-running baseline timed out in CUA; the prepared current synthetic screenshot preview still requires launch approval.
