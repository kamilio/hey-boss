# Saved workers by machine

Run `hey-boss auto-workers` on a machine to apply its saved worker configuration and open one dashboard with project tabs. Reopening the dashboard reuses worker IDs and running processes. Quitting the dashboard leaves them running. Independent ChatGPT and Codex sessions are outside this configuration.

The existing fleet supervisor and machine companions keep workers running, reconcile configuration changes, and finish graceful removals. If the fleet has not been installed, run `hey-boss fleet setup` once on the supervisor.

## Two worker layouts

Use one worker per checkout when each checkout should have its own slot. Several checkouts of the same project stay separate workers and appear together in that project's tab:

```sh
hey-boss auto-workers add --name 'poe-code main' -C /work/poe-code --concurrency 1
hey-boss auto-workers add --name 'poe-code second' -C /work/poe-code-2 --concurrency 1
```

Use a multi-project worker when several projects should share a pool. Each selected project has an explicit checkout; its slots are shared across those projects:

```sh
hey-boss auto-workers add --name 'Tools' --concurrency 2 \
  -C /work/hey-proxy -C /work/hey-gh -C /work/ashby-mcp
```

A second worker may use a different set of those checkouts. It has its own slots, filters, and history. All multi-project workers appear together in one **Shared** tab after the dedicated project tabs. Its label shows the worker and unique-project counts, such as **Shared · 2 workers · 5 projects**. Workers covering all projects also appear here. Each agent row identifies its project; Chiefs and completed attempts remain available in the same tab. Tabs do not combine worker configurations or multiply a shared pool's capacity.

## Live controls

- **Tab / Shift+Tab:** next / previous tab.
- **↑ / ↓:** select an agent across the selected tab's workers.
- **w:** list the selected tab's workers, including idle workers, their checkouts and slot counts.
- **a:** add a worker. Enter its name, slot count and checkout. Tab changes fields; Ctrl+N adds another checkout; Enter saves and starts it.
- **d** in the worker list: remove the selected worker after confirmation. It stops picking up new issues immediately, waits for its current agents and Chief, then exits. It stays marked for removal in saved configuration so reconnecting cannot resurrect it.
- **h:** switch active work and attempt history.
- **q / Ctrl+C:** close the dashboard.

The existing `s` control remains an explicit immediate stop; use `w`, then `d`, for graceful removal.

The same controls work from another terminal while the dashboard is running:

```sh
hey-boss auto-workers config
hey-boss auto-workers --json status
hey-boss auto-workers remove WORKER_ID
```

For scripts retrying a creation request, pass the same `add --id ID` and settings. Replays reuse that worker; different settings under the same ID are rejected. The dashboard does this automatically when retrying a failed submission.

## Configuration location

`hey-boss auto-workers config` prints the current machine's configuration and source path. The supervisor's canonical file is `~/.hey-boss/fleet.json`, under `machines.local.workers` and `machines[SSH_HOST].workers`. Companions retain their machine's copy in `~/.local/share/hey-boss/fleet-agent.json`. Add/remove controls save locally and synchronize to the supervisor, including after reconnecting.

A worker definition contains its stable `id`, `config`, and `intent` (`running`, `pause`, `stop`, or `drain`). `config.directory` is for one project; `config.directories` maps project IDs to checkouts for a shared worker. Invalid configuration is rejected before starting workers. Retired definitions remain as tombstones.

## Current fleet layout and capacity

The September 24 inventory found these hey-boss worker processes, separately from independent ChatGPT/Codex sessions:

| Machine | Hardware | Worker layout | Configured slots |
| --- | --- | --- | --- |
| Local Mac | 15 logical CPUs, 24 GiB RAM | Four poe2 workers; four shared pools for hey-gh, hey-proxy and ashby-mcp; four hey-boss workers | 36 |
| MacBook | 10 logical CPUs, 16 GiB RAM | Seven single-slot poe-code workers across separate checkouts, with two using the same base checkout | 7 |
| Devbox | 4 logical CPUs, about 31 GiB RAM | Two single-slot poe-code workers with separate checkouts | 2 |

Saved configuration initially preserves these choices and all current sessions. The useful structure is one slot per isolated checkout and an explicitly sized pool for multi-project workers. Identical project names are not duplicates when their checkouts differ. Repeated launches of the same saved configuration should add zero slots.

The local Mac's 36 configured slots are a ceiling, not proof that 36 agents are active. Reserve memory for independent Codex sessions and project builds. Prefer removing an overlapping worker gracefully over increasing concurrency to compensate for a slow queue; use the live worker list to choose the exact checkout and pool to retain.
