# Routine issue edits through the fleet tunnel

A connected companion already has an authenticated route to its supervisor. It
does not need a reverse SSH hostname, a running worker, or a work claim to edit
issue metadata.

```sh
hey-boss fleet capabilities
hey-boss fleet status
hey-boss issue view 1 --project github.com/quora-internal/ans --supervisor --json
hey-boss issue edit 1 --project github.com/quora-internal/ans --draft --if-version 1 --json
```

Use the version from the authoritative read. The last command automatically
routes through the tunnel on companions; the response identifies `store.host` as
`supervisor` and returns its request ID. Explicit `--request-id` values are kept;
otherwise a stable key covers the actor, project, version and exact draft edit.
Repeat the identical command after an uncertain response. A different version or
edit is a different operation. Stale versions fail without mutation.

Drafting requires open or blocked, unassigned work with no active worker
reservation. It retains the original actor and all normal project and label
authorization. It never claims work or takes over a reservation. Undrafting,
assignment and other lifecycle operations are not added to the metadata route.

Guarded `issue reopen NUMBER --supervisor --if-version VERSION --request-id ID`
uses the same tunnel when `issue_reopen` is advertised. It requires unassigned,
unreserved work with no unfinished attempt. Unresolved dependencies remain
blocked; see [Chief reopen guards](chief-metadata-routing.md) for manual holds
and retry behavior.

Title, body and label edits use the explicit `--supervisor` route described in
[Chief metadata routing](chief-metadata-routing.md). That route also supports
drafting with an explicit request ID and version. Ordinary reads remain local
replica snapshots, which can lag behind the authoritative store.

`fleet capabilities` reports the negotiated `authority_rpc`, `issue_metadata`
and `issue_draft`/`issue_reopen` flags, route and supervisor build without fetching fleet
history. It also works when the supervisor predates metadata or draft support.
Missing support produces `fleet_capability_unsupported`, with the missing flag,
known build and `sent: false`. Run `hey-boss upgrade` on the supervisor to update
the fleet, then reconnect and inspect again. A missing connection produces
`fleet_unavailable`; there is no local write fallback or deferred replay.

Verification: `cargo test --locked -p hey-boss --test fleet_tunnel`, the authority
unit tests and existing fleet integration tests. Installed binaries can run the
18-stage `tools/chief_metadata_checks.mjs` private-fleet qualification; `--serve`
supports the desktop/phone visual checks in `tools/fleet_tunnel_browser_checks.js`.
