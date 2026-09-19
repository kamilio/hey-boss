# Inbox

The web app at `http://127.0.0.1:4781/#view=inbox` shares navigation with Issues.
The menu-bar Inbox and `hey-boss inbox` open this page and start the web service
when needed. The native Inbox window has been removed; native notification
banners, questions, document previews, and agent overview remain available.

Unread contains pending updates, alerts, questions, and document reviews. Activity
contains completed and cancelled records. Both views show all matching notices,
newest first, with project/search filters. Appearance follows the system.
The list loads bounded summaries from the existing native SQLite history;
document bodies and attachments load only when opened. There is no second history
store and no migration of existing notices.

Ordinary updates/alerts become read when opened successfully. Questions stay
pending until answered or cancelled. Reviews stay pending until finished or
cancelled; feedback alone does not finish a review. Answers use the existing
native/mobile winner mechanism. Late answers cannot overwrite the winning result.
Answer and review drafts persist locally, and Cmd/Ctrl+Enter submits them.

## Issue links

```sh
hey-boss alert --project poe2 --title Ready 'Ready for review.' --issue 123
hey-boss update --project poe2 --title Review --file report.md --comments --issue 123
hey-boss inbox --json
```

`--issue NUMBER` defaults to the current repository/directory issue project.
`--issue-project FULL_ID` overrides it; `--issue-host HOST` targets an SSH issue
store, also inherited from `HEY_BOSS_ISSUE_HOST` when set. These flags apply to
notice creation, independently of the notice's display project.

The web notice sidebar can link, change, or unlink an existing issue, including a
closed issue. Notice list chips and detail links open the issue directly; the issue
sidebar shows related notices and links back to Inbox. Each notice has at most one
issue relationship; many notices may relate to one issue. Remote links retain the
host so project/issue numbers on different machines are not confused.

Linking changes only the relationship. It never changes notice completion,
answers, issue state, claims, content, queue order, or revisions. Creation does not
require an issue lookup, so offline notice delivery retains its existing durable
queue behavior. Web linking validates the selected issue before saving.

## Verification

Isolated native history and issue stores exercise creation links, summary loading,
read receipts, questions, winner preservation, cancellation, review comments and
completion, archived links, and unchanged issue revisions/content/assignment.
`tools/inbox_fixture.swift` serves the real native Store through an isolated socket
without notification banners or a LaunchAgent. Browser checks run through the
Playwright CLI in Chromium and WebKit:

- `tools/inbox_browser_checks.js`: 32 checks per browser, including Markdown safety,
  two-way links, reload persistence, drafts, keyboard submission, filters, shared
  navigation, automatic appearance, mobile overflow and dialog geometry.
- `tools/inbox_pending_checks.js`: 6 checks per browser for cancellation, retained
  drafts, pointer delivery to list chips, and issue navigation.
- `tools/inbox_accessibility.js`: 11 light/dark/mobile/detail/dialog states per
  browser, zero axe violations for WCAG 2 A/AA, WCAG 2.1 AA and best practices.
- `tools/inbox_remote_checks.js`: 4 checks against an isolated devbox issue store
  through real SSH, including both directions and unchanged remote issue content.

Rust HTTP checks cover same-origin/CSRF protection, typed action restrictions,
invalid/nonexistent link rejection, HTML sanitization, canonical project links,
and continued issue service when native Inbox is unavailable. Durable queue tests
verify optional issue metadata survives disk round trips and legacy records decode
without it. The native audit verifies replies match the CLI contract.
