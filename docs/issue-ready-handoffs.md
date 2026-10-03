# Guarded Ready handoffs

Ready means the caller considers the attached PR usable by dependent work. It
assigns the issue to Boss and unblocks eligible dependents. It does not assert
passing CI, completion, merge, or deployment, and performs no GitHub or Git action.

Run `hey-boss issue ready NUMBER`. The CLI reads the authoritative revision,
assignee, and reservation snapshot and supplies the guards internally. The reservation token covers allocation and unfinished worker
attempt identities, including claim timestamps and reservation expiry.

The authoritative transaction checks every guard before changing the issue,
recording the handoff, and reconciling dependencies. Retry an uncertain result
with the identical operation and request ID. A successful retry returns its saved
response, even if the issue has since changed, without repeating the handoff.
Reusing that ID for a different operation fails.

For a reconciled manual hold, add `--clear-manual-hold` to the guarded Ready
command. The hold clears only if the entire handoff succeeds. Dependency links
remain and unfinished dependencies still prevent Ready. Closed issues require an
explicit reopen. An attached fix or unspecified PR and
project PR support remain required.

For a useful draft source, add `--keep-draft` to the guarded Ready command. This
records a development handoff, assigns Boss, and unblocks eligible dependents
while retaining the source's draft flag and no-worker scope. It requires an
unreserved draft with no unfinished worker attempt. It never undrafts, launches
workers, releases reservations, or approves CI, review, merge or production.
Ordinary drafts and draft dependents remain unschedulable. Reopening the source
withdraws its Ready handoff while preserving its draft flag; dependent pickups
pause again. This path does not sync a linked plan or assert its completion.
Companions require the `issue_ready_keep_draft` capability; upgrade older peers.

A changed version, assignee, allocation, or worker attempt rejects the handoff.
Even a matching guard or `--force` cannot release another worker's unfinished
attempt, an unclaimed attempt, or another machine's reservation. Expired attempts
remain protected. An exact owner guard permits a metadata handoff of a manual
assignment without making the caller claim the issue. Legacy unguarded Ready is
limited to the authoritative store and the caller's ownership permissions.

On companions, use the same command. Ready
routes through the existing supervisor tunnel; it never queues a lifecycle change
for offline replay. `fleet capabilities` reports `issue_ready`; older peers must
be upgraded. The web Ready action sends the displayed snapshot and retains retry
IDs after uncertain errors.

After handing off your claimed task, run `hey-boss issue assign NUMBER github`
to transfer its automatic Boss assignment to the GitHub watcher. This atomic,
version-guarded transfer preserves Ready, dependent usability, and your running
attempt. It requires an open attached GitHub PR and the same actor that handed
off the claim, with no intervening ownership change, foreign reservation, or
foreign/unclaimed attempt. Comments and label edits do not revoke the handoff.
Another owner's handoff requires Boss to assign GitHub in the web UI. Do not
reopen, unassign, impersonate Boss, or use `--force` to enable the watcher.
Companions use the existing `issue_assignment` tunnel capability and never save
an offline assignment. For uncertain responses, retry the identical command with
the same `--request-id`; upgrade the supervisor if it still rejects your own handoff.

An explicit `--acknowledge-requirements` handoff remains bound to its actor, run,
requirements and GitHub evidence. The owner's later comments and field-identical
replica merges retain its exact version while recording preservation in history.
External comments, changed requirements and unexplained revision gaps require a
fresh acknowledgment. Ready and assignment commands still check exact versions
and reservations; final notes never acknowledge another actor's new work.

The mutation path supports resident workers that compare exact versions. Replica
replay invalidates the issue before publishing external history, even when comment
rows, events and issue deltas arrive separately. Existing version receipts remain
readable; the writer never rewinds versions or renews a stale acknowledgment.
Verify mixed builds with `node tools/requirements_handoff_install_checks.mjs BINARY BUILD --worker-binary PATH`.
This uses disposable state and does not restart the live pool.

When the watcher is behind the evidence you reviewed, add
`--reviewed-evidence reviewed.json` to that GitHub assignment. The file is a JSON
array of `{ "report": FULL_REPORT, "policy": REQUIRED_CHECKS_REPORT }` objects,
one for every attached open GitHub PR. Capture raw reports with
`hey-gh pr OWNER/REPO NUMBER` and `hey-gh required-checks OWNER/REPO NUMBER`, combine
their JSON into the array, and review those exact saved snapshots before handoff.
Projected PR lists, watcher summaries, and prose are not acknowledgement evidence.

The transaction requires complete, settled evidence with matching PR, head, base,
CI, and policy. It records the reviewed signal identities even if the watcher has
never fetched them. An unchanged delayed scan stays quiet; a changed review,
thread, head, rerun result, or required-check policy wakes work once. Incomplete
or mismatched evidence rejects the whole handoff. Newer watcher findings absent
from the supplied snapshot reject it too; inspect and reconcile those findings
before retrying. This acknowledges handled evidence, including deliberately
accepted optional failures, without asserting merge readiness. Companions require
the `issue_reviewed_github_handoff` capability. Evidence files can be deleted after
a confirmed handoff; retain them for an uncertain retry with the same request ID.

Labels, including `PR ready` and `rework needed`, are metadata: a label-only
batch preserves lifecycle and ownership. It advances the issue version, so an
older Ready guard must be refreshed. Dependency reconciliation uses lifecycle,
holds and dependency links, not label names. If a later read differs from a batch
receipt, inspect issue history for intervening lifecycle writes; a receipt records
the original result, not current state.

When a useful source has a separate runnable repair, put rework metadata on the
repair and hand off only the source with its fresh Ready guard. That transaction
unblocks source dependents while preserving the separate repair's labels, claim
and reservation. Do not add the repair as a source blocker unless it truly makes
the source unusable. If the source itself needs rework, explicitly reopen it;
that pauses new dependent pickups while preserving running work. A concurrent
source pickup rejects Ready instead of releasing the new owner.

Verification: `cargo test --test issue_ready_guards`, focused Ready worker and
explicit dependency regressions, and `tools/issue_ready_browser_checks.js` through
Playwright CLI against `tools/serve_issue_reopen_fixture.mjs`. Browser checks use
synthetic native and paired stores, cover concurrent changes and retry recovery,
and inspect desktop/tablet/phone layouts in light and dark modes. Draft-preserving
handoffs use `tools/issue_draft_handoff_browser_checks.js` against the same fixture.
