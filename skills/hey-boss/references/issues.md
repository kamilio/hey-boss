# Issue coordination

## Drafts and handoffs

`issue create --title TITLE --draft` persists a draft without files or sessions.
`issue edit NUMBER --draft` drafts an eligible issue; `issue undraft NUMBER`
makes it runnable. Assign human work with `issue assign-to-boss NUMBER`; find it
with `issue list --assignee boss`.

For PR work, use `issue pr add NUMBER URL` to attach the PR and `issue pr list NUMBER`
to inspect its links. Linking a PR does not close the issue; close it when the
requested work is complete.

## Priority, blockers, and subtasks

```sh
hey-boss issue list --all --label ready --unassigned --json
hey-boss issue move NUMBER --before OTHER
hey-boss issue block NUMBER --by BLOCKER --comment 'Waiting for the API change.'
hey-boss issue subtask create PARENT --title TITLE --body MARKDOWN
```

`issue move` also accepts `--after OTHER`; omit both to move to the end.

Blocking releases the claim. Repeat `--by` for several dependencies;
`issue blocked-by NUMBER BLOCKER...` replaces the dependency list, and omitting
BLOCKERs clears it. `issue list --state blocked` finds paused work;
`issue reopen NUMBER` resumes eligibility. Blocking should be rare: make every
effort to resolve the issue, raise questions through `hey-boss notif ask`, and ask
the user for help before giving up. Explain the blocker and what enables progress.

Subtasks are ordinary issues linked atomically on creation. Use
`issue subtask add PARENT CHILD`, `list PARENT [--all]`, or `remove PARENT CHILD`
to link, inspect, or unlink. Unlinking preserves the issue.

## Remote reads and guarded edits

`issue --host HOST` uses the authoritative SSH host and never falls back locally.
On connected fleets with supervisor metadata support, use
`issue --supervisor view NUMBER --json` for an authoritative read; ordinary
companion reads can lag behind the supervisor.

Title/body/label edits through `--supervisor` require `--if-version VERSION`
from that read and `--request-id ID`. Label-only batches retain version/owner
guards and `assignment: keep`. Lifecycle and ownership changes are rejected.
Never claim work just for metadata access. `--supervisor` cannot be combined with
`--host` or `HEY_BOSS_ISSUE_HOST`.

For retried mutations, reuse the same request ID and identical content after an
uncertain response. If the version is stale, read the current issue and reassess
before submitting a new operation.
