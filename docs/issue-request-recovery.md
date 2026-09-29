# Recovering an uncertain issue creation

Inspect the saved creation receipt on the original store, with the original
project and actor:

```sh
hey-boss issue request REQUEST_ID --project FULL_ID --agent ORIGINAL_ACTOR --json
```

Use `--supervisor` from a connected companion, or `--host HOST` for the original
remote store. `fleet capabilities` advertises `issue_request_status` for the
supervisor route. This command reads a snapshot without registering a project,
creating an issue, changing ownership, or retrying a mutation.

`recorded` includes the original operation and saved response, including its issue
number. The saved response describes creation time; use `issue view` for current
state. `not_recorded` means only that this store snapshot has no matching receipt.
It does not prove non-creation: another store, actor, pending caller, or unsynced
companion may hold the result. A list/search miss is equally inconclusive.

After checking the scope, repeat the **identical original creation** with its
original `--request-id`, project, actor, body, labels, placement and dependencies.
A committed request returns its saved response; a rolled-back request creates
once. Concurrent identical retries serialize and return one result. Different
content under the same ID is rejected. If the original arguments are unavailable,
coordinate with their owner instead of creating a replacement.

## Transaction recovery

A standalone SQL batch owns any transaction it opens: failure rolls it back
before the next request. A rejected nested BEGIN preserves its caller's existing
transaction. Lost rollback replies discard the dead transport so the same client
can reconnect; they never replay mutations. Issue creation, dependencies, numbering
and request receipts still commit atomically. A lost COMMIT reply remains ambiguous
until the receipt or an identical retry reconciles it.

Regression coverage uses isolated stores and service connections, including failed
batches, dependent creation after failure, caller-owned rollback, lost rollback and
commit replies, invalid dependencies, and concurrent identical retries. Recovery
requires no manual database edits or service restart.
