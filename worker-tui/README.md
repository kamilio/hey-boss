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
token so worker supervisors can restore the terminal before shutdown or reload.
When started by `hey-boss worker`, q / Ctrl+C stops that worker and its sessions.
With `worker status` or the standalone executable, quitting leaves workers running.

Workers appear on the left, with state and busy/total slots. The selected worker
lists project scope and its ID to distinguish workers with similar names. It
shows eligible issues, active sessions, optional recent attempts, and session
activity. Selection follows worker/session IDs through refreshes. Switching
workers hides the previous worker's sessions until fresh data arrives. Errors
retain the last successful snapshot, display an error, and retry every two
seconds. Deleted workers trigger an inventory refresh and selection recovery.
Session durations and claim deadlines update once per second. Requests time out
after ten seconds. Output retention is bounded.

| Key | Action |
| --- | --- |
| ↑/↓ or j/k | Select worker or session |
| Tab | Switch focus |
| h | Toggle completed attempts (up to 20 from the CLI) |
| PgUp/PgDn | Scroll activity |
| r | Refresh or retry |
| p | Confirm pause; running sessions drain |
| s | Confirm stop; worker sessions are stopped |
| ? | Help |
| Esc | Cancel confirmation or close help |
| q or Ctrl+C | Exit dashboard; workers keep running |

At narrow widths, workers stack above sessions. Below 48 columns or 12 rows,
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
dashboard and renderer do not start a supervisor or change queue state.

## Verification

```sh
cargo test --locked --manifest-path worker-tui/Cargo.toml
cargo clippy --locked --manifest-path worker-tui/Cargo.toml --all-targets -- -D warnings
cargo build --locked --manifest-path worker-tui/Cargo.toml
cd worker-tui
npm ci
npm run test:terminal
cargo build --locked --manifest-path ../Cargo.toml
node tests/integrated-terminal.mjs
```

The terminal walkthrough uses the terminal-pilot SDK and a temporary synthetic
queue. It tests history, navigation, help during slow requests, resizing,
deleted workers, outage recovery, pasted keys, confirmation cancellation,
pause/stop, and restored terminal settings after quit and SIGTERM. It writes screenshots
under the ignored `worker-tui/target/terminal-qa` directory. Real queues and
workers are not changed. CI checks the Rust package and PTY walkthrough on macOS
and Linux. Node.js 22+ and Python 3 are required only for the walkthrough.

`TERMINAL_PILOT_MODULE` and `TERMINAL_PNG_MODULE` can point to built local SDK
modules when testing from a poe-code development checkout.
