# Moving issues between projects

Open an issue, choose **More issue actions → Move to project…**, select an
existing project, and choose **Move issue**. The issue opens in its destination
with a new number at the bottom of that project's queue. Description, labels,
state, Boss assignment, drafts, comments, comment resolutions, PR links, authors,
timestamps, and activity are preserved. Unsaved comment text follows the move.
Old issue URLs open the new location.

The move action is part of the initial issue header, including on phones and
paired devices. It remains available for open, blocked, ready, draft, and closed
issues if a progress, attachment, document, or subtask component fails to mount.
Deleted issues must be restored first. Component failures do not bypass the
server's ownership and transfer checks.
On narrow phones, the overflow control stays right-aligned when header buttons
wrap, keeping its menu within the screen.

The Rust browser checks exercise keyboard access, light and dark phone layouts,
failed components, stale revisions, lost replies, and old-link redirects. After
building the CLI, run `cargo test --locked --example web-fixture browser::issue_transfer -- --ignored`.
Screenshots are written to `out/issue-transfer/`.

The terminal equivalent is:

```sh
hey-boss issue view 12 --json
hey-boss issue transfer 12 --destination PROJECT_ID
```

The CLI captures the current revision. A concurrent edit rejects the move without
changing either project. Reuse the same `--request-id` when retrying an uncertain
request. Moves run atomically on the authoritative store; fleet replicas must
use the supervisor. Agent claims and active worker reservations prevent moving.
Stop the work and release the claim first. Parent/subtask and document links must
be unlinked first; issues with a file-synced plan cannot move between projects.
Drafts require a destination that permits drafts. Hidden projects cannot receive
issues.

The original record becomes a permanent redirect and retains its audit history
and completed worker records. It cannot be restored or picked up as a duplicate.
