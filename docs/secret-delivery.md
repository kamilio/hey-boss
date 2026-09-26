# Private credential delivery

The CLI reports a non-secret `delivery` status on stderr. Stdout remains reserved
for the explicitly selected private-file sink; child stdout and stderr stay suppressed.

| Status | Meaning |
| --- | --- |
| `delivered` | The file was published, redirected output was synced, or the child started with the credentials. A failed child does not undo delivery. |
| `not_delivered` | This invocation did not write credentials or start the child. |
| `unknown` | Redirected output may be partially written. Do not repeat automatically. |

Failure messages include a fixed `reason`, never the received payload, field values,
destination path, child arguments, or upstream error text. Success exits 0; failures
exit 1, including child failure after delivery. Inspect both status and exit code.

`cancelled` means the user declined; do not re-request automatically. `expired`
means the native prompt reached its deadline. `rejected` means invalid request
metadata; `busy` means the desktop already has four private prompts open.
Legacy companion errors appear as `unavailable` without claiming cancellation.
`response_empty`, `response_missing`, and `response_malformed` distinguish a
connection closed without a reply, an absent result, and invalid response data.
Transport failures and response-size limits also leave the destination untouched.
Before an explicitly authorized retry after a transport/protocol failure, dismiss
any remaining private prompt and repair/reconnect the companion. Requests are
ephemeral: they are never queued, replayed, or written to notification history.

## Investigation of issue 469

The historical `Invalid secret response` was emitted at either JSON decoding step
in `src/secret_cli.rs::read_response`. Both steps precede destination writing and
child launch. Therefore the invocation reported in poe2 #442/#449 did **not**
deliver credentials to its destination. This establishes no facts about existing
destination contents, provider access, or authenticated Convex preview admission.

Synthetic socket tests reproduce that exact old error with both an empty EOF and
malformed outer/inner JSON. The historical message contains insufficient evidence
to choose between those causes. A well-formed cancellation already had a separate
error; a malformed cancellation could also be hidden by eager result decoding.
No live request was replayed and no private destination or value was inspected.

Coverage exercises success and failure through the CLI with synthetic values,
redaction, cancellation/expiry/rejection, missing/malformed replies, child launch
and child exit, and native prompt lifecycle/clearing. Native visual fixtures use
synthetic data only; never capture a real credential window.

The issue 469 session inspected ten AppKit render variants: single field and
login/password pair, empty/masked/revealed, compact, dark, and invalid input.
These renders cover text, wrapping, and geometry; macOS compositor-hosted controls
are omitted by the print renderer. Full interactive visual verification remains
pending: targeted screen capture was unavailable and the computer-use service
failed. Keep the issue open until that final check can run with synthetic data.
