# Surviving task attempts

A blocked or interrupted agent is not evidence that its task process stopped.
Report a surviving process on its owning machine before ending the agent:

```sh
hey-boss issue attempt hold 123 --if-version 7 --file attempt.json
```

The JSON contains `attempt_id`, `owner` (the original issue assignee), `pid`,
`log_path`, and `worktree`. Paths are absolute. Optional `process_start` supplies
an already recorded process identity when inspection is unavailable; a mismatched
or already-terminal process is rejected without creating a hold. A live process's
identity is captured automatically. `issue view --json` retains the resolved
identity, host, original Git directory, branch and HEAD.

The hold is independent of draft status, manual blocks, session/UI availability,
worker reservation deadlines and machine allocations. It survives claim release,
worker restart and repeated scheduler scans. An unfinished worker retains its slot
and process group; other jobs and concurrency settings are unchanged. Neither
`--force`, reopening nor retrying bypasses the hold. Ordinary comments and metadata
remain editable. A companion journals the hold with its guarded issue version;
fleet replay accepts that narrow delta without requiring an allocation and rejects
stale or foreign releases. Inspect sync conflicts before treating a companion's
local report as accepted by the supervisor.

After the process has actually ended, read its log and inspect the retained Git
work. Capture and review the evidence on the process host:

```sh
hey-boss issue attempt inspect 123 --json > evidence.json
hey-boss issue attempt reconcile 123 --if-version 9 --file evidence.json \
  --outcome 'Validation failed; reviewed the log and retained staged changes.'
```

Reconciliation requires a terminal process (including a reaped child, zombie or
reused PID), matching attempt identity, log inode/length/tail digest, Git directory,
HEAD, branch, status and tracked diff digest. Missing permissions or inspection
errors preserve the hold. Changed evidence or issue version requires another
review. The audit event retains the original report and reviewed outcome.

Release enables one ordinary, transactionally reserved continuation. It does not
close the issue, undo manual blocks/dependencies, change concurrency or discard
source files. Remove review JSON files when finished. See each command's `--help`.
