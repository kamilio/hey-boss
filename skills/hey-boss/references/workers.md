# Workers and agents

## Start or observe

```sh
hey-boss worker run --concurrency 2 --tag ready
hey-boss worker status --json
hey-boss worker watch
hey-boss auto-workers status --json
hey-boss auto-workers watch
```

`worker run` starts an independent worker; omit `--tag` for unrestricted pickup.
Standalone queues are per machine. `status` prints one snapshot; `watch` opens
the dashboard. Bare `worker` and `auto-workers` show help.

`auto-workers run` applies the machine's saved configuration and explicitly
resumes paused pickup. Use `status`, `config`, or `watch` to observe without
resuming it. Quitting the dashboard leaves workers running.

## Saved workers and fleet

```sh
hey-boss auto-workers add --name 'Tools' -C /work/hey-boss --concurrency 1
hey-boss auto-workers config
hey-boss auto-workers remove WORKER_ID
```

Repeat `-C` to share one worker's slots across several project checkouts.
`remove` stops new pickup and waits for current work to finish. For a retried
creation, reuse `add --id ID` with identical settings to avoid duplicate workers.

`hey-boss fleet setup --source /path/to/hey-boss` enables automatic configuration,
software deployment, and replica sync for the saved SSH inventory. For remote
standalone workers, `worker --host HOST` runs on that host and `-C` paths belong
to it.

## Agent sessions

Use `hey-boss agent list` to discover sessions, `agent overview` for combined
status, and `agent configure --help` for integration setup. `agent control`
targets a loaded Codex thread on its owning server and uses JSON stdin/output:

```sh
printf '{}' | hey-boss agent control inspect --thread THREAD_ID
hey-boss agent control --help
```

Control requires an owning app-server socket configured in
`~/.config/hey-boss/agent-control.json`; standalone terminal sessions cannot be
controlled. Use the discovered thread ID for any subsequent control; starting
workers and steering an existing session are separate operations.
