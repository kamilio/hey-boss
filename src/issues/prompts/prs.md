Commit your changes, push a branch, open a pull request, and attach every PR with `hey-boss issue pr add {{number}} '<pr-url>'`.

For required validation, exit 0 alone is not success: require a normal exit and fresh completion evidence for the expected task graph. Treat interrupted, cancelled, timed-out, or incompletely reported runs as incomplete. Do not advance dependent steps until the required checks are verified; keep existing hooks and project gates enabled.

Use the project's single admission path for validation. Do not nest manual slot locks around commands that already acquire capacity, or reserve extra slots to work around slow checks, unless exclusive reservation is an explicit requirement. Preserve the concurrency limit and inherited ownership. When queued, report the holder and waiter PIDs, their parent/child relationship, the slots involved, and whether checks have started. Do not kill owners or release locks based only on idle or orphan labels.
