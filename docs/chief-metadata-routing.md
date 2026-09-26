# Chief metadata through the connected supervisor

Use `hey-boss issue --supervisor` from a connected companion to read the current
issue and apply guarded metadata changes over its existing authenticated fleet
connection. No separately reachable SSH hostname or work reservation is needed.
On the supervisor itself, the same command uses its authoritative store.

`hey-boss fleet capabilities` reports the route, supervisor build and advertised
`issue_metadata`, `issue_draft` and `issue_reopen` support. `hey-boss fleet status` also exposes
capabilities. Issue help and allocation recovery messages point to this existing
connection; no socket inspection or SSH configuration is necessary.

```sh
hey-boss issue view 1341 --supervisor --project github.com/poe-platform/poe-code --json
hey-boss issue edit 1341 --supervisor --remove-label 'rework needed' \
  --if-version 8 --request-id chief-cleanup-1341-v8 \
  --project github.com/poe-platform/poe-code --json
```

Use the version returned by the first command; the numbers above illustrate the
syntax. A stale version rejects the entire edit. The response identifies
`store.host` as `supervisor`; ordinary companion reads remain replica snapshots
and can lag until the next pull.

| Operation | Supported with `--supervisor` |
| --- | --- |
| `view`, `allocation` | Authoritative read |
| `edit` title, body, labels | Requires `--if-version` and `--request-id` |
| `edit --draft` | Requires `issue_draft`, a current `--if-version`, and an eligible, unassigned, unreserved issue |
| `batch` label changes | Requires version and expected owner for every entry, `assignment: keep`, and `--request-id` |
| `reopen` | Requires `issue_reopen`, current `--if-version`, `--request-id`, and unassigned, unreserved work with no unfinished worker attempt |
| Other lifecycle operations, claims, assignment, reservations, worker controls | Rejected before mutation |
| Interactive editing, web server, RPC, migration and other commands | Rejected |

Label edits work for an unassigned closed issue and an issue assigned to a live
worker. They leave lifecycle, ownership and reservations unchanged. Title/body
corrections use the same version guard. Batches retain their all-or-nothing
version/owner checks. This route preserves the original actor and normal
authorization, including the restriction on the `yolo` label; it does not act as
Boss or acquire/release work.

Ordinary `issue edit NUMBER --draft --if-version VERSION --request-id ID` routes
through the tunnel automatically on a companion. Without an explicit request ID,
this automatic route derives a stable ID from the actor, project and guarded
operation. Explicit `--supervisor` uses the same draft eligibility guards and
requires an explicit request ID. Read the authoritative issue again after the
edit to verify its draft state. Assigned and reserved work remains protected.

The companion checks the operation before forwarding, and the supervisor checks
it again before executing its normal store transaction. Mutation receipts exist
only at the authority and are scoped to the original actor. Retry an uncertain
write with the **same request ID and identical operation**. A lost response may
mean the write completed. There is no local fallback or offline replay. An old
supervisor without the required metadata or draft capability returns an explicit
upgrade error with its build and the missing capability, before forwarding the
mutation. Upgrade the fleet from the supervisor and recheck capabilities. An
installed CLI that does not recognize these options must also be upgraded.

`--supervisor` cannot be combined with `--host` or `HEY_BOSS_ISSUE_HOST`. The
ordinary metadata/offline path and explicit SSH route retain their existing rules.
Unsupported lifecycle work must use those established paths with their normal
ownership and allocation checks. Never claim an issue merely to edit its labels.

To expose incomplete delivery, read the authoritative version, then use:

```sh
hey-boss issue reopen NUMBER --supervisor --if-version VERSION --request-id ID --project FULL_ID --json
```

Reopen retains the original actor and existing history. Closed issues with
unfinished dependencies become Blocked; they cannot be picked up until those
dependencies satisfy the project's readiness rule. Ordinary reopen of a blocked
issue still refuses unfinished dependencies. `--clear-manual-hold` explicitly
clears a blocked issue's manual hold while retaining dependency blocking.
No claim, allocation or unfinished attempt may be present, including an expired
reservation or an owner whose liveness is unknown. Checks and mutation share
one transaction. This route never releases ownership or changes worker controls.
Retries use the same ID and identical operation; a changed payload or stale
version fails. Disconnection has no local fallback or deferred replay. Supervisors
without `issue_reopen` reject the request before forwarding it; upgrade the fleet
and recheck capabilities.

This extends issue 145's relay and issue 151's metadata routing with issue 152's
discovery and guarded drafting, preserving issue 150's offline acceptance
protection and issue 56's general batch semantics.

Verification uses `cargo test --locked -p hey-boss --lib --test issues --test
fleet_native --test issue_allocation`, including real supervisor/companion
transport, replay, stale guards, unsupported operations and disconnected failure.
Run `HEY_BOSS_TEST_BINARY=/path/to/hey-boss node tools/chief_metadata_checks.mjs`
to qualify an installed binary against eighteen private fleet stages. It requires
normal exits and removes its services and temporary stores. Add `--serve` for the
57-assertion `tools/chief_metadata_browser_checks.js` visual session, then stop
the fixture and require its cleanup completion message. The 75-assertion
`tools/fleet_tunnel_browser_checks.js` session additionally covers guarded drafts,
tunnel recovery, phone layouts, and the committed draft response before replica sync.

Reopen qualification adds `--reopen` to the private-fleet command (23 stages).
With `--serve`, run `tools/reopen_tunnel_browser_checks.js` through Playwright CLI:
161 checks cover reopened, dependency-blocked and actively owned issues at
1440, 768, 390 and 320 pixels in light/dark themes. The Rust `fleet_tunnel`
integration test also checks uncertain ownership, reservations, stale versions,
request-ID misuse and disconnected failure; authority tests cover older peers.
