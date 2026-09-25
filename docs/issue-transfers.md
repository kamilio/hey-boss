# Moving issues between projects

Open an issue, choose **More issue actions → Move to project…**, select an
existing project, and choose **Move issue**. The issue opens in its destination
with a new number at the bottom of that project's queue. Description, labels,
state, Boss assignment, drafts, comments, comment resolutions, PR links, authors,
timestamps, and activity are preserved. Unsaved comment text follows the move.
Old issue URLs open the new location, and saved mindmap references follow it.

The move action is part of the initial issue header, including on phones and
paired devices. It remains available for open, blocked, ready, draft, and closed
issues if a progress, attachment, document, or subtask component fails to mount.
Deleted issues must be restored first. Component failures do not bypass the
server's ownership and transfer checks.
On narrow phones, the overflow control stays right-aligned when header buttons
wrap, keeping its menu within the screen.

`node tools/test_issue_transfer_controls.cjs` covers this render regression in
CI. `tools/issues_transfer_browser_checks.js` exercises the native and paired
interfaces with Playwright: component failures, keyboard access, light/dark
phone layouts, stale revisions, lost-response retries, and old-link redirects.

The terminal equivalent is:

```sh
hey-boss issue view 12 --json
hey-boss issue transfer 12 --destination PROJECT_ID --if-version VERSION
```

Use the version from the latest read. A stale version rejects the move without
changing either project. Reuse the same `--request-id` when retrying an uncertain
request. Moves run atomically on the authoritative store; fleet replicas must
use the supervisor. Agent claims and active worker reservations prevent moving.
Stop the work and release the claim first. Parent/subtask and document links must
be unlinked first; issues with a file-synced plan cannot move between projects.
Drafts require a destination that permits drafts. Hidden projects cannot receive
issues.

The original record becomes a permanent redirect and retains its audit history
and completed worker records. It cannot be restored or picked up as a duplicate.
