# Guarded Ready handoffs

Ready means the caller considers the attached PR usable by dependent work. It
assigns the issue to Boss and unblocks eligible dependents. It does not assert
passing CI, completion, merge, or deployment, and performs no GitHub or Git action.

Read `hey-boss issue view NUMBER --json` immediately before handoff. Its
`ready_guard` contains `if_version`, `expected_assignee`, and
`expected_reservation`. Pass that snapshot to `issue ready NUMBER` using
`--if-version`, `--expected-assignee` (use `unassigned` for null), and
`--expected-reservation`, plus a stable `--request-id`. All three guards are
required together. The reservation token covers allocation and unfinished worker
attempt identities, including claim timestamps and reservation expiry.

The authoritative transaction checks every guard before changing the issue,
recording the handoff, and reconciling dependencies. Retry an uncertain result
with the identical operation and request ID. A successful retry returns its saved
response, even if the issue has since changed, without repeating the handoff.
Reusing that ID for a different operation fails.

For a reconciled manual hold, add `--clear-manual-hold` to the guarded Ready
command. The hold clears only if the entire handoff succeeds. Dependency links
remain and unfinished dependencies still prevent Ready. Closed issues require an
explicit reopen; drafts require undrafting. An attached fix or unspecified PR and
project PR support remain required.

A changed version, assignee, allocation, or worker attempt rejects the handoff.
Even a matching guard or `--force` cannot release another worker's unfinished
attempt, an unclaimed attempt, or another machine's reservation. Expired attempts
remain protected. An exact owner guard permits a metadata handoff of a manual
assignment without making the caller claim the issue. Legacy unguarded Ready is
limited to the authoritative store and the caller's ownership permissions.

On companions, read with `--supervisor` and use the same guarded command. Ready
routes through the existing supervisor tunnel; it never queues a lifecycle change
for offline replay. `fleet capabilities` reports `issue_ready`; older peers must
be upgraded. The web Ready action sends the displayed snapshot and retains retry
IDs after uncertain errors.

Verification: `cargo test --test issue_ready_guards`, focused Ready worker and
explicit dependency regressions, and `tools/issue_ready_browser_checks.js` through
Playwright CLI against `tools/serve_issue_reopen_fixture.mjs`. Browser checks use
synthetic native and paired stores, cover concurrent changes and retry recovery,
and inspect desktop/tablet/phone layouts in light and dark modes.
