# Worker terminal dashboard

A live dashboard for hey-boss workers, built with Ratatui and Crossterm. Queue
requests run on a background thread; keyboard input and rendering stay responsive
during slow local or SSH requests. Only changed terminal cells are written.

```sh
hey-boss worker watch                   # Watch workers without starting one
hey-boss worker --host devbox watch      # Watch an SSH host
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
Quitting a dashboard leaves workers running.

Only the current worker appears. `worker watch`
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

`hey-boss workers watch` gives dedicated projects their own tabs, followed by one
**Shared** tab for all workers serving multiple projects (or all projects). Its
label shows the worker and unique-project counts. Shared agents stay
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
Runtimes use seconds, minutes, hours, or days: `45s`, `1m 04s`, `11h 09m`, `1d 11h`.
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

From `worker-tui`, run:

```sh
cargo test --locked
cargo test --locked --test terminal resume:: -- --ignored
cargo clippy --locked --all-targets -- -D warnings
```

The first command exercises controls, project tabs, activity, timers, reconnects,
resizing, and terminal restoration against a synthetic queue. The second builds
the main CLI and runs a private supervisor with a fixture agent. It checks that
opening the dashboard leaves pickup paused and that resuming pickup lets the
same agent finish its work.

Both use Poe Code’s Rust PTY and PNG crates. Find the captured screens and text
in `target/{terminal,activity,runtime,project-tabs,tab-order,resume}-qa`.
CI runs these on macOS and Linux, plus a separate Python check for terminal titles.
