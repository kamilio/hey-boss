# Routine issue edits through the fleet tunnel

A connected companion already has an authenticated route to its supervisor. It
does not need a reverse SSH hostname, a running worker, or a work claim to edit
issue metadata.

```sh
hey-boss fleet capabilities
hey-boss fleet status
hey-boss issue view 1 --project github.com/quora-internal/ans --supervisor --json
hey-boss issue edit 1 --project github.com/quora-internal/ans --draft --json
```

The last command automatically
routes through the tunnel on companions; the response identifies `store.host` as
`supervisor` and returns its request ID. Explicit `--request-id` values are kept;
otherwise a stable key covers the actor, project, version and exact draft edit.
Repeat the identical command after an uncertain response. A different edit is a different operation. Concurrent changes fail without mutation.

Drafting requires open or blocked, unassigned work with no active worker
reservation. It retains the original actor and all normal project and label
authorization. It never claims work or takes over a reservation. Undrafting
is not added to the metadata route.

`issue close NUMBER --supervisor --request-id ID` closes completed work when
`issue_close` is advertised. The CLI captures the authoritative version, owner
and reservation snapshot. Eligible targets are unassigned work, your own claim,
or an idle Ready handoff to Boss/GitHub. Foreign allocations and unfinished or
unclaimed attempts are refused; an owner's own claimed attempt may finish its
task. `--force` is unsupported. The closing comment and actor attribution commit
with the lifecycle change. No claim, reverse SSH or replica write is needed.

`issue ready NUMBER` routes through the tunnel on companions; `--supervisor`
also works explicitly. The snapshot includes the current GitHub watcher event,
so idle triage can hand off reviewed work without claiming it. New findings
between the read and write cause a guard conflict. An unguarded handoff with
unacknowledged findings is refused, never reported as a successful Open result.
Ready records development usability, not CI, merge or production approval.

For both operations, use the same request ID and identical command after an
uncertain acknowledgement. `issue request ID --supervisor` retrieves the original
receipt. Replays return that committed result without overwriting later edits;
use `issue view NUMBER --supervisor` for the current state. Disconnection never
falls back to a replica mutation.

Guarded `issue reopen NUMBER --supervisor --request-id ID`
uses the same tunnel when `issue_reopen` is advertised. It requires unassigned,
unreserved work with no unfinished attempt. Unresolved dependencies remain
blocked; see [Chief reopen guards](chief-metadata-routing.md) for manual holds
and retry behavior.

Title, body and label edits use the explicit `--supervisor` route described in
[Chief metadata routing](chief-metadata-routing.md). That route also supports
drafting. Ordinary reads remain local
replica snapshots, which can lag behind the authoritative store.

`issue blocked-by NUMBER [BLOCKERS...] --supervisor
--request-id ID` replaces dependency links; omit blockers to clear them. It
requires `issue_dependencies` support and unassigned, unreserved work with no
unfinished worker attempt, even when the caller owns the claim. `--force` is
not supported on this route. The original actor, dependency validation and
version checks still apply. Successful retries return the original response;
reuse of an ID with different content fails. No claim or reservation is created
or released. Dependency state follows the existing Ready/Closed scheduling rules.

`issue pr add NUMBER URL --purpose prerequisite --supervisor --request-id ID`
attaches a PR when `issue_pr_attachments` is advertised. Use
`issue pr list NUMBER --supervisor` for authoritative read-back and
`issue request ID --supervisor` for the saved result. The CLI derives a stable
retry key when omitted. Reuse the original key and identical command after an
uncertain response; changing the operation under that key conflicts.

PR attachment is additive: a duplicate URL keeps its existing purpose, and the
operation never claims, releases or changes assignment, reservations or unfinished
attempts. It needs no issue-version or owner snapshot, so unrelated concurrent
edits do not prevent attachment. Replaying an older receipt returns that result
without overwriting newer links or purposes. PR reclassification, removal and
the local CLI's commit-URL alias are outside this capability.

`fleet capabilities` reports the negotiated `authority_rpc`, `issue_metadata`
and `issue_draft`/`issue_reopen`/`issue_dependencies`/`issue_pr_attachments` flags, route and supervisor build without fetching fleet
history. It also works when the supervisor predates metadata or draft support.
Missing support produces `fleet_capability_unsupported`, with the missing flag,
known build and `sent: false`. Run `hey-boss upgrade` on the supervisor to update
the fleet, then reconnect and inspect again. A missing connection produces
`fleet_unavailable`; there is no local write fallback or deferred replay.

Verification: `cargo test --locked -p hey-boss --test fleet_tunnel`, the authority
unit tests and existing fleet integration tests. Installed binaries can run the
18-stage `tools/chief_metadata_checks.mjs` private-fleet qualification; `--serve`
supports the desktop/phone visual checks in `tools/fleet_tunnel_browser_checks.js`.
Use `--dependencies --serve` for dependency routing and replication qualification,
then run `tools/dependency_tunnel_browser_checks.js` for desktop/phone checks.
Use `--pr-attachments --serve` and `tools/pr_tunnel_browser_checks.js` to verify
installed PR routing, replicated links and their desktop/phone presentation.
Use `--lifecycle --serve` and `tools/lifecycle_tunnel_browser_checks.js` for
Ready/close receipts, idle watcher handoffs, active-work guards and responsive
replica rendering. Browser screenshots go to `/tmp/hb-lifecycle-visual`; remove
them and stop the fixture and disposable browser after inspection.
