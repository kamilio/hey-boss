# Quick issue actions

Use the **…** button on an issue row, right-click the row, or focus a row control
and press **Shift+F10** to open its actions. Arrow keys, Home, End, and typing an
action's first letter move through the menu. Enter runs the focused action;
Escape closes the menu and returns focus to its button.

The menu offers assignment, Unassign, Close, Block, draft readiness, and the
applicable Unblock, Reopen, or Restore action. Release hold clears a manual pause
while keeping unfinished dependencies. Ready handoffs can be reopened or
unassigned. Protected attempts offer only View and Copy; disabled project drafts
hide Move to draft. View issue and Copy issue link are always available.
Changing another agent's claim retains the usual confirmation.

Actions keep the current filters. If the changed issue leaves the list, focus
returns to search. Failed changes show an error and leave the row available for
retry. The same interface is shipped to desktop and paired mobile clients.

Run `node tools/test_issue_list_actions.cjs` for state and mutation-scope checks.
The browser suite, `tools/issues_list_actions_browser_checks.js`, runs with
`playwright-cli run-code --filename` against an isolated issue web server. It
creates synthetic issues and must never be pointed at the live issue database.

Build with `cargo build --locked --bin hey-boss` and `npm --prefix mobile run build`,
then start `node tools/serve_issue_list_actions_fixture.mjs`. The fixture exposes
the native UI at `http://127.0.0.1:59639` and the paired UI at
`http://127.0.0.1:52039/issues`. For the paired UI, retrieve the synthetic code from
`/fixture-pairing` and POST it to `/api/pair` using the browser's request context.
Run the suite on each surface; each must finish normally with `COMPLETE` and
**68/68** checks against the compiled assets. Review the light/dark desktop and phone screenshots in
`output/playwright/issue139`, including the short phone viewport and ownership
confirmation. Close the browser session, stop the fixture with Ctrl+C, and remove
the generated screenshots and CLI reports. The fixture shuts down its own database
and web processes and removes its temporary data.
