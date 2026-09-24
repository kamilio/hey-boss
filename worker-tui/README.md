# Worker terminal dashboard

A live dashboard for hey-boss workers, built with Ratatui and Crossterm. Queue
requests run on a background thread; keyboard input and rendering stay responsive
during slow local or SSH requests. Only changed terminal cells are written.

```sh
hey-boss worker                          # Start a worker with its dashboard
hey-boss worker status                   # Watch workers without starting one
hey-boss worker --host devbox status      # Watch an SSH host
hey-boss worker --json status             # Machine-readable output

# Optional standalone executable using the same library:
cargo install --locked --path worker-tui
hey-boss-worker-tui
hey-boss-worker-tui --host devbox --directory /home/me/hey-boss
hey-boss-worker-tui --id WORKER_ID --history
```

Requires an installed hey-boss version supporting `worker --json status` and an
interactive macOS or Linux terminal. `--binary PATH` selects a development CLI.
The CLI's `HEY_BOSS_ISSUE_HOST` and `HEY_BOSS_ISSUE_DB` settings are inherited.
This package is independent of the root crate. The main CLI embeds the same
library source from `src/worker_tui`, so ordinary installation and fleet upgrades include the dashboard
without installing a separate executable. `runtime::run` accepts a cancellation
token so workers can restore the terminal before shutdown or reload.
When started by `hey-boss worker`, q / Ctrl+C stops that worker and its sessions.
With `worker status` or the standalone executable, quitting leaves workers running.

Only the current worker appears. Starting a worker pins its own ID; `worker status`
shows the latest worker, or use `--id WORKER_ID` to watch a specific one. A prominent
AVAILABLE / BUSY badge and busy/available slot counts show its capacity. Pause and
update states explain that existing sessions finish before pickup resumes.
The header shows the selected worker's project names and checkout directory on
separate lines, including while finishing work. Workers without a fixed directory
show “Per-project checkouts”. iTerm tab titles and tmux window names use the same
project and checkout context, including when viewing status from another directory.
Naming is best-effort, runs in the background, and changes only when the context
changes; tmux targets the original pane and times out after 500 milliseconds.

Active work is the default view. Press h to switch to a separate completed-attempt
history; completed attempts never consume slots. Selection follows session IDs
through refreshes. Supervisor shows connectivity: this machine for the
supervisor, connected/disconnected for fleet companions, or not configured for local
queues. Companion connectivity uses the existing five-second heartbeat, expires after
fifteen seconds, and shows changes waiting to sync. Unknown means status could not
be read. Request errors retain the last successful sessions, mark connectivity
unknown, and retry every two seconds. Deleted workers in status dashboards trigger
an inventory refresh; an owning dashboard stays pinned to its worker.
Session durations and claim deadlines update once per second. Requests time out
after ten seconds. Output retention is bounded.

`hey-boss auto-workers` gives dedicated projects their own tabs, followed by one
**Shared** tab for all workers serving multiple projects (or all projects). Its
label shows the worker and unique-project counts. Shared agents and Chiefs stay
together, with project names on their rows and completed attempts under History.
Tab / Shift+Tab cycles through tabs; w lists only the selected tab's workers.
Pause and stop target the selected agent's worker; graceful removal targets the
worker selected in that list. Refreshes preserve the selected tab and session.

Tab order stays fixed when selecting a project. On narrow terminals the row shows
a contiguous portion of that order, with ‹ / › indicating hidden tabs. The selected
tab stays visible; long labels shorten at a complete Unicode character with an
ellipsis. Selection reserves the same space as inactive tabs, so labels do not shift.

| Key | Action |
| --- | --- |
| ↑/↓ or j/k | Select session |
| h | Switch active work / completed attempts (up to 20 from the CLI) |
| PgUp/PgDn | Scroll activity |
| r | Refresh or retry |
| p | Confirm pause; existing sessions finish normally |
| s | Confirm stop; worker sessions are stopped |
| ? | Help |
| Esc | Cancel confirmation or close help |
| q or Ctrl+C | Exit dashboard; workers keep running |

The dashboard uses the main app's blue and slate palette. At 100 columns and up,
activity appears beside sessions; narrower terminals stack the panes. Short
terminals prioritize sessions, while 64 × 18 still shows the latest update.
Activity retains twelve recent updates, newest first, with relative timestamps,
multiline messages, readable goal status, and a result section for finished
attempts. PgUp/PgDn scroll safely within the log; the bottom border shows the
position. Selecting another session returns to its latest activity.

Below 48 columns or 12 rows,
only quit keys work. Bracketed paste is ignored. Alternate-screen, cursor, and
raw-terminal state are restored on exit, errors, panic, SIGINT, and SIGTERM.
Queue text is stripped of terminal controls and bidi overrides.

## Library

`Dashboard` owns selection and display state; `ui::render` draws into a Ratatui
frame without performing IO. Applications can supply snapshots from their own
store, including in-process workers. `backend::Client` is the optional CLI
adapter, with cancellable, bounded requests. It never shells out through an
interpolated command string. Controls require a captured worker ID and an
explicit confirmation in the executable.

```rust,ignore
use hey_boss_worker_tui::{Dashboard, ui};

let mut dashboard = Dashboard::default();
dashboard.apply(worker_status_json);
terminal.draw(|frame| ui::render(frame, &dashboard))?;
```

Set `dashboard.now_ms` from the wall clock when embedding live timers. The
dashboard and renderer do not start a worker or change queue state.

## Verification

```sh
cargo test --locked --manifest-path worker-tui/Cargo.toml
cargo clippy --locked --manifest-path worker-tui/Cargo.toml --all-targets -- -D warnings
cargo build --locked --manifest-path worker-tui/Cargo.toml
cd worker-tui
npm ci
npm run test:terminal
npm run test:activity
cargo build --locked --manifest-path ../Cargo.toml
node tests/integrated-terminal.mjs
python3 ../tools/worker_title_terminal_checks.py ../target/debug/hey-boss
```

The terminal walkthrough uses the terminal-pilot SDK and a temporary synthetic
queue. It tests history, navigation, help during slow requests, resizing,
outage recovery, pasted keys, confirmation cancellation,
pause/stop, and restored terminal settings after quit and SIGTERM. It writes screenshots
under the ignored `worker-tui/target/terminal-qa` directory. Real queues and
workers are not changed. The activity walkthrough adds color screenshots across
five terminal sizes, paging, waiting/failure states, compact help, and disconnect
recovery under `worker-tui/target/activity-qa`; its fixtures use Node and shell.
CI checks the Rust package and PTY walkthrough on macOS
and Linux. Node.js 22+ and Python 3 are required only for the walkthrough.

`TERMINAL_PILOT_MODULE` and `TERMINAL_PNG_MODULE` can point to built local SDK
modules when testing from a poe-code development checkout.
