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

`hey-boss fleet capabilities` reports the existing authenticated supervisor tunnel,
negotiated issue capabilities, and supervisor build. `fleet status` shows connectivity.
If a capability is missing, run `hey-boss upgrade` on the supervisor to update the
fleet, reconnect, and inspect again. No separately reachable supervisor SSH hostname
or worker is needed. `issue --host HOST` remains an explicit SSH alternative and
never falls back locally.
On connected fleets with supervisor metadata support, use
`issue --supervisor view NUMBER --json` for an authoritative read; ordinary
companion reads can lag behind the supervisor.

Title/body/label edits through `--supervisor` require `--if-version VERSION`
from that read and `--request-id ID`. Label-only batches retain version/owner
guards and `assignment: keep`. Other lifecycle and ownership changes are rejected.
Never claim work just for metadata access. `--supervisor` cannot be combined with
`--host` or `HEY_BOSS_ISSUE_HOST`.

For retried mutations, reuse the same request ID and identical content after an
uncertain response. If the version is stale, read the current issue and reassess
before submitting a new operation.

For ordinary drafting, first read the current version through `--supervisor`, then
run `hey-boss issue edit NUMBER --draft --if-version VERSION`. On companions this
automatically uses the tunnel, preserves actor authorization, and still rejects
assigned or reserved work. Add `--request-id ID` for an explicit retry key; otherwise
the guarded draft edit derives a stable key and returns it as `request_id`.
Missing `issue_draft` support returns `fleet_capability_unsupported` before sending;
disconnection never falls back to a replica write. A timeout may mean a write was
saved: retry the identical command/key. Explicit `--supervisor --draft` also works
with `--if-version` and `--request-id`; undrafting is not supported on this route.
