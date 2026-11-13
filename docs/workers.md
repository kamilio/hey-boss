# Worker fleet

Open **Workers** (`/workers`). Each machine lists its saved workers. Use **Add worker** to create one, or **Add project** on an existing worker to share its agent slots across projects.
Offline machines show last-known activity and contribute no available capacity.
Organizers have a separate disclosure because they do not consume issue-agent slots.

**Project settings → Configuration → Worker execution** sets the Codex model,
reasoning level, and permissions for that project on every machine. Auto is the
default permissions mode. Inherit leaves the machine's Codex configuration in
control; YOLO disables the sandbox and approval requests. Changes apply to the
next task or scheduled Codex job run, including resumed threads. Jobs use the
project model when their Model field is empty; an explicit job model overrides it.
Changing a job's effective model starts a fresh thread. An issue's `yolo` label
overrides project permissions and can also be added or removed through the CLI.
Model and reasoning inherit when left unset.

Use **Workers & settings** to find a saved worker, inspect its state, or edit its
name, slot count, pickup mode, projects, and checkout paths. Review and save the
changes directly to the supervisor's YAML. Other settings are preserved; structured
edits may reformat the file. Stopped workers are collapsed by default. The configuration
file path is shown below the workers; edit that file in your own editor for YAML changes.
The local web app and paired web app edit the same supervisor file.

From a project checkout, run `hey-boss workers run 42` to execute just issue #42
with Codex in the current directory. The terminal shows the expanded prompt, agent
messages, tool calls, command output and exit codes, then the recorded outcome.
The usual pickup checks apply. Ctrl-C stops the attempt; closing or blocking the
issue has the same effect as in a background run. Pipe to `tee` to keep a transcript.

## One configuration

The supervisor owns `~/.hey-boss/fleet.yaml`. Machine keys are SSH host names;
`local` means the supervisor. Worker IDs are stable and unique across the fleet.
Registered companions in `~/.hey-boss/config.json` also stay connected for sync,
even with no saved workers. Removing a worker drains it without disconnecting the machine.

With a custom `HEY_BOSS_FLEET_STATE`, `fleet.yaml` and `config.yaml` default to
that directory, keeping private fleets separate from the home inventory.
`HEY_BOSS_FLEET_DESIRED` and `HEY_BOSS_FLEET_CONFIG` can explicitly select other files.

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
            - github.com/me/service-a
            - github.com/me/service-b
          directories:
            github.com/me/service-a: /work/service-a
            github.com/me/service-b: /work/service-b
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
hey-boss workers watch     # Observe the live dashboard
hey-boss workers status    # Print one snapshot
hey-boss workers config    # Show this machine's definitions and source
hey-boss workers add --name Tools -C /work/hey-boss --concurrency 2
hey-boss workers remove WORKER_ID
```

Creation starts the worker automatically. Use `workers pause ID` or `workers resume ID` to change pickup; quitting the dashboard leaves workers running.

The TUI keeps project tabs and a Shared tab for multi-project pools. Tab switches
projects, `w` lists workers, `a` adds one, `d` removes the selected worker, `h`
shows attempt history, and `q` closes the dashboard. `p` pauses pickup; `s` stops
the selected worker immediately. Use `--help` for command options.

## One-shot commands

`exec` runs commands in separate execution directories. Project selection allocates workers; it does not select the command's checkout, harness, or model.

```sh
hey-boss exec check --project poe2 --harness claude --model claude-opus-5-5
hey-boss exec submit --project poe2 --request-id benchmark-task-01 \
  --harness claude --model claude-opus-5-5 --effort high \
  --input ./trial-input.tar.gz \
  -- agent-eval trial --input input.tar.gz --output result/
hey-boss exec status --project poe2 EXECUTION_ID --json
hey-boss exec logs --project poe2 EXECUTION_ID --follow
hey-boss exec collect --project poe2 EXECUTION_ID --output ./results/task-01
hey-boss exec cancel --project poe2 EXECUTION_ID
```

Retry an uncertain submission with the same request ID and identical arguments/input. A different command under that ID is rejected. Files transfer in verified chunks through the fleet connection; no S3 is required. The input is staged as `input.tar.gz`. Files written to `result/` are retained and collected into a new local directory. Outputs remain available after success, failure, and graceful cancellation.

Commands receive `HEY_BOSS_EXEC_HARNESS`, `HEY_BOSS_EXEC_MODEL`, and `HEY_BOSS_EXEC_EFFORT`. These values are stored with the execution and never inherit project defaults. Commands that do not run an agent may omit harness/model together. Shell expansion is not applied to arguments.

Readiness reports the installed harness, CLI login status, credential source, evaluator version, and worker capacity. Codex can use its CLI login, an API key from the worker environment, or verified proxy credential sources. Claude requires a confirmed CLI login; Gemini login checks are not yet supported. Authentication checks never return credential contents and do not test access to the requested model. A real trial is required to establish model access. Companions must advertise `exec_v1` before they can receive commands. One-shot commands reserve worker capacity alongside issue work; cron jobs keep their existing scheduling behavior.

Cancellation gives the command process group 30 seconds to save output before forcing it to stop. Existing commands survive a client disconnect. The supervisor owns pending placement, so put it and the benchmark coordinator on a server for laptop-independent scheduling.
