# Codex approvals in Hey Boss

Workers own a Codex app-server process, so they can receive approval callbacks
directly and answer them through the existing desktop/SSH Inbox bridge. This
works on the Mac and on registered companions, including offline queue delivery.
The desktop's paired web/phone Inbox offers the same decisions.

The Inbox question links to the issue and shows the command, directory, reason,
network destination or requested permissions supplied by Codex. File-change
requests include the diff from the corresponding `item/started` event when
available. Approve once and Decline continue the original session. Permission
grants use the protocol's turn scope; no session grants or persistent policy
amendments are offered. Only an exact advertised choice is accepted as a decision.

Cancel, dismissal, an unsupported answer, or an unavailable bridge stops the
attempt on a manual retry hold and retains the saved Codex session. Resolved
requests and stopped workers dismiss their remaining questions. Multiple
callbacks, including callbacks for one item, are keyed by their JSON-RPC request
IDs. Only the worker's owned thread receives decisions, and issue ownership and
stop state are checked again before replying. Status reads are nonblocking so a
slow companion does not hold up cancellation or incoming Codex events.

This integration covers `item/commandExecution/requestApproval`,
`item/fileChange/requestApproval`, `item/permissions/requestApproval`, and
advertised choices in `tool/requestUserInput` / `item/tool/requestUserInput`.
Connector tool approvals such as Accept, Decline and Cancel reach the same Inbox.
Each question gets its own notice; a multi-question callback receives one response
only after every question has an exact advertised answer. Dismissal cancels the
whole callback and its remaining notices. Secret and free-text questions retain
the fail-closed behavior.

URL-mode `mcpServer/elicitation/request` also reaches the phone, including
Okta-style sign-in requests. The pending Inbox card and reader show **Open sign-in
page**, its destination host, and instructions to return afterward. The URL must
be HTTPS without embedded credentials. Opening it never answers the request.
**I've finished signing in** explicitly returns `action: accept, content: null`
to the original callback; Decline and Cancel return their corresponding actions.
Credentials stay on the external sign-in page. Schema/form elicitations remain
unsupported because they can collect sensitive input.

It does not intercept approvals from independently launched terminal
or desktop sessions, which have their own controlling client. Existing blocked
attempts need an explicit retry to use the new bridge; they are not approved or
retried during installation.

Investigation references:

- [Official OpenAI app-server approval protocol](https://developers.openai.com/codex/app-server#approvals)
- [Codex command/file request and decision types](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/item.rs)
- [Codex permission request, response and scopes](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/permissions.rs)

`cargo test --lib worker_approvals` checks supported choices and scopes.
`cargo test --test codex_approvals` drives a synthetic app-server and isolated
Inbox to verify explicit replies, concurrent out-of-order callbacks, approvals
before turn acknowledgment, file previews, cancellation, slow connections and
unowned/resolved requests. The fixture never executes an approved command.
The connector/sign-in cases additionally verify exact actions, URL preservation,
multi-question answers arriving out of order, and cancellation of sibling notices.
The native mobile audit verifies durable sign-in publication, pending state after
opening, and phone completion syncing back to the desktop store.

Visual verification uses the real native Inbox store and a synthetic Codex
worker. `tools/codex_approvals_browser_checks.js` checks desktop, 390 px and
320 px layouts, light/dark themes, readable diffs, issue links, keyboard focus
and reload persistence. The September 20, 2026 session passed 30 browser checks,
including keyboard approval, unchanged winning decisions after late replies,
and completion in the original worker session. Axe reported zero WCAG A/AA
violations in four desktop/mobile theme states. Seven unit tests, six app-server
integration tests and both existing approval-hold regression tests passed.

Issue 74's phone session passed 80 Chrome checks and 92 WebKit checks across
1440, 390 and 320 px, light/dark themes, and reduced motion. Screenshots were
reviewed; sign-in completion uses the full row, long readers scroll, and narrow
navigation fits. Checks covered separate-tab sign-in, unchanged pending state
after opening/reloading, offline refusal, keyboard completion, phone decline,
Activity outcomes, and immutable winning decisions after a late reply. Temporary
browser sessions, screenshots and reports were removed after review. These are
browser and isolated bridge checks; physical iPhone push delivery was not tested.
