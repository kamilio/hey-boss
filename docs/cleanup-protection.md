# Cleanup releases

Use `hey-boss health` (or `hey-harvester`) for checkout and dependency cleanup.
An old candidate list, a quiet process, a shared application PID, an unlocked
checkout, and ignored files do not establish that work is disposable.

After the owning session confirms completion, release the **exact** target:

```sh
hey-boss health release-cleanup /absolute/worktree/node_modules --owner codex:SESSION
hey-boss health cleanup-check /absolute/worktree/node_modules --json
hey-boss health remove-dependencies /absolute/worktree/node_modules --json
```

For an entire linked checkout, release its root and use `remove-worktree`.
Only the owner should unlock an ownership lock. Publication, tracked-file,
process, database, filesystem and primary-checkout protections still apply.
A release records the caller's owner attestation; the label does not impersonate
an agent or release an issue claim. Do not derive it from a PID or candidate list.

Before resuming work or queuing validation, withdraw the release with
`retain-cleanup PATH`. A Git worktree lock also blocks removal. Releases have no
age expiry. They bind the exact path, directory identity, HEAD and index state;
replacement or changed source invalidates them. Removal consumes the release,
so rebuilding dependencies does not authorize a second removal. After a rejected
or interrupted operation, inspect the reason and target before releasing again.

All manual and scheduled worktree removal passes the same final release and
ownership gate. Dependency removal is limited to an exact `node_modules`
directory, checks its contents, then repeats the gate immediately before removal.
Generic cache sweeping excludes every `node_modules`, including saved cursors.
It cannot be used as an alternative dependency-removal path.

The issue ownership inventory includes each session independently, unfinished
claims regardless of PID presence, queued worker runs, fleet reservations and
retained attempts. Missing or ambiguous inventory fails closed. Older issue
services must be upgraded to inventory version 2 before cleanup proceeds.
Git locks and active files/processes remain additional exclusions. A standalone
harvester without an issue service still requires an explicit owner release.

`--json` returns an accepted/rejected receipt and a reason. Rejection exits
nonzero. Requests and receipts are saved in the bounded Machine Health activity
history (`health logs --json`); scheduled worktree decisions also appear in the
Worktrees view. Export receipts externally if longer retention is needed.
A successful check is informational: removal always rereads the current state.

Ad-hoc filesystem removers are outside this cooperative boundary. Replace their
destructive calls with `remove-dependencies` or `remove-worktree`; do not run
`rm` based on a prior successful check. No tool can stop an unrelated same-user
script from ignoring these guards. Never edit a running remover's manifest as a
substitute for acknowledged exclusions. Before any real sweep, confirm every
active or retained root with its owner. Regression checks use disposable fixtures.
