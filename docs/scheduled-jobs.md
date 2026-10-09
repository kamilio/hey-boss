# Scheduled jobs

`hey-boss job --help` manages schedules and run history. The fleet supervisor dispatches due jobs through an independent service, including with no saved workers or a paused, stopped or saturated worker pool.

Create requires a stable ID, project, name, cron, explicit IANA timezone, harness, logical model and user-written `.md` instructions. Edit replaces definition fields with an `--if-revision` guard; omitted instructions retain the prior file. Pause, resume and delete also require that guard. Delete retains revisions and history, and does not stop active work.

## Schedule contract

- Five fields: minute, hour, day of month, month, day of week. Numeric lists, ascending ranges and steps are supported; month/day names are case-insensitive. Sunday is 0 or 7. Seconds, years, macros, `?`, `L`, `W`, `#` and descending ranges are rejected.
- Day fields follow Vixie cron: if either starts with `*`, both must match; otherwise either may match. Thus `0 8 1 * MON` matches every first day and every Monday. A step applies within its field, not elapsed time.
- Missing local times are skipped. Repeated local times use the earlier UTC instant exactly once, including non-hour DST changes.
- Save/edit/resume schedules strictly after the change time. Pause suppresses future occurrences. Downtime coalesces to the latest missed occurrence. If that job is pending/running, the occurrence is recorded as skipped with reason `overlap`; other jobs proceed independently.
- Preview and next use `(after, through]`, UTC milliseconds in JSON and RFC3339 at the CLI. They return at most 1,000 occurrences within nine years. The calendar search is bounded; the nine-year horizon includes the eight-year leap-day gap across a non-leap century. Future entries are projections, never tasks.

## Persistence and routing

The existing resource API accepts `{"action":"job","operation":...}`; `hey-boss job rpc` accepts the inner operation. Commands are `create`, `edit`, `set_enabled`, `delete`, `view`, `list`, `preview`, `next`, `history`, `revision`, `run`, `run_now`, and `stop`. See `src/jobs/mod.rs` for the typed wire contract. All reads/mutations on companions use the authenticated supervisor tunnel and require its `scheduled_jobs` capability. There is no replica-write fallback. Mutations require a request ID and retain durable, content-bound receipts; the CLI derives one unless supplied; Run Now creates a fresh key for each invocation and prints its retry key on uncertain failure.

Markdown bytes are never normalized or stored as database prompt text. References point to SHA-256-named `.md` files beside the database in its `.jobs` directory. Files are published and fsynced before references commit. Revisions are immutable and retained after edits/deletion. A revision/run read transfers and verifies its exact bytes, then fsyncs the companion copy before acknowledging. The runner must fetch a run on its destination before launch; cached instructions can subsequently be loaded offline. Failed/crashed saves can leave unreferenced content-addressed files, which must not be removed while an in-flight save could reference them.

## Execution

Run Now can execute a paused schedule without enabling it. Use the same request ID to retry an uncertain request; overlapping scheduled/manual occurrences are recorded as skipped. Stop takes an exact run ID and cancels only its owned process group. Pause/delete affect future scheduling and retain current executions and history.

Each occurrence atomically creates its task at the top of the project list. A persisted, immutable job marker excludes it from ordinary pickup even after unassignment, failure or restart. Follow-up tasks use the ordinary pool. Success closes the execution task; failure/cancellation leaves an open task with an explicit result.

Fleet status reports job-service capability independently of workers, using existing project checkouts and installed harnesses. Pi selections use the configured `route/model-id`; other harnesses use an exact model ID. Instructions load from the immutable Markdown revision with no added agent prompt.

Owners are generation-fenced. A disconnect never releases ownership. A revoked, unsubmitted owner must acknowledge that it stopped before handoff; submitted work is never replayed. Process identity is committed before a launch gate opens, and submission intent is committed before sending instructions. After a service crash, surviving processes must be conclusively stopped before the attempt can finish. Unknown custody remains visible and blocks overlap.
