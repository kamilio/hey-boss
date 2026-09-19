# hey-boss

Licensed under [MIT](LICENSE).

Native macOS notifications and questions for coding agents. Short updates stack by project; Read update opens a Markdown preview. Rust library and CLI, Swift/AppKit daemon, SQLite history. No Python or Electron.

Project issues for agents are stored in SQLite, including Markdown bodies,
comments, session ownership, and change history. The issue CLI works on macOS
and Linux without the notification daemon:

```sh
hey-boss issue create --title 'Fix reconnect' --body 'Reconnect after waking from sleep.'
hey-boss issue list
hey-boss issue claim 1
hey-boss issue comment 1 --body 'Reproduced; working on a fix.'
hey-boss issue close 1 --comment 'Fixed in commit abc123.'
```

Projects default to the Git repository (including its worktrees), or the current
directory. `whoami` shows your session identity; `unassign` returns unfinished
work to the pool. Use `--body -` for Markdown on stdin, `--json` for automation,
and `--agent ID` if session detection is unavailable. See [issue commands and
storage](docs/issues.md) for recovery, filtering, retry IDs, and a shared SSH host.

Open the issue interface with `hey-boss issue web`, then visit
[127.0.0.1:4781](http://127.0.0.1:4781/). It shares the CLI's SQLite database and
includes a project switcher, Markdown editing, comments, claims, search, history,
and light/dark themes. All web assets are embedded in the binary; no frontend
build or separate service is needed. See the [web interface guide](docs/issues.md#web-interface)
and [verification results](docs/issues-web-verification.md).

Projects appear automatically from running agents and first CLI use, ordered by
recent activity. Hide projects you no longer want in the switcher, then restore
them from **Hidden projects** at any time; their issues and history remain intact.

Codex workers launch from the CLI: `hey-boss worker --concurrency 2 --tag ready`.
Each worker owns its concurrency and filters, uses its current directory, and
shows slots, pickup stages, active Codex sessions, and three recent attempts in
separate terminal sections. `--history 0` hides finished attempts; `--history 20`
shows more history. Concurrency limits active sessions only. Open, unassigned
issues retry after unsuccessful attempts with a delay of 30 seconds to five
minutes; approval requests remain on hold until explicitly retried.
The terminal identifies the queue host, database, and selected projects.
Run `hey-boss fleet setup --source /path/to/hey-boss` once on the main machine
to manage the existing SSH machine inventory automatically. The controller
installs agents, mirrors issue queues, distributes saved worker configurations,
and redeploys changed source builds. Open `/workers` in the Issues web app for
live connections, capacity, tasks, sync conflicts, and pause/resume/stop/restart.
Fleet database access uses the CLI bundled SQLite across all machines. Agents
keep local replicas and continue allocated work offline; their
transaction journals sync on reconnect. Allocations do not expire when a machine
disconnects, preventing another machine from starting the same task. Concurrent
same-field edits are retained as conflicts rather than overwriting work.
`hey-boss fleet status` shows the fleet without opening a browser.
Standalone workers still use their local queue. `worker --host HOST --directory
/remote/checkout` runs a supervisor and its Codex sessions on that host.
**Instructions** in the web app edits the project prompt, its exact preview, and
PR behavior. Tags can be assigned directly in issue sidebars.
See [automatic workers](docs/issues.md#automatic-codex-workers).

The macOS menu-bar menu includes **Issues…**, which opens the interface in your
default browser and starts its local service when needed. **Inbox…** opens a
shared web Inbox for unread notifications, questions, reviews, and past activity.
Search and filter notices by project, answer questions, and review documents there.
The native Inbox window has been removed; banners and document previews remain.
Closing the page preserves unread items. Floating notification panels can be moved by their notification heading
and hidden with the × button; their project-level close button dismisses the
whole group, including document reviews.

Notices can optionally link to an issue, with links visible in both directions:

```sh
hey-boss alert --project poe2 --title Ready 'Ready for review.' --issue 123
hey-boss inbox
hey-boss inbox --json
```

`--issue` infers the current repository’s issue project. Use `--issue-project FULL_ID`
and optionally `--issue-host devbox` to target another project or machine. Existing
notices can be linked, changed, or unlinked in the web Inbox. Linking only adds a
relationship: it never changes issue state, assignment, content, or notice completion.
Ordinary updates become read when opened; questions and reviews stay pending until
answered, finished, or explicitly cancelled. Creation links use the existing durable
notification queue, including while the desktop is disconnected.
See [Inbox behavior and verification](docs/inbox.md).

Install with Homebrew:

```sh
brew install kamilio/tap/hey-boss
```

Requires macOS 26+ and Xcode command-line tools. Homebrew installs Rust as a build dependency and compiles locally; no unsigned installer download is executed. The first notification or question registers the daemon automatically. Use `brew services restart hey-boss` to restart it, or `brew services stop hey-boss` before `brew uninstall hey-boss`. History is preserved.

For installation directly from source, install Rust and Xcode command-line tools. Run from a logged-in desktop session:

```sh
export HEY_BOSS_STATE_DIR="$HOME/Library/Application Support/hey-boss"
export HEY_BOSS_BIN_DIR="$HOME/bin"
export HEY_BOSS_LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
swift install_hey_boss.swift
export PATH="$HEY_BOSS_BIN_DIR:$PATH"
hey-boss alert --project Atlas --title 'Migration ready' 'The migration is ready for your review.' --autoclose 10
hey-boss update --project Atlas --title Report 'Review ready' '# Findings'
hey-boss ask --project Atlas --title Format 'Which format?' '' --option PDF --option Markdown --async
hey-boss wait '<task_id>'
hey-boss hide '<task_id>'
```

The installer builds both executables and registers a launch agent. macOS starts the daemon at login so its menu-bar item is available, and can also start it when a command connects. Keep `hey-boss.state` beside the installed CLI; it records the chosen state directory. Re-run the installer to upgrade. Companion connections and overview require protocol-version metadata from the current installer and a matching daemon handshake; older installations report an upgrade error before new commands are sent. Update the server companion too so remote stale-socket cleanup is available. `cargo install` alone does not install the daemon.

Source and remote companion installation also allow `hey-boss` globally in Codex
and Claude Code. Homebrew queues this step for first-run daemon setup because its
post-install hook is sandboxed away from user configs. To apply it immediately,
or configure an existing installation, run:

```sh
hey-boss configure-agents
```

This writes command-specific `allow` rules to `~/.codex/rules/hey-boss.rules` and
merges `Bash(hey-boss *)` plus the installed executable's absolute path into
`permissions.allow` in `~/.claude/settings.json`. `CODEX_HOME` and
`CLAUDE_CONFIG_DIR` override those directories. Use repeatable `--binary /path/to/hey-boss`
to allow stable symlink paths; Homebrew's first-run setup uses its stable public and
`opt` paths automatically. Restart Codex to load the rules. Existing deny/ask rules,
managed policies, and Claude's sandbox restrictions still apply.

The updater preserves unrelated settings and formatting, recognizes existing
permission entries, and does not touch `config.toml` or `default.rules`. It validates
both files before changing either, refuses malformed settings or custom edits in
its managed rules file, preserves existing symlinks and file permissions, and makes
a private, uniquely named `.hey-boss-*.bak` copy before replacing a changed file.
Writes use atomic replacement, with installer locks and checks for concurrent edits.
Re-running without changes creates no backups. An error reports the affected path;
correct it and rerun `configure-agents`. To remove these permissions, delete the
managed `hey-boss.rules` file and remove only the hey-boss entries from Claude's
`permissions.allow` array.

Mac banners disappear after 12 seconds (alerts) or 20 seconds (updates), pausing while you read them. They stay unread and can be reopened from the menu-bar Inbox. Questions remain open; explicit `--autoclose` keeps its completion behavior. Three or more notifications form a collapsible project stack. The project × dismisses the stack. Read update/Open dismisses its card and keeps history. Questions support `--sync` and `--async`. Run `hey-boss --help` for concise usage guidance.

The pinned **Close all** button dismisses notification cards across all projects and cancels active and queued questions. History and already-open readers are kept. Waiting clients (`ask --sync` or `wait`) receive `status: cancelled` without a result; cancellation is not an answer or approval. Items arriving after the click remain available. CLI responses, including cancellation, exit successfully when the request was handled; inspect `status` to distinguish `ok`, `pending`, and `cancelled`.
`hide` also cancels a question, whether it is queued on the server or already
displayed on the Mac. Repeating it preserves the stored terminal status and answer.

Creation commands (`alert`, `update`, `ask`, `prompt`, and `approval`) accept optional appearance flags:

- `--severity neutral|info|success|warning|error` adds a subtle status badge color. The default is neutral. Info marks useful context, success marks a completed outcome, warning means something needs attention, and error means an actual failure.
- `--icon NAME` chooses a cohesive native SF Symbol. Aliases: `info`, `success`, `warning`, `error`, `build`, `code`, `test`, `review`, `deploy`, `docs`, `folder`, `bell`, `question`. Any SF Symbol name is also accepted; unknown or unavailable symbols fall back to the default.
- `--icon-file PATH` uses a local PNG, JPEG, TIFF, ICNS, or PDF up to 4 MiB instead of a symbol. The CLI resolves a readable regular file to an absolute path before contacting the daemon. Custom icons keep their full colors, with severity shown separately. `--icon` and `--icon-file` are mutually exclusive.

Custom images are snapshotted into history at 128 × 128 pixels, preserving their aspect ratio. After enqueue succeeds, the source file can be moved or deleted. Files that become unreadable or cannot be decoded safely fall back to the default icon. Icons are loaded locally; no remote images are fetched.

| Alias | SF Symbol |
| --- | --- |
| `info` | `info.circle.fill` |
| `success` | `checkmark.circle.fill` |
| `warning` | `exclamationmark.triangle.fill` |
| `error` | `xmark.octagon.fill` |
| `build` | `hammer.fill` |
| `code` | `chevron.left.forwardslash.chevron.right` |
| `test` | `checklist` |
| `review` | `text.magnifyingglass` |
| `deploy` | `shippingbox.fill` |
| `docs` | `doc.text.fill` |
| `folder` | `folder.fill` |
| `bell` | `bell.fill` |
| `question` | `questionmark.bubble.fill` |

```sh
hey-boss alert --project Atlas --title 'Migration ready' 'The migration is ready for your review.' --severity success --icon review
hey-boss update --project Atlas --title Report --icon docs 'Review ready' '# Findings'
hey-boss alert --project Atlas --title Release --icon-file ./brand.png 'Release published' --severity success
```

Rust callers can configure `Client::new(socket_path).with_appearance(hey_boss::Appearance { severity: Some(hey_boss::Severity::Success), icon: Some("build".into()), icon_path: None })`. Existing `Notification`, `Update`, and `Question` struct literals remain compatible. Appearance applies only to creation requests; all fields are optional on the wire (`severity`, `icon`, `icon_path`).

The agent skill reserves notifications for major outcomes or essential decisions from long-running background work that need attention. Synchronous conversation stays in chat; routine QA, builds, publishing, and individual agent milestones do not generate notifications. Messages are brief and consolidated by the coordinating agent.

Updates accept inline Markdown or a file snapshot:

```sh
hey-boss update --project Atlas --title 'Migration ready' 'The migration is ready for your review.' --file report.md
```

`--file` (also `--markdown-file`) accepts UTF-8 files up to 1 MiB, reads them before posting, and stores their contents in history. The same command works on the server: durable queue/replay transports Markdown text, so deleting or editing the original file does not change an already posted update. Relative server files and images are not transferred.

The reader uses CommonMark/GFM parsing with our own HTML/CSS presentation in native WebKit. It supports headings, nested and task lists, tables, footnotes, code, links, images with absolute web URLs, and GitHub alert callouts; frontmatter is hidden and raw HTML is displayed literally. Long code and tables scroll within the document. A basic native reader remains available if rendering fails; documents above the renderer's 1 MiB bound use that fallback. See [reader verification](docs/markdown-reader.md).

Severity also tints the card background and border, with a stronger leading accent: blue for information, green for success, orange for warning, and red for error. Neutral notifications retain the regular glass appearance. Collapsed project stacks use the highest severity; expanded cards keep their individual colors. Tints adapt to light and dark appearance, and severity icons/text remain available alongside color.

Notifications display a muted source icon and “This Mac” or the server hostname, including within project stacks and launch details. Older clients or historical records without source metadata display “Source unavailable”; upgrade both the Mac CLI and server broker to identify new messages reliably.

To install the agent skill: `mkdir -p ~/.codex/skills && cp -R skills/hey-boss ~/.codex/skills/`.

Build and check: `cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`; then `mkdir -p out && xcrun swiftc -O -parse-as-library -D HEY_BOSS_AUDIT hey_boss_daemon.swift test_hey_boss.swift -o out/hey-boss-test && out/hey-boss-test`. Rust consumers use `hey_boss::Client::new(socket_path)`; `cargo doc --open` lists the API.

History and launch metadata (working directory, Git branch, process ancestry) stay in the chosen state directory. There is no telemetry. Links open in your associated applications. Review notification contents before sharing history; use synthetic data in bug reports.

To uninstall, run `launchctl bootout "gui/$(id -u)" "$HEY_BOSS_LAUNCH_AGENTS_DIR/local.hey-boss.plist"`, then remove that plist, the installed CLI, its adjacent `hey-boss.state`, and the daemon executable. Keep `history.db` if you want your archive.

Maintainers: run `swift release_hey_boss.swift 0.1.0 kamilio/hey-boss`. Upload the generated source archive and `SHA256SUMS` to the matching GitHub release, then copy `out/hey-boss.rb` to `homebrew-tap/Formula/hey-boss.rb`. The manual Release artifacts workflow produces the same files without publishing.

## Global profile

The web profile badge opens a dropdown with **Settings**. Set your display name
there once for all projects; project settings contain only project instructions
and PR behavior. The stable assignee remains `human:boss`. The CLI equivalents are
`hey-boss settings show` and `hey-boss settings set --boss-name 'Alex'`; add
`--host HOST` to address a remote issue store. Settings are stored in SQLite,
with version checks and request IDs for safe retries.

## Upgrading every machine

Run `hey-boss upgrade` on the Mac to update its CLI, desktop app, canonical agent
skills, and every host in `~/.local/share/hey-boss/companion-hosts`. By default it
fetches the latest upstream `main` into a managed checkout. For development,
`hey-boss upgrade --source /path/to/hey-boss` installs a snapshot of that checkout,
including uncommitted changes, and remembers the path for later upgrades. A
remembered checkout is used as-is; pull it before upgrading to consume upstream changes.

`hey-boss upgrade --check` reports current, outdated, and unreachable machines
without installing. `--local-only` limits the operation to this machine; repeat
`--host HOST` to override the registered targets. `--force` reinstalls a matching
build. `--json` emits a machine report. Exit codes are 0 for success, 1 for failures,
and 2 when a check finds an outdated machine.

The source snapshot has a build ID shown by `hey-boss --version`, so matching
machines skip compilation even when package versions are unchanged. Each host
needs Python 3, Rust, and a working unattended SSH connection. Desktop upgrades
also need the Xcode command-line tools. Builds use a persistent cache, replacements
are staged beside the installed executable, and failed verification restores the
previous binary. The existing state, issue databases, and running workers are
preserved. The desktop daemon and managed companion brokers restart after an
upgrade. Updated workers drain their active sessions and then restart with the
same ID and settings before picking up more issues. Workers started before this
handoff feature need a one-time restart. Unreachable hosts do not prevent the other machines from updating;
rerun the command after reconnecting to retry them.

## Remote server companion

After your usual VPN/SFT authentication (`d` connects to the SSH alias `devbox`),
run on the Mac from this source checkout:

```sh
cargo build --locked --release
./target/release/hey-boss companion install devbox --source .
hey-boss companion connect devbox
```

Use the newly built/installed CLI for `connect`; it needs the Mac installation's
adjacent `hey-boss.state`. The server requires Rust and Unix sockets; installation
builds its native CLI, installs the Codex skill, and starts a durable queue broker.
On Linux with user systemd, the broker is enabled as `hey-boss-companion.service`.
User services run while the user manager is active; enable lingering separately
if you need the service before login. Without user systemd, installation starts a
`nohup` broker; restart after reboot with
`~/.local/bin/hey-boss companion serve --state ~/.local/share/hey-boss`.
Add `~/.local/bin` to the server's PATH.

The server CLI talks to its private `daemon.sock`; SSH forwards `bridge.sock` to
the Mac's daemon. Keep `connect` open in a terminal and use Ctrl+C to disconnect.
No public network listener is opened. Only one Mac connection per server is
supported. Do not open concurrent connections to the same server.

Alerts, updates, and questions are durably saved before the server acknowledges
`pending`, even while disconnected. Connection replays the queue in creation
order. Server task IDs remain stable across reconnects and broker restarts.
`status`, `hide`, `wait`, and synchronous questions map to the corresponding Mac
task; observed answers and cancellations are cached on the server. Interrupting
`wait` does not cancel the question. Entries are retained in
`~/.local/share/hey-boss/queue`; there is currently no automatic retention limit.
Delivery is **at least once**: interruption after the Mac accepts an item but
before the server saves its acknowledgment can replay a duplicate. Server icons
use `--icon`; local files and file links cannot be transferred by this transport.

Edit **only** `skills/hey-boss/SKILL.md`. Its contents are embedded into the CLI at
build time. Rebuild/install the Mac CLI, then `hey-boss companion sync-skill`
updates the local Codex skill and every registered server. To add/update a
specific destination, use `hey-boss companion sync-skill HOST`. Installation and
connection register destinations and sync automatically. Existing agent sessions
may need to reload the skill. The registry is `~/.local/share/hey-boss/companion-hosts`.

## Website actions (no VS Code required)

The companion opens websites in your Mac’s default browser and returns a session
for HTTP requests to that website. Server loopback URLs automatically receive a
private loopback-only SSH forward over the existing connection.

```sh
hey-boss browser open http://127.0.0.1:4123/review/token
# Use the returned session:
hey-boss browser request SESSION --path /api/comments --method POST \
  --headers '{"Content-Type":"application/json"}' --body '{"text":"Looks good"}'
hey-boss browser close SESSION
hey-boss action capabilities
```

Git-shelf defaults to this opener; automatic opener selection no longer probes
VS Code. Explicit legacy VS Code modes remain available for compatibility.
See [the action protocol](docs/browser-actions.md) for envelopes, limits and
connection lifecycle.

## VPN-aware automatic connection

On the Mac, import the hey-proxy host inventory once and enable the login manager:

```sh
hey-boss companion setup
hey-boss companion status
```

Discovery and automatic notification delivery both use `~/.hey-boss/config.json`:

```json
{
  "ssh_hosts": [
    { "host": "devbox", "vpn_domain": "quora.net", "enabled": true },
    { "host": "kamils-macbook-pro.local", "enabled": true }
  ]
}
```

Strings are also accepted as host entries. `.local` hosts use the LAN; other hosts
use their configured VPN domain (default `quora.net`). The manager watches local
config edits every five seconds and independently adds, removes, or restarts the
changed connections. Invalid edits retain existing connections until corrected.
Each machine has its own tunnel, persisted retry state, and lock. Set `enabled`
to false or remove an entry to disconnect that machine. **Machines…** in the app
opens this config in VS Code when available. Initial import preserves the existing
VPN setting and copies only host names from hey-proxy.

The VPN domain must appear exactly in a local macOS DNS search-domain or resolver
entry. It does not start or modify the VPN.

While the domain is absent, **no SSH requests are sent**. Local DNS is inspected
once every 15 seconds; two consecutive positive samples are required. When VPN
connectivity disappears the SSH process group is stopped. Setup combines skill
sync and server readiness into one SSH session, then opens the persistent tunnel.
Failures retry after about 1, 2, 4, 8, and 15 minutes, with up to 15 seconds of
jitter. Cooldown is persisted across process restarts and VPN flaps; a connection
that stays healthy for one minute resets it. There is one attempt per machine in flight,
a 45-second setup deadline, and noninteractive SSH with an 8-second connection
timeout and no host-key prompts. The connector never invokes `sft login`; if
ScaleFT credentials expire, authenticate using your normal manual workflow.
Third-party SSH proxy commands retain their own authentication behavior.

`hey-boss companion status` shows state, Unix timestamps, failure count, and next
retry time. Logs are in `~/.local/share/hey-boss/autoconnect.log`; the launch agent
is `~/Library/LaunchAgents/local.hey-boss-companion.plist`. There are no repeated
notification cards for failed attempts. Manual and automatic tunnels share a
local lock to prevent competing connections. The compatibility command changes only the named machine:

```sh
hey-boss companion configure devbox --vpn-domain quora.net --disable
```

For comment-enabled reviews, `status ID --sync` returns as soon as a persisted
comment is available, without waiting for the reader to close. Existing comments
return immediately; the response includes the full comment list, exact selected
source lines, and the current review state. `status ID` / `--async` return immediately.
Use `wait ID` or `update --comments --sync` when you need the review to close.
Rust callers can use `Client::try_wait_for_feedback(ID)` for first-comment delivery.
These semantics also apply through the server companion and its offline cache.

## Agent overview

Run `hey-boss overview` on the Mac, or choose **Agent overview** from the new
menu-bar item. The native window lists Codex and Claude sessions/processes with
project, latest task, activity, state, and host. Search by task/project/host, filter
using agent names, open local projects, and copy session IDs. Press ⌘F to focus search and
replace the current query. Sessions always group by repository, combining normalized
Git origins across hosts and worktrees. Repository headers carry project identity;
agent, branch, state, and host appear as compact badges. Click an agent's disclosure
to expand its task, directory, worktree, session ID, PID, and discovery evidence
in place. There is no separate inspector. The header reports view rebuild and
scanner times; JSON includes the latest and p95 rebuild milliseconds. Unchanged
cards are reused and search is debounced to avoid rebuilding on every keystroke.

The CLI also provides
`hey-boss agents` and `hey-boss agents --json` for a local read-only snapshot.
`hey-boss overview --json` reads the running window's current state through its
existing socket, including local and received server snapshots. Its `rows` reflect
the current search. It does not open the window, change selection,
refresh discovery, or start a network connection. Connected server companions can
forward this command through the existing tunnel; disconnected controls return an
error immediately and are never queued.

Discovery runs once when the overview opens, or when you choose **Refresh agents**
while it is open. There is no periodic refresh on the Mac or server broker.
Reading `overview --json` returns the cached view without launching scanners or SSH.
Last-known snapshots are retained and marked stale when the view is rebuilt.

Additional machines are read from the shared `ssh_hosts` inventory in
`~/.hey-boss/config.json`. Disabled entries are omitted. Discovery still runs only
on open or manual refresh; the connection manager does not scan agents.
Install a compatible CLI on additional machines at `~/.local/bin/hey-boss-scanner`,
`~/.local/bin/hey-boss`, or a standard Homebrew location. Opening or manually
refreshing collects one snapshot per host over noninteractive SSH, with bounded
output, a five-second connection timeout, and a 25-second total timeout.
Unavailable hosts back off from one minute to fifteen minutes across subsequent
opens or manual refreshes; there is no automatic retry timer and no VPN/login command.
The menu-bar item is icon-only. The Dock and Cmd+Tab use hey-boss's purple
chat-bubble icon. Opening the overview adds hey-boss to the Dock
and Cmd+Tab; closing the overview returns it to status-bar-only operation.

Processes with no session ID or usable activity are omitted from the viewer.
Known sessions without a task remain visible. Search matches provider names,
tasks, titles, repositories, branches, directories, and hosts. The window title
shows the session count; there are no provider filter buttons or summary row.
The CLI's raw agent snapshot retains unattributed processes for diagnostics.
Codex's exact-session saved remote and branch identify deleted worktrees when live
Git inspection is unavailable; live Git always takes precedence, and expanded
discovery evidence labels saved metadata.

Codex chat titles are read by exact session ID from its read-only thread database,
preferring a user-supplied name when available. Claude's explicit transcript custom
titles are supported. Titles remain separate from the latest task and progress.
Unrelated historical threads are never added as live agents. Desktop Claude and
Codex app/MCP servers without observed sessions are excluded from the agent list.
Global configuration flags before an app-server command are recognized.

A live `codex resume ID` command can identify a requested resume target even when
its transcript is closed; the view marks that target as unverified live state.
Previously observed process/session links survive idle transcript closure, using
an OS process-start identity to reject PID reuse. A process never observed with a
transcript and without an exact resume target remains unattributed. Discovery
does not scan the large Codex log database or guess a thread from the newest file.

A process can have several loaded sessions, and a new empty session can have no
task yet. Helper processes are excluded. Codex uses legacy and modern rollout
lifecycle events; Claude uses PID-specific metadata when available, validated
against OS process start time. Task unavailable means discovery lacks evidence,
not that the agent is necessarily idle. This is a read-only overview; it does not
start, interrupt, steer, or attach to existing agent runtimes. Discovery details
and investigated open-source projects are in [docs/agent-discovery.md](docs/agent-discovery.md).

Local native UI evidence is indexed in [docs/screenshots.md](docs/screenshots.md);
private captures remain in ignored out/overview-screenshots.

Notification dismissal fades departing cards before smoothly repositioning the
survivors. Overlapping dismissals and new arrivals preserve the fading views until
they finish; Reduce Motion uses immediate removal.

For an additional LAN Mac, install the full companion with
`hey-boss companion install kamils-macbook-pro.local`, then run
`hey-boss companion auto --host kamils-macbook-pro.local --local-network`.
The LAN exception accepts only `.local` SSH destinations; the default connector
keeps its VPN gate. Host-specific locks and status files allow both connections
at once, with bounded retry backoff. Darwin installs a login broker LaunchAgent.

Repository groups start collapsed. Click anywhere on a repository header to toggle
it. Expanded repositories and inline agent details are saved locally and restored
when the viewer or app reopens. New repositories remain collapsed.

## iPhone Home Screen companion

The mobile companion provides a glass-style inbox, real Apple Web Push, approvals
and questions. Mac and phone share one authoritative outcome so only one answer
can be accepted. See [mobile setup and platform limits](docs/mobile.md).

Codex sessions on a managed app-server can be steered and their saved goals paused
or re-enabled from expanded Agents rows. See [Codex controls](docs/codex-steering.md)
for endpoint configuration and the limits of existing standalone sessions.

## Machine Health

Open **Machine Health…** from the menu bar, or run `hey-boss health open`.
The native window shows disk space, memory pressure, available memory and swap,
plus process and worktree cleanup controls. The default **Processes** tab lists
current-user processes on the selected machine, refreshed every five seconds and
sorted by resident memory (RSS). Rows include PID, age, CPU usage and executable;
RSS excludes swapped memory. This live inventory is separate from cleanup
candidates: appearing here never authorizes termination. Scans run off the UI thread. The
**Activity** tab browses the latest 1,000 timestamped events: scan phases, cleanup
decisions, preserved items and errors. Search the log and select an entry to read
or copy its message. The current phase refreshes while a scan runs.
The desktop daemon is installed as **Hey Boss.app**, with a bundled icon for the Dock and app switcher.
**Cmd+W** closes the window. Select text and use **Cmd+C** to copy it;
editable fields also support **Cmd+V**, **Cmd+X** and **Cmd+A**. With a table
row selected, **Cmd+C** copies the row. Refreshes preserve text selections.

The machine selector includes configured SSH clients and connected clients from
hey-boss's connection inventory. Metrics, logs, roots, settings and every cleanup
action belong to the selected machine. Remote calls use unattended SSH and reuse
the companion connection where available; failed connections show an error and
never fall back to local cleanup. Remote Macs and Linux hosts use the same backend.

The **Worktrees** tab shows each checkout's age, path, GitHub repository and
preservation reason. Select a row and click **Remove selected worktree** to remove
it immediately when safe. This bypasses the automatic age threshold, retaining
the named branch (including unmerged commits). Detached unmerged commits, local
files, open processes, locks and primary checkouts remain protected. **Open GitHub**
opens the repository. Age uses checkout creation time, falling back to modification
time where birth time is unavailable; automatic eligibility also checks the latest
Git and tracked-file activity.

```sh
hey-boss health enable          # checks at login and every five minutes
hey-boss health status          # current metrics and latest counters
hey-boss health logs --limit 50  # recent activity; --json for structured output
hey-boss health scan            # inspect only
hey-boss health clean           # clean verified candidates
hey-boss health disable
hey-boss health add-root /path/to/workspaces
hey-boss health remove-root /path/to/workspaces
hey-boss health configure --worktrees false
hey-boss health remove-worktree /absolute/path/to/checkout
hey-boss health hosts --json
hey-boss health --host devbox status
hey-boss health --host devbox scan
hey-boss health --host devbox enable
```

`tools/machine-health.sh` runs one cleanup cycle using the release build; pass
`enable`, `status`, or another health subcommand to use the same backend. Set
`HEY_BOSS_CLI_PATH` to select an installed binary. `health watch` runs the scheduler
in the foreground; macOS `enable` installs `local.hey-boss.health` in LaunchAgents.
Linux `enable` installs a user systemd service and timer (`hey-boss-health`); its
schedule runs independently of the Mac. User services require a running user
manager (and login or configured lingering).

Install/update the backend on each SSH client with
`tools/install-health-worker.sh HOST`. It builds and tests on that host, installs
`~/.local/bin/hey-boss-health` atomically under the maintenance lock, and leaves
companion services and agents running. The host needs Rust, a C compiler, Python 3,
Git (and lsof on macOS). Linux inspects procfs directly. To reuse an existing SSH master, set `HEY_BOSS_SSH_CONTROL_PATH`.
An updated remote `hey-boss` CLI also works when no dedicated worker is installed.
Use the UI's **Add workspace…** or `health --host HOST add-root /remote/path` for
repositories outside the default roots. Scheduling and cleanup settings are per host.

The harvester targets known Wrangler/Miniflare test browsers, workerd test
runtimes, disconnected discovery commands, and hey-boss HTTP test fixtures.
On Linux it also handles headless Selenium Chrome with temporary WebDriver
profiles, explicit `poe_proxy.unix_server --test-mode` fixtures and their
co-launched application servers, and temporary-venv `scripts/catbot.py` test
servers in the GraphQL test workspace. Generic Python/Node servers are protected.
Candidates must belong to the current user and have lost their parent. Test
browsers must be at least 10 minutes old; other processes must be at least one
hour old. These thresholds are `browser_min_age_seconds` and
`process_min_age_seconds` in the per-machine configuration. Cleanup requires
continuous quiet observations at least five minutes apart. On Linux, any I/O
counter change resets observation; test fixtures also require nearly unchanged
CPU time. Dead logging pipes are allowed only when no external process holds
them; incoming clients and external Unix socket peers prevent cleanup. Idle
upstream connections are allowed only for the explicitly identified test fixtures.
The harvester checks identity, activity and connections again before signalling
and escalates from TERM to KILL only for the verified processes. Linux uses pidfds
so a reused PID cannot redirect a signal to another process.
**Codex, Claude, unknown processes, normal services, and personal browsers are
never automatic targets.** A disconnected or idle agent is not proof of abandonment.

The worktree cleaner discovers repositories directly inside `~/Workspace` and
`~/.codex/worktrees` (or `$CODEX_HOME/worktrees`), plus configured roots. It only
removes linked checkouts at least 14 days old with no active process, agent or
open file, no modified/untracked/ignored files, no locks, in-progress Git
operations or populated submodules, and a HEAD merged into the locally recorded remote
default branch. It rechecks before `git worktree remove`, never uses `--force`,
and keeps branches. Missing registrations and primary checkouts are preserved.
Ignored files such as `.env` or build directories also prevent automatic removal.
Empty, uninitialized submodule directories do not block cleanup; submodule
contents and symlinked submodule paths remain protected.

Scheduling is opt-in. Settings and the latest bounded runtime state live in
`~/.local/share/hey-boss/health` (`HEY_BOSS_HEALTH_DIR` overrides it). There are no
report files, notification posts, unbounded logs, or telemetry. The state contains
quiet-observation timestamps needed for safe cleanup, the current scan phase,
and a bounded activity history for the native log browser. Inspection failures
preserve the affected resources. On Linux, non-dumpable PAM, SSH transport and
credential daemons are kept running and excluded from checkout-use inspection;
their child jobs are inspected separately. Unknown protected processes and SFTP
jobs prevent cleanup when their open files cannot be checked.

## Secret input

`hey-boss secret --field API_KEY --env-file .env` opens masked native input and
writes directly to a private file without returning values to the caller's output.
For a login/password pair, use `--field LOGIN --field PASSWORD --login`.
Use `--field API_KEY -- python3 app.py` to set the child's environment instead;
child output is suppressed. A destination is required. Secrets are never ordinary
questions and are not stored in Hey Boss history, mobile sync, or offline queues.
The same command works over a connected, updated SSH companion.

Long normal answers wrap and grow, then scroll. Secret fields support long pastes;
Show reveals the full wrapping editor locally. ⌘Return submits. See the
[skill's secret workflow](skills/hey-boss/SKILL.md#secrets-keep-values-out-of-agent-context)
for guarded file redirection, cancellation behavior, and environment use.

## Issue subtasks

Create a child from the parent's Subtasks card or add an existing issue. Each
child keeps its own Markdown, labels, assignee, PRs and lifecycle. Parent links
and completion progress appear in the list; unlinking preserves the issue.
Workers finish reachable open descendants before picking up their parent.

```sh
hey-boss issue subtask create 12 --title 'Implement the API' --body '## Requirements'
hey-boss issue subtask add 12 15
hey-boss issue subtask list 12
hey-boss issue subtask remove 12 15
```

Nested relationships, queue sorting and offline sync are covered in
[the issue guide](docs/issues.md#subtasks).

### Remote worker restart

Run `hey-boss worker --host HOST restart WORKER_ID` on the fleet controller machine, or use Restart on `/workers`. For a local worker use `hey-boss worker restart WORKER_ID`. These controls require `hey-boss fleet setup`. Check `hey-boss fleet status` for the durable signal acknowledgment; queued does not mean restarted.

The controller and companion remain alive. The agent stops the old worker and its owned Codex sessions, then starts a replacement with the same ID and saved settings. It acknowledges only after the replacement registers. A cross-process lock prevents reconciliation from launching a duplicate; interrupted requests replay safely and failures retry with a delay capped at five minutes. A new Stop request supersedes an unfinished restart. Restart explicitly cancels active sessions; Pause drains them. If the old worker or its sessions cannot stop, the agent reports the error and does not launch a duplicate.

Worker and browser reads use WAL snapshots; migration writes run only when needed. A temporary SQLite lock during worker status refresh retries without shutting down Codex sessions. Browser discovery uses its own connection. Git identity checks avoid enumerating all worktrees for ordinary checkouts. The manual claim window starts at the first model activity, with a separate fifteen-minute wait for model startup, so a queued model does not consume the claim deadline.
