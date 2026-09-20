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
`item/fileChange/requestApproval` and `item/permissions/requestApproval`.
Other user-input and MCP elicitation methods retain the existing fail-closed
behavior. It does not intercept approvals from independently launched terminal
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

Visual verification uses the real native Inbox store and a synthetic Codex
worker. `tools/codex_approvals_browser_checks.js` checks desktop, 390 px and
320 px layouts, light/dark themes, readable diffs, issue links, keyboard focus
and reload persistence. The September 20, 2026 session passed 30 browser checks,
including keyboard approval, unchanged winning decisions after late replies,
and completion in the original worker session. Axe reported zero WCAG A/AA
violations in four desktop/mobile theme states. Seven unit tests, six app-server
integration tests and both existing approval-hold regression tests passed.
