# Companion metadata acceptance

Issue 150 reproduced a difference between local command acceptance and fleet
acceptance. The companion allowed label edits and guarded reopens on issues
without a local allocation. The supervisor then rejected their issue-row journal
entries with `Issue allocation was revoked or belongs to another machine`.
Append-only comments and audit events were accepted independently. A subsequent
pull restored canonical metadata, while the local success response and its audit
history remained.

The production conflict receipts identify poe-code issue 17, reopen version 81,
and issue 1104, label edit version 72. This is a transport acceptance defect,
separate from the decisions to close the migration issues. No production issue
rows, claims, or reservations are repaired automatically.

Companion metadata and lifecycle mutations now check their local allocation inside the same write
transaction as the mutation. Missing, foreign, or expired allocations return an
explicit error before an issue change, audit event, or request-ID receipt commits.
`--force` does not bypass this replication prerequisite. Comments and comment
resolution keep their append-only behavior. Claims and handoffs retain their
existing dedicated allocation and ownership checks. Authority-side mutations retain their
existing version, ownership, and worker guards. Valid allocated work retains its
offline journal and the supervisor's concurrent-field arbitration.

An allocation is permission for offline work, not a promise that no later actor
can change canonical state. Later ownership or same-field conflicts still retain
the attempted journal payload in fleet conflicts. Never obtain an allocation by
forcing a claim or releasing someone else's reservation just to edit metadata.
For unallocated companion edits, use the authoritative supervisor connection via
[`--supervisor`](chief-metadata-routing.md) for guarded metadata changes, or
`--host` when available; otherwise the operation fails explicitly and must be
retried deliberately once a valid route is available. DNS configuration is outside
this repair.

Pulls also reject a cursor older than the last committed pull before consuming
receipts or changing rows. This protects acknowledged state from a delayed old
snapshot and leaves the journal available for a current retry. Legitimate newer
writes continue to apply; an unexplained authority cursor rewind requires explicit
recovery rather than silently restoring older metadata.

Repeatable verification:

- `cargo test --locked -p hey-boss --test issues`
- `cargo test --locked -p hey-boss --lib fleet::native::replica::tests`
- `node tools/metadata_sync_checks.mjs /path/to/installed/hey-boss`
- Add `--serve` to keep the private companion viewer available at port 59650 for
  visual checks. Stop the fixture afterward to remove its services and files.

The installed-binary probe requires ten completed stages, advancing pull cursors,
drained journals, matching labels/lifecycle/comments/audit counts, and database
integrity. Every CLI invocation requires a normal expected exit. Fixture setup
only changes private temporary stores.

The editor preserves denied drafts, clears retry tokens only for definitive
allocation failures, and scrolls the error above the footer. Cancel restores the
Edit button's focus in Safari as well as Chrome. The dark Save hover state retains
readable contrast.

Qualification included 422 Rust tests passing (six optional tests ignored),
format and Clippy checks, four retry-token cases, and the ten-stage replica probe.
Chrome and WebKit each exercised 45 interaction/layout assertions across light
and dark themes at 1440, 768, 390, and 320 pixels. Four editor accessibility audits
per browser found no WCAG violations; desktop and phone screenshots were reviewed.
The private fixture has no inbox service, so inbox polling returns an expected
503 independently of metadata operations.
