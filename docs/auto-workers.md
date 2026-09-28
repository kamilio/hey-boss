# Worker fleet

Open **Agents → Workers** (`/workers`) to see every machine, worker, active task,
and configuration status. Expand **Edit fleet configuration** to edit YAML,
preview the changes, and save. The local web app and paired web app edit the
same file on the supervisor.

## One configuration

The supervisor owns `~/.hey-boss/fleet.yaml`. Machine keys are SSH host names;
`local` means the supervisor. Worker IDs are stable and unique across the fleet.

```yaml
machines:
  local:
    workers:
      - id: tools
        intent: running
        config:
          name: Tools
          concurrency: 2
          projects: [github.com/kamilio/hey-boss]
          directory: /Users/me/Workspace/hey-boss
  devbox:
    workers:
      - id: shared-tools
        intent: pause
        config:
          concurrency: 1
          projects:
            - github.com/kamilio/hey-proxy
            - github.com/kamilio/hey-gh
          directories:
            github.com/kamilio/hey-proxy: /work/hey-proxy
            github.com/kamilio/hey-gh: /work/hey-gh
```

A shared worker divides its slots across the selected projects. Separate
checkouts use separate worker IDs. Use a new ID when moving a worker to another
machine, and remove the old definition to drain it.

`intent` is `running`, `pause`, `stop`, or `drain`. Pause stops new pickup while
current work finishes. Stop cancels that worker's agents. Drain finishes current
work and then exits. Deleting a definition has the same effect as drain.
Changing concurrency or pickup settings leaves current agents running.

The supervisor validates the whole document, then each machine validates its
own checkout paths and executable before applying its settings. The dashboard
distinguishes saved, applied, offline, and failed changes. Invalid manual edits
leave the last valid configuration active. Processes retry failed starts with
backoff; reopening a dashboard does not start or resume anything.

The editor saves the exact YAML text, atomically, with a revision check to
prevent overwriting another edit. The previous file is kept at
`~/.hey-boss/fleet.yaml.previous`. CLI and worker controls update the same YAML;
these structured edits can reformat it. Machine-local JSON files are runtime
caches and pending changes, not additional configuration files to maintain.
Offline edits that conflict with newer supervisor settings are rejected.

Existing `fleet.json`, machine inventory, and pending local worker definitions
migrate automatically on first use. Existing IDs, intents, and processes are
preserved. The legacy file is retained as a backup. After migration, discovery
never enrolls independent workers or Codex sessions in this configuration.

## Terminal controls

```sh
hey-boss auto-workers run       # Apply this machine's config; preserve pause/stop
hey-boss auto-workers watch     # Observe the live dashboard
hey-boss auto-workers status    # Print one snapshot
hey-boss auto-workers config    # Show this machine's definitions and source
hey-boss auto-workers add --name Tools -C /work/hey-boss --concurrency 2
hey-boss auto-workers remove WORKER_ID
```

Repeated `run` calls reuse worker identities and live processes. Unlike the
previous behavior, `run` does not resume paused workers; use the web Resume
pickup control or change their YAML intent. Closing the auto-workers dashboard
leaves its workers running.

The TUI keeps project tabs and a Shared tab for multi-project pools. Tab switches
projects, `w` lists workers, `a` adds one, `d` removes the selected worker, `h`
shows attempt history, and `q` closes the dashboard. `p` pauses pickup; `s` stops
the selected worker immediately. Use `--help` for command options.
