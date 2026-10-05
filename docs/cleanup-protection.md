# Automatic cleanup safety

Use `hey-boss health` (or `hey-harvester`) for checkout and dependency cleanup.
Every in-scope candidate reaches automatic safety checks without an owner release.
Age or a prior successful check alone never authorizes deletion.

```sh
hey-boss health cleanup-check /absolute/worktree --json
hey-boss health remove-worktree /absolute/worktree --json
hey-boss health remove-dependencies /absolute/worktree/node_modules --json
```

Worktree cleanup preserves active or queued work, locks, staged/unstaged changes,
non-ignored untracked files, unpublished commits, primary checkouts, uncertain
ownership and persistent data. Scheduled cleanup also applies its age and quiet
observation policy. Explicit removal bypasses age only. Only the owner should
unlock an ownership lock after finishing all use, including queued validation.

Dependency removal checks an exact `node_modules` directory and repeats ownership
checks immediately before removal. Tracked dependencies, databases (including
SQLite-format package assets), repository metadata and filesystem protections
prevent removal. Generic cache sweeping excludes `node_modules`; ordinary caches
keep their existing age, activity, ownership and persistent-data checks.

Ownership inventory includes each session independently, unfinished claims,
queued worker runs, fleet reservations and retained attempts. Missing or ambiguous
inventory fails closed. Older issue services require inventory version 2. A
standalone harvester without an issue service still checks Git ownership, locks
and active files/processes. No release records are created, consumed or migrated.

`cleanup-check` reports current safety and worktree age checks without deleting.
It does not replace scheduled quiet observations or the final removal checks.
`--json` returns an accepted/rejected receipt with a concrete reason; rejection
exits nonzero. Requests and receipts remain in bounded health activity history
(`health logs --json`); scheduled decisions appear in the Worktrees view.

Use the guarded removal commands instead of acting on a stale candidate list.
Regression checks use disposable fixtures, never production cleanup candidates.

## Recovering a full companion disk

Measure available blocks and inodes on the home volume, then inspect directory
allocation and APFS snapshots. A reconnect does not establish adequate build
headroom. Record both allocated bytes removed and the net free-space change;
concurrent builds and shared APFS blocks can make those numbers differ.

Preserve worker intent, source, active validation dependencies, session rollouts,
task databases and ownership records. A disconnected worker's missing run list
does not prove its agents stopped. Reuse the existing supervisor connection and
let its normal deployment retry recover after space is available.

Diagnostic SQLite retention is separate from cache cleanup. Validate the exact
schema first, retain recent evidence, and delete only expired diagnostic rows in
bounded transactions with busy handling. Compact incrementally only when the
database supports it; a full vacuum needs additional free space. Verify integrity
and checkpoint results afterward. Never delete a database or its live sidecars
to recover space.

Verify installed receipts on every connected machine, a successful deployment,
advancing heartbeat and sync on the same node, and the original worker/run
records. Confirm retained source still exists before considering recovery done.
