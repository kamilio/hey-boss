# Workers and agents

## Workers

Create a worker explicitly; it starts automatically. Watching or closing the dashboard does not change pickup.

```sh
hey-boss workers add --name Tools -C /work/hey-boss --concurrency 2
hey-boss workers status --json
hey-boss workers watch
hey-boss workers pause WORKER_ID
hey-boss workers resume WORKER_ID
hey-boss workers remove WORKER_ID
```

Repeat `-C` to share one worker's slots across several project checkouts.
`remove` stops new pickup and waits for current work to finish. For a retried
creation, reuse `add --id ID` with identical settings to avoid duplicate workers.

`hey-boss fleet setup --source /path/to/hey-boss` enables automatic configuration,
software deployment, and replica sync for the saved SSH inventory.

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
