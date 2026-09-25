# Chief metadata through the connected supervisor

Use `hey-boss issue --supervisor` from a connected companion to read the current
issue and apply guarded metadata changes over its existing authenticated fleet
connection. No separately reachable SSH hostname or work reservation is needed.
On the supervisor itself, the same command uses its authoritative store.

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
| `edit` title, body, labels | Requires `--if-version` and `--request-id`; no draft change |
| `batch` label changes | Requires version and expected owner for every entry, `assignment: keep`, and `--request-id` |
| Lifecycle, drafts, claims, assignment, reservations, worker controls | Rejected before mutation |
| Interactive editing, web server, RPC, migration and other commands | Rejected |

Label edits work for an unassigned closed issue and an issue assigned to a live
worker. They leave lifecycle, ownership and reservations unchanged. Title/body
corrections use the same version guard. Batches retain their all-or-nothing
version/owner checks. This route preserves the original actor and normal
authorization, including the restriction on the `yolo` label; it does not act as
Boss or acquire/release work.

The companion checks the operation before forwarding, and the supervisor checks
it again before executing its normal store transaction. Mutation receipts exist
only at the authority and are scoped to the original actor. Retry an uncertain
write with the **same request ID and identical operation**. A lost response may
mean the write completed. There is no local fallback or offline replay. An old
supervisor without the metadata capability returns an explicit upgrade error.

`--supervisor` cannot be combined with `--host` or `HEY_BOSS_ISSUE_HOST`. The
ordinary local/offline path and explicit SSH route retain their existing rules.
Unsupported lifecycle work must use those established paths with their normal
ownership and allocation checks. Never claim an issue merely to edit its labels.

This extends issue 145's relay without changing issue 150's offline acceptance
protection or issue 56's general batch semantics.

Verification uses `cargo test --locked -p hey-boss --lib --test issues --test
fleet_native --test issue_allocation`, including real supervisor/companion
transport, replay, stale guards, unsupported operations and disconnected failure.
Run `HEY_BOSS_TEST_BINARY=/path/to/hey-boss node tools/chief_metadata_checks.mjs`
to qualify an installed binary against twelve private fleet stages. It requires
normal exits and removes its services and temporary stores. Add `--serve` for the
57-assertion `tools/chief_metadata_browser_checks.js` visual session, then stop
the fixture and require its cleanup completion message.
