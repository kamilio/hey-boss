# Compact CLI reads

`hey-boss issue list --compact --json` returns an opt-in, versioned projection.
Defaults stay unchanged. The command needs no shell pipeline. `--compact`
requires `--json`.

Issue lists retain all existing filters, queue order, `--limit`, `--offset`,
`--all`, `order_version` and `next_offset`.
Follow `next_offset` until null. Empty pages are valid. Concurrent edits can shift
pages; restart traversal if the version/order changes. These are separate read
snapshots, not a cursor that freezes the database.

Responses identify `projection: "compact"`, `projection_version: 1`,
`omitted`, and `health` (store machine/role, authority, stored-snapshot freshness).
An omitted field means **not loaded**, never empty, absent, healthy or resolved.
Database and routing failures keep their normal structured errors and exit codes.

Issue fields:

- Number, title, state, draft, version, labels, sort order and lifecycle timestamps.
- Assignee ID and saved assignment target; allocation reason, reserved machine,
  expiry, authority and active worker reservation/claim state.
- Latest status level/time, agent launch count, comment count on issue lists,
  and presence of an attempt hold (details require `issue view`).
- Manual blocking, explicit `blocker_numbers`, `parent_number`, and direct
  `child_numbers`. Relationships include saved deleted references; these IDs do
  not assert that a dependency is satisfied or that a referenced issue exists.
- Attached PR URL, purpose, stored status, check time and error. A stale or unknown
  PR status is not a successful check; no network refresh is performed.
