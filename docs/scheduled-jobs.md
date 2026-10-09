# Scheduled jobs

`hey-boss job --help` manages durable definitions and reads run history. No timer or worker-pool pickup is enabled by this foundation. Run-now execution and automatic dispatch belong to the independent job runner.

Create requires a stable ID, project, name, cron, explicit IANA timezone, harness, logical model and user-written `.md` instructions. Edit replaces definition fields with an `--if-revision` guard; omitted instructions retain the prior file. Pause, resume and delete also require that guard. Delete retains revisions and history, and does not stop active work.

## Schedule contract

- Five fields: minute, hour, day of month, month, day of week. Numeric lists, ascending ranges and steps are supported; month/day names are case-insensitive. Sunday is 0 or 7. Seconds, years, macros, `?`, `L`, `W`, `#` and descending ranges are rejected.
- Day fields follow Vixie cron: if either starts with `*`, both must match; otherwise either may match. Thus `0 8 1 * MON` matches every first day and every Monday. A step applies within its field, not elapsed time.
- Missing local times are skipped. Repeated local times use the earlier UTC instant exactly once, including non-hour DST changes.
- Save/edit/resume schedules strictly after the change time. Pause suppresses future occurrences. Downtime coalesces to the latest missed occurrence. If that job is pending/running, the occurrence is recorded as skipped with reason `overlap`; other jobs proceed independently.
- Preview and next use `(after, through]`, UTC milliseconds in JSON and RFC3339 at the CLI. They return at most 1,000 occurrences within nine years. The calendar search is bounded; the nine-year horizon includes the eight-year leap-day gap across a non-leap century. Future entries are projections, never tasks.

## Persistence and routing

The existing resource API accepts `{"action":"job","operation":...}`; `hey-boss job rpc` accepts the inner operation. Commands are `create`, `edit`, `set_enabled`, `delete`, `view`, `list`, `preview`, `next`, `history`, `revision`, and `run`. See `src/jobs/mod.rs` for the typed wire contract. All reads/mutations on companions use the authenticated supervisor tunnel and require its `scheduled_jobs` capability. There is no replica-write fallback. Mutations require a request ID and retain durable, content-bound receipts; the CLI derives one deterministically unless supplied.

Markdown bytes are never normalized or stored as database prompt text. References point to SHA-256-named `.md` files beside the database in its `.jobs` directory. Files are published and fsynced before references commit. Revisions are immutable and retained after edits/deletion. A revision/run read transfers and verifies its exact bytes, then fsyncs the companion copy before acknowledging. The runner must fetch a run on its destination before launch; cached instructions can subsequently be loaded offline. Failed/crashed saves can leave unreferenced content-addressed files, which must not be removed while an in-flight save could reference them.

## Runner integration

`Store::due_jobs(now, limit)` uses the due index and computes at most 100 decisions outside a write transaction. `manual_job(project, id, request_key, now)` prepares a manual occurrence with its own stable deduplication key; paused jobs may run manually. `commit_job_occurrence` rechecks the revision/cursor and commits a run plus the runner's task-creation callback in one transaction. The callback receives the same database transaction, immutable snapshot and run ID; it must reserve the task for the independent runner before returning its number and must perform no network/process work. Launch happens after commit. This module intentionally never calls the callback from a timer.

Scheduled identity is unique per job and UTC occurrence, manual identity per job and request key. Retried commits return the original identity before examining later edits. A partial unique active-run index prevents overlapping execution. `start_job_run` records machine/session/start; `finish_job_run` records terminal state/reason/finish. Only a started run may succeed; pending launch failures/cancellations are retained. History uses a stable descending sequence cursor, 100 rows per page, with its own job-scoped index.
