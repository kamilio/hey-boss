# Auto-workers ownership

`hey-boss auto-workers` manages one explicit, saved group on the current machine. Its membership is in `~/.local/share/hey-boss/auto-workers.json`:

```json
{"worker_ids":["selected-worker-id"]}
```

Worker settings remain in the existing fleet configuration. The ownership file only selects IDs; it does not duplicate settings or discover processes. A fresh installation has an empty group. `auto-workers add` enrolls its new worker automatically. Reopening the dashboard reuses the group. Additions made through another worker command, remote workers, and independent Codex sessions are never enrolled automatically.

An existing worker can be explicitly adopted by adding its ID to this file. `auto-workers config` reports both the ownership file and the backing fleet configuration. Removing an ID from this list only excludes it from this dashboard and its launch controls; it leaves the worker running. Use `auto-workers remove ID` for graceful removal of an owned worker.

Each refresh reads activity only for owned IDs. Each project or shared tab totals the same workers that supply its agent rows. Current work on a draining worker remains visible until completion, while that worker contributes no available slots. There is no global Codex-session scan in this path.
