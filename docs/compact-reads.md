# Compact CLI reads

`hey-boss issue list --compact --json` and `hey-boss mm show --compact --json`
return opt-in, versioned projections. Defaults stay unchanged. Neither command
needs a shell pipeline. `--compact` requires `--json`; map `--bodies` conflicts
with it.

Issue lists retain all existing filters, queue order, `--limit`, `--offset`,
`--all`, `order_version` and `next_offset`. Map pages contain 50 saved nodes by
default (`--limit 1..100`, `--offset`), `total`, map `version` and `next_offset`.
Follow `next_offset` until null. Empty pages are valid. Concurrent edits can shift
pages; restart traversal if the version/order changes. These are separate read
snapshots, not a cursor that freezes the database.

Both responses identify `projection: "compact"`, `projection_version: 1`,
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

Map fields retain node ID, project, alias, parent, position, kind, saved reference,
label/title and timestamps. Issue nodes include the issue projection above,
resource version and availability; missing/deleted issues remain visible with
`resource_health: "missing_or_deleted"`. Text nodes report `has_body` without
loading the text. PR nodes include stored attachment statuses/purposes, or
`not_checked` when no observation exists. Notification nodes remain saved nodes
with unknown availability and `not_checked`; the full view resolves the Inbox.

Map `scope: "saved_nodes"` excludes automatically expanded PR/resource nodes.
PR attachments remain in each issue summary. `links` contains saved directed
links incident to the current page, with endpoint IDs and kind; endpoints may
belong to another page/project. A link can appear on both endpoint pages;
deduplicate by `(from, to, kind)`. Link descriptions, external resource expansion,
artifacts, bodies/rendering, commit provenance, GitHub evidence and histories are
not loaded. Use `mm view NODE`, `mm links NODE`, or `issue view NUMBER` for details.
