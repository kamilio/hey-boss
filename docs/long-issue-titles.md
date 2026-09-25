# Long issue titles

Issue creation and title edits accept text longer than 512 UTF-8 bytes. The stored
title remains within 512 bytes; the remaining text becomes the start of the
description, followed by a blank line and any supplied description. A title-only
edit preserves the existing description. Short titles are unchanged.

The Rust store applies this rule for the CLI, native Quick Add, web editor, paired
phone UI, drafts, and subtasks. A supplied first-line heading takes precedence;
otherwise it splits at the last whitespace within the title limit, falling back
to a UTF-8 boundary for unbroken text. Boundary whitespace is removed. Text is
never summarized or discarded. The resulting description must still fit its
existing 1 MiB limit; a failed edit leaves the issue unchanged.

Native and web Quick Add show a neutral note when extra text will move to the
description. Web inputs accept the entire paste. Splitting happens when saving,
so typing, canceling, and retrying preserve the original input. Request receipts
retain the original operation, preventing retry conflicts or duplicated overflow.

Verification:

- `cargo test --test issues overlong_issue_titles`
- `cargo test --lib title_content`
- `node tools/test_quick_issue.js`
- Native Quick Add audit described in [native-quick-issue.md](native-quick-issue.md),
  including long-title request contents, retries, and light/dark window captures.
- `node tools/serve_long_title_fixture.mjs [path-to-built-hey-boss]` starts an
  isolated native server at port 59653 and paired relay at port 52053. Pair using
  the synthetic code from `/fixture-pairing`, then run
  `tools/long_title_browser_checks.js` with Playwright CLI on each surface. It
  checks 320–1440 px layouts in light/dark mode, full creation and reload,
  Unicode byte boundaries, complete descriptions, and lost-response retries.
  Stop the fixture with SIGINT to remove its processes and temporary store.

Visual outputs are disposable under `output/playwright/issue253/`; remove them
and close the dedicated browser session when verification is complete.

Issue 253 verification on 2026-09-25 passed both Rust integration tests, both
splitter unit tests (including 2,460 Unicode boundary cases), the native Quick Add
behavior audit, and 36 browser assertions on each of the desktop and paired
surfaces. Visual review covered four viewport widths in both themes, full-editor
and saved-detail states, and light/dark exports of the actual AppKit controls.
Desktop window capture was unavailable, so the native layout review used
offscreen AppKit rendering; it did not assess desktop glass compositing.
