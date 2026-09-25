# hey-boss

Licensed under [MIT](LICENSE).

The Rust [agent runtime](docs/agent-runtime.md) controls owned Codex, Claude Code,
and Pi sessions through one API, including resume, activity, steering, interruption,
explicit approvals/input, and provider-neutral goal continuation. Issue workers
continue using Codex; worker/task agent selection is deferred.

Run `hey-boss auto-workers run` to start or resume saved workers, then
`hey-boss auto-workers watch` to open their dashboard.
Tab / Shift+Tab switches project tabs; `a` adds a worker, and `w` then `d` removes
one after its current agents and Chief finish. Reopening the dashboard reuses the
same workers. Separate checkouts and shared multi-project pools retain their own
slots. See [per-machine configuration and live controls](docs/auto-workers.md).

The [worker terminal dashboard](worker-tui/README.md) shows the current worker, live sessions,
activity, queue state, and worker controls. Run `hey-boss worker run` to start a worker
with its dashboard, or `hey-boss worker watch` to watch existing workers.
Use `hey-boss worker status` (or `list`) for one text snapshot; add `--json` for JSON.
Bare `worker` and `auto-workers` commands show help and do not start work.
The older `worker --id ID` restore form remains accepted so running workers can reload during upgrades.
For a live log in scripts or agent sessions, use `hey-boss worker watch`.
It observes every registered worker on the selected machine every two seconds,
showing slots, pickup state, active issues, latest activity and session IDs.
It never starts or controls workers. Stop with Ctrl-C, or use
`hey-boss worker --json watch --count 3` for three newline-delimited JSON snapshots.
Each JSON record contains `observed_at` (Unix milliseconds), the worker inventory,
and per-worker `snapshots` with queue counts, active runs and recent events.
Use `--id ID` to watch one worker, `--history 5` to include up to five finished
attempts per worker (default: active only), or `--host HOST` for an SSH companion.
Disconnected hosts fail explicitly; they never fall back to this Mac's queue.
It is included in normal installations and SSH companion upgrades. The dashboard
opens automatically in an interactive terminal; use `--json` for scripts. Quitting a started worker stops its
sessions; quitting a watch dashboard leaves workers running.

Native macOS notifications and questions for coding agents. Short updates stack by project; Read update opens a Markdown preview. Rust library and CLI, Swift/AppKit daemon, SQLite history. No Python or Electron.

Notification commands (`alert`, `update`, `ask`, `prompt`, and `approval`) infer the
project from the Git repository or current directory, just like issue commands.
`--title` is required; `--project` optionally selects a full project ID or an
unambiguous short name from the same database shown by `hey-boss issue projects`.
New custom project names are registered there too. Worker sessions inherit
`HEY_BOSS_ISSUE_PROJECT` unless `--project` overrides it.

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

Create project outlines with `hey-boss mm add 'Release' --id release`, reference work
with `mm issue NUMBER --under release`, and explain dependencies with
`mm link FROM TO --kind depends-on --description 'Why it depends'`.
`mm web` opens a read-only branching map with pan/zoom, collapsible topics, a
clickable overview and an Outline view. Maps support cross-project references,
live issues, automatic issue→PR links and pending-only notifications.
See [mindmap commands](docs/mindmaps.md) and [tool research](docs/mindmaps-research.md).

Open the issue interface with `hey-boss issue web`, then visit
[127.0.0.1:4781](http://127.0.0.1:4781/). It shares the CLI's SQLite database and
includes a project switcher, Markdown editing, comments, claims, search, history,
and light/dark themes. All web assets are embedded in the binary; no frontend
build or separate service is needed. See the [web interface guide](docs/issues.md#web-interface)
and [verification results](docs/issues-web-verification.md).

Use the same interface on your phone through private Tailscale HTTPS access,
with `hey-boss issue web --mobile-origin https://mac.tailnet.ts.net:8443`.
The database stays on its authoritative machine; no mobile replica or cloud hub
is created. See [mobile issue setup](docs/issues.md#mobile-access-without-a-database-copy).

Projects appear automatically from running agents and first CLI use, ordered by
recent activity. Hide projects you no longer want in the switcher, then restore
them from **Hidden projects** at any time; their issues and history remain intact.

Codex workers launch from the CLI: `hey-boss worker run --concurrency 2 --tag ready`.
Each worker owns its concurrency and filters, uses its current directory, and
shows only that worker, with prominent availability and busy/free slot counts.
Active work is the default; h switches to a separate completed-attempt history.
`--history 20` starts in history. In plain terminal output, finished attempts
appear separately only when `--history` is supplied. Concurrency limits active
sessions only. Open, unassigned
issues retry after unsuccessful attempts with a delay of 30 seconds to five
minutes. Codex command, network, file-change and permission approvals appear as
issue-linked questions in Hey Boss Inbox. The worker keeps its session and claim
while you decide; approving or declining continues that same session. Command
and file approvals apply once, and permission grants last only for the current
turn. Cancel or dismiss stops the attempt on a manual retry hold. Pending
questions are cancelled when the request is resolved or the worker stops.
Unsupported input requests or an unavailable Inbox still save the session on
a manual retry hold; no action is automatically approved.
The dashboard identifies selected projects and connectivity to the supervisor.
**Supervisor → Worker → Agent:** the supervisor coordinates the fleet; each
worker picks issues, manages its Codex agents, and handles retries. An agent is
one coding session. A companion synchronizes a machine and applies worker controls.

Run `hey-boss fleet setup --source /path/to/hey-boss` once on the supervisor machine
to manage the existing SSH machine inventory automatically. The supervisor
installs companions, mirrors issue queues, distributes saved worker configurations,
and redeploys changed source builds. Open **Agents** (`/agents`) for a project
view of each task and the device where it runs. Open a task for its live Codex
conversation: the original request, saved replies, and expandable tool activity.
Conversations open at the latest activity; load earlier messages above it.
Choose **Steer** in a live conversation to add an instruction while the agent keeps
working. **This agent** sends a message to that session; **This issue** also appends
the requirement to the saved issue; **This project** also appends it to the project’s
base instructions. Other running agents using those project instructions receive
the update; worker overrides keep their own instructions. Messages are queued on
the owning device and delivery confirmation appears in the conversation. Retries
reuse the same request, so a connection interruption cannot duplicate a saved
requirement. Completed and standalone saved conversations cannot be steered.
Sending dismisses the instruction dialog immediately, so you can keep reading.
If submission fails or remains unconfirmed for 15 seconds, the dialog returns
with your text and scope preserved for a safe retry.

Choose **Take over** in a live conversation to stop that agent and assign its issue
to Boss. After it stops, **Copy command** gives you a terminal command to resume
the saved session, including SSH and the checkout directory for remote devices.
The same action is available on paired devices. Other agents keep working.
Live updates preserve your reading position. Completed conversations remain under each project's history;
disconnected tasks show their last known state. **Manage devices** keeps
pause/resume/stop/restart controls out of the task overview. `/workers` remains
available as an alias. Paired devices can read the same conversations through
the authenticated supervisor bridge; hidden projects stay inaccessible.
Fleet database access uses the CLI bundled SQLite across all machines. Companions
keep local replicas and continue allocated work offline; their
transaction journals sync on reconnect. Allocations do not expire when a machine
disconnects, preventing another machine from starting the same task. Concurrent
same-field edits are retained as conflicts rather than overwriting work.
`hey-boss fleet status` shows the fleet without opening a browser.

Use `hey-boss issue allocation NUMBER --json` to inspect a fleet reservation
without claiming, syncing, or changing workers. Claim denials preserve exit 4
and distinguish `fleet_reserved` from `fleet_allocation_missing`; their structured
`details` include the reserved machine, last known hostname, caller/store machine,
replica role, connectivity and safe recovery guidance. A missing companion
allocation may be stale: it never proves the supervisor has no reservation.
`issue view NUMBER --json` includes the same allocation diagnostics. Remote
inspection uses the initiating caller's machine, rather than the remote store's
machine, just as normal claim protection does.
Noninteractive `issue view` preserves configured actor identities but skips live
session discovery. Without a configured identity it uses the terminal caller;
claims and edits still require the normal verified agent identity.
Issue details show the reserved device, with reservation timing explained on
hover or keyboard focus. Boss can use **Release reservation** on the supervisor
to let another device pick up an unassigned issue. Stop any active worker attempt
first. The confirmed device and issue revision must still match; releases are
recorded in activity and replicated automatically. Reservations do not expire
automatically, including while a device is offline.

To resume a released manual claim, keep its saved `--agent ID`. For missing local
allocation, run `hey-boss issue allocation NUMBER --host SUPERVISOR` with the
supervisor's SSH host. If that authoritative store has no reservation or reserves
your machine, run `hey-boss issue claim NUMBER --host SUPERVISOR --agent SAVED_ID`.
The supervisor reserves previously unallocated work for the caller's machine in
the same transaction as ownership. Companions synchronize automatically while
connected; there is no one-shot sync command. Wait until local allocation shows
your machine before offline work. For another device's reservation, resume on
that device or ask Boss for a handoff. Do not change worker controls to discover
allocation. `--force` is an explicit takeover override; never use it to resolve
a synchronization or allocation denial. Ordinary claims protect an offline
device's reservation.
The supervisor and companion run natively in Rust, including replication,
allocation, worker controls, and the mobile bridge. Restarting either fleet
service leaves independently running workers and agents alive. Python remains
required for the source upgrade and GitHub import utilities; the old fleet
implementation is retained only as a regression-test reference.
Issue database writes are coordinated by one owner inside an existing local
service. CLI commands, web requests, workers and replication connect to it
automatically; no database server setup is needed. Reads remain available while
another transaction writes, and the service starts automatically when absent.
Standalone workers still use their local queue. `worker run --host HOST --directory
/remote/checkout` runs a worker and its Codex agents on that host.
**Project settings** in the web app edits shared instructions and conditional worktree/PR prompts,
with a live assembled preview. Implementation instructions contain only the shared
prompt and selected workspace and delivery branches; no extra cleanup or PR handoff
paragraphs are appended. Each branch inherits a code default or uses a project override. `worker run --worktree` selects a dedicated worktree only when the project allows it; otherwise workers use the existing checkout. Tags can be assigned directly in issue sidebars.
See [automatic workers](docs/issues.md#automatic-codex-workers).
For validation queue diagnosis and the single-admission workflow, see
[nested verification reservations](docs/verification-slots.md).
Worktree prompts can use `{{worktree_name}}` and `{{worktree_path}}`. The default selects a sibling directory and matching branch named `<project>-<title-slug>-<number>` (title slug capped at 15 characters), and tells agents to reuse them on restart.

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
hey-boss notif alert --project poe2 --title Ready 'Ready for review.' --issue 123
hey-boss notif inbox
hey-boss notif inbox --json
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
hey-boss notif alert --project Atlas --title 'Migration ready' 'The migration is ready for your review.' --autoclose 10
hey-boss notif update --project Atlas --title Report 'Review ready' '# Findings'
hey-boss notif ask --project Atlas --title Format 'Which format?' '' --option PDF --option Markdown --async
hey-boss notif wait '<task_id>'
hey-boss notif hide '<task_id>'
```

The installer builds both executables and registers a launch agent. macOS starts the daemon at login so its menu-bar item is available, and can also start it when a command connects. Keep `hey-boss.state` beside the installed CLI; it records the chosen state directory. Re-run the installer to upgrade. Companion connections and overview require protocol-version metadata from the current installer and a matching daemon handshake; older installations report an upgrade error before new commands are sent. Update the server companion too so remote stale-socket cleanup is available. `cargo install` alone does not install the daemon.

Notifications, questions, secret requests, and the Inbox are grouped under `hey-boss notif`.
Run `hey-boss notif --help` to see them. Existing root forms such as `hey-boss alert`
remain accepted as hidden compatibility aliases.

Use `hb` as a shortcut for any `hey-boss` command, such as `hb issue list` or
`hb notif alert --title Ready 'Build passed'`. Cargo and Homebrew install both commands;
source and companion installers and `hey-boss upgrade` create an adjacent `hb`
symlink when that name is available. An existing `hb` command is preserved.
The shortcut uses the same configuration, output, and exit status as `hey-boss`.

`hey-boss agent` groups `list`, `overview`, `configure`, and `control`. The old
root names remain accepted as hidden compatibility aliases.

Source and remote companion installation also allow `hey-boss` globally in Codex
and Claude Code. Homebrew queues this step for first-run daemon setup because its
post-install hook is sandboxed away from user configs. To apply it immediately,
or configure an existing installation, run:

```sh
hey-boss agent configure
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
correct it and rerun `agent configure`. To remove these permissions, delete the
managed `hey-boss.rules` file and remove only the hey-boss entries from Claude's
`permissions.allow` array.

Mac banners disappear after 12 seconds (alerts) or 20 seconds (updates), pausing while you read them. They stay unread and can be reopened from the menu-bar Inbox. Questions remain open; explicit `--autoclose` keeps its completion behavior. Three or more notifications form a collapsible project stack. The project × dismisses the stack. Read update/Open dismisses its card and keeps history. Questions support `--sync` and `--async`. Run `hey-boss --help` for concise usage guidance.

The pinned **Close all** button dismisses notification cards across all projects and cancels active and queued questions. History and already-open readers are kept. Waiting clients (`ask --sync` or `wait`) receive `status: cancelled` without a result; cancellation is not an answer or approval. Items arriving after the click remain available. CLI responses, including cancellation, exit successfully when the request was handled; inspect `status` to distinguish `ok`, `pending`, and `cancelled`.

Click a notification’s title or body to open it; click a collapsed project summary to expand it. Reading, answering, dismissing, and closing reviews update the native UI immediately. Relay requests run off the database queue, and failed actions restore only the affected notices and answer drafts. Close all uses bounded bulk requests and preserves concurrent arrivals. Answers already submitted on another device remain authoritative. Documents show a native loading indicator until the complete selectable document is ready, without replacing an intermediate preview.
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
hey-boss notif alert --project Atlas --title 'Migration ready' 'The migration is ready for your review.' --severity success --icon review
hey-boss notif update --project Atlas --title Report --icon docs 'Review ready' '# Findings'
hey-boss notif alert --project Atlas --title Release --icon-file ./brand.png 'Release published' --severity success
```

Rust callers can configure `Client::new(socket_path).with_appearance(hey_boss::Appearance { severity: Some(hey_boss::Severity::Success), icon: Some("build".into()), icon_path: None })`. Existing `Notification`, `Update`, and `Question` struct literals remain compatible. Appearance applies only to creation requests; all fields are optional on the wire (`severity`, `icon`, `icon_path`).

The agent skill reserves notifications for major outcomes or essential decisions from long-running background work that need attention. Synchronous conversation stays in chat; routine QA, builds, publishing, and individual agent milestones do not generate notifications. Messages are brief and consolidated by the coordinating agent.

Updates accept inline Markdown or a file snapshot:

```sh
hey-boss notif update --project Atlas --title 'Migration ready' 'The migration is ready for your review.' --file report.md
```

`--file` (also `--markdown-file`) accepts UTF-8 files up to 1 MiB, reads them before posting, and stores their contents in history. The same command works on the server: durable queue/replay transports Markdown text, so deleting or editing the original file does not change an already posted update. Relative server files and images are not transferred.

The reader uses CommonMark/GFM parsing and native AppKit text layout, adapting poe-code's structured Markdown renderer. It supports headings, nested and task lists, tables, footnotes, highlighted code, links, images with absolute web URLs, and GitHub alert callouts; frontmatter is hidden and raw HTML is displayed literally. One text view keeps selection continuous across blocks. Copy includes plain and rich text, preserves code whitespace, and exports selected table cells as tab-separated text. Parsing and attributed-text construction run in the background with a bounded cache; long code wraps without changing copied text. The renderer accepts up to 2 MiB and retains the complete plain source if rendering fails. See [reader details](docs/markdown-reader.md).

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

## Chief

Enable a project's **Chief** in Project settings or with `hey-boss worker run --chief`.
It is disabled by default. While a worker monitors that project, Chief runs one
organizing pass each hour, outside issue concurrency. Its separate prompt covers
issues, PR readiness, and mindmap maintenance; workers handle code changes.
Chief finishes each pass without a goal and resumes the same saved Codex thread,
including after worker restarts. A missing thread starts a replacement; ordinary
failures retain the conversation and retry with exponential backoff from 30 seconds
to five minutes. A successful pass restores the hourly schedule.

`hey-boss issue settings set --chief --chief-prompt 'Review issues and PRs, then stop.'`
sets the project prompt. Use `--no-chief` to disable it; an active pass is stopped.
The fleet supervisor selects one worker for each project's Chief across all
machines. It retains that assignment across reconnects. Before moving ownership,
it revokes the old assignment and waits for that machine to acknowledge it and
report that its Chief has stopped. A disconnected machine keeps ownership until
it reconnects; another Chief is not started while the old one may still be running.
Standalone workers sharing a database share one Chief reservation per project.
Stopping a worker stops its owned Chief too. Chief startup failures and thread
crashes are recorded through the supervising worker, which retains results until
they are saved. Worker reloads recover abandoned passes without losing the saved
conversation. Chief uses no issue slots and has no pass deadline.

## URL lookup

Issue-store reads, store opening, and transaction acquisition retry transient
SQLite contention within a six-second window (an in-progress SQLite wait may
take up to two additional seconds). Exhaustion returns `database_busy`, including
in JSON output, with retry guidance. Mutations are never replayed automatically;
guarded edits check `--if-version` after acquiring the writer lock and still
reject a newer revision. Read the latest revision before retrying a guarded edit.

Read an item from a copied web link with `hey-boss lookup 'URL'`. For example:

```sh
hey-boss lookup 'http://127.0.0.1:4781/#project=github.com%2Fpoe-platform%2Fpoe-code&view=issues&issue=114'
hey-boss lookup 'https://hey-boss-mobile-kamil.fly.dev/artifacts#project=named%3AAtlas&artifact=a-ID' --json
```

Lookup shares the web's resource route definitions for issues, Inbox notices,
mindmap topics (including issue/PR/notice references), artifacts and agent
conversations. Links to lists return their collection; issue list filters are
preserved. `--json` includes the resolved `route` and the usual resource result,
including document comments and links. Reads do not mark notices as read or
change resources. Conversation reads return the existing bounded first page.
The URL's explicit project wins over worker defaults; `--project` supplies context
for links without a project. Issue resources honor `--host`, then the fragment's
`host`, then `HEY_BOSS_ISSUE_HOST`. The URL origin is not contacted; the command
reads your configured store or authenticated SSH backend. Inbox reads use the
connected desktop, and conversation `host` identifies the owning fleet device.

Issues created from Codex automatically capture the creating model from that
session's saved turn metadata or exact thread record, when available. No CLI
flags are needed. Lists, details, and the Origin card show the model in place of
the creator's session hash; the full session and conversation link remain in
Origin. The captured model stays unchanged after edits, retries, or later model
switches. Older issues and unavailable metadata keep their existing attribution;
configuration defaults are never treated as evidence of the model used.

Every web page includes a source comment and a `hidden` guide for agents, with a
shell-quoted `hey-boss lookup 'URL' --json` command that follows URL navigation.
The paired Inbox root uses `hey-boss notif inbox --json`. `/llms.txt` publishes the same
Markdown guide on desktop and paired web installations. This is a discovery
convention, not a guarantee that a browser agent reads hidden content. The guide
is not a copy of resource data: lookup uses the existing CLI readers and JSON
fields, so new fields need no parallel Markdown renderer. URL fragments select
resources in these pages and are not sent to an HTTP `.md` endpoint. User-written
resource bodies remain data, not agent instructions.

## Blocking dependencies

Waiting for unfinished subtasks is shown as **Blocked**. Issue lists and details
show the linked issues preventing pickup. Add blockers in the issue sidebar, or
from the CLI:

```sh
hey-boss issue block 12 --by 8 --by 9 --comment 'Needs both fixes'
hey-boss issue blocked-by 12 8 9 --if-version 4
hey-boss issue list --state blocked
hey-boss issue blocked-by 12  # Remove explicit blocker links
```

In PR-enabled projects, `hey-boss issue ready NUMBER` marks an attached delivery PR
Ready for Boss. Workers also mark Ready when completing PR delivery. The worker
decides readiness; Hey Boss does not independently verify CI. Both explicit
dependencies and subtask sequencing unblock at **Ready** or **Closed**, enabling
stacked PRs before merge. Reopen a Ready task before rework: new dependent pickups
pause, while running agents keep their claims and receive a dependency update.

Dependencies must belong to the same project and cannot form cycles. Closing or
deleting all blockers reopens the dependent issue automatically; reopening or
restoring a blocker blocks it again. Subtasks remain blocking until every reachable
unfinished descendant reaches the project’s dependency completion state. A manual block without linked issues requires
explicit reopening. Pending agent approval requests also appear as Blocked.

Agent crashes, startup failures, incomplete completion reports and service outages
are failed attempts. Open tasks retry automatically after 30 seconds, one minute,
two minutes, four minutes, then five minutes; there is no attempt limit. The
retry deadline survives worker restarts. Once the owned process stops, its claim
and slot are released, so other tasks can proceed during the delay. The dashboard
shows scheduled retries and retains the failure reason in conversation history.
Successful delivery requires a completed agent turn and an authoritative issue
closure or attached fix PR. Posting text alone cannot mark a failed run successful.

The saved Codex session, checkout and issue history remain available. Before
repeating a database write with an unknown outcome, the resumed agent must read
current state or reuse its original deduplication request ID. Worker results are
saved locally before finalization; reconnection reconciles the atomic transaction,
so a lost commit reply cannot duplicate a handoff. Explicit approval decisions,
manual blocks and human ownership remain respected. Retrying grants no permissions.

## Upgrading every machine

Boss can add **YOLO** in **Assign tags** in the issue web UI. The
amber YOLO label marks an issue whose next worker attempt runs without a sandbox
or approval prompts, including a resumed Codex session. Selecting the tag enables
it directly; it stays enabled until Boss removes the tag. Changes affect
the next attempt, not an agent already running. Ordinary CLI labels, agent edits,
and batch triage cannot grant or change YOLO. Removing the tag restores the normal
workspace sandbox and Auto approval review on the next attempt. This permission
control does not reopen, claim, or retry an issue automatically.

For simultaneous `http://hey-boss.test/` and `http://127.0.0.1:4781/` access,
see [local HTTP setup](docs/local-http.md). macOS uses a loopback-only launchd
socket while the application continues running as your normal user.

Run `hey-boss upgrade` on the Mac to update its CLI, desktop app, canonical agent
skills, and every host in `~/.local/share/hey-boss/companion-hosts`. By default it
fetches the latest upstream `main` and archives that commit. For development,
`hey-boss upgrade --source /path/to/hey-boss` installs a snapshot of that checkout,
including uncommitted changes, and remembers the checkout location. Later upgrades
without `--source` fetch and archive committed `main` from that location; dirty
work is only installed when `--source` is explicitly supplied. A clean main checkout
is recorded as committed main; other explicit snapshots are recorded as development.
Automatic supervisor rollouts watch fetched/published `origin/main` (local `main`
for a checkout without an origin). Unpushed local commits do not request a rollout.
They leave an explicit development installation in place until published main
advances beyond its base commit (or a normal upgrade is requested). Deployment
reports must verify the requested published build and source receipt; a different
release is retried with the normal failure backoff without interrupting sync.
An already matching companion connection stays open after a no-change upgrade.

Each machine queues installers behind its installation lock, then checks source
ancestry against its last verified installation. Older or unrelated commits are
refused, including with `--force`. Development snapshots also require the observed
installation generation to remain unchanged while queued; retry explicitly to
stage a fresh snapshot after an intervening upgrade. Previously unrecorded
installations acquire provenance on their first successful upgrade. Receipts in
`~/.local/share/hey-boss/upgrade-receipt.json` record commit, repository, source kind,
build ID, generation and installation time. A fleet report includes the installation
before the upgrade, the verified result, and a final audit of each host. If another
rollout intervenes before that audit, it reports `superseded` and exits unsuccessfully
instead of silently claiming the originally verified build is still installed.

`hey-boss upgrade --check` reports current, outdated, and unreachable machines
without installing. `--local-only` limits the operation to this machine; repeat
`--host HOST` to override the registered targets. `--force` reinstalls a matching
build. `--json` emits a machine report. Exit codes are 0 for success, 1 for failures,
and 2 when a check finds an outdated machine.

The source snapshot has a build ID shown by `hey-boss --version`, so matching
machines skip compilation even when package versions are unchanged. Each host
needs Rust, Git, tar, and a working unattended SSH connection. Desktop upgrades
also need the Xcode command-line tools. Builds use a persistent cache, replacements
are staged beside the installed executable, and failed verification restores the
previous binary. Before replacing the CLI or restarting services, the staged
build opens this installation's issue store and commits required schema changes
atomically. Startup also reconciles missing draft, plan, project planning, and
mindmap label columns without overwriting existing values. A failed migration
keeps the installed CLI and services in place and reports the store and cause;
resolve that cause and retry rather than deleting the store. Committed schema
changes are retained if a later installation step fails, so recovery must use a
build that supports the migrated schema. The existing state, issue databases, and running workers are
preserved. The desktop daemon and managed companion brokers restart after an
upgrade. Updated workers keep picking up issues and load the new CLI when naturally
idle, retaining the same ID and settings. Emergency draining is explicit: create
an empty `issues.db.drain-for-update` file beside the machine's issue database
before or after replacing the CLI. Only workers awaiting a CLI update stop pickup;
active agents finish normally. The dashboard labels this **Emergency update drain**.
Remove the file to resume pickup or after the emergency update; it is not removed
automatically and applies to later updates while present. With `HEY_BOSS_ISSUE_DB`,
append `.drain-for-update` to that database path. Each machine has its own marker.
Workers running an older build still use their previous update behavior until
their first reload. Workers started before this
handoff feature need a one-time restart. Unreachable hosts do not prevent the other machines from updating;
rerun the command after reconnecting to retry them.

If worker startup reports `no such column: i.draft`, the issue database is
missing draft columns expected by that build. This is a schema mismatch, so
restarting alone is not a reliable fix. Run `hey-boss upgrade` on the supervisor
to update it and registered companions, then restart any workers that predate
automatic upgrade handoff. Current builds repair missing columns before worker
pickup, even when the database already reports the latest schema version;
issues and their history are preserved.

## Move GitHub issues into a project

`hey-boss issue drain-github` imports issues from the checkout's GitHub repository
and deletes each original only after verifying the saved copy and checking for
source changes. It preserves Markdown, labels, comments, source metadata, and
closed state. Python 3 and an authenticated GitHub CLI (`gh`) are required.

```sh
hey-boss issue drain-github
hey-boss issue drain-github --repo owner/repo --author octocat --project target
```

The creator filter defaults to the authenticated GitHub user. Use `--all-authors`
to include everyone or `--state all` to include closed issues. Retries reuse the
same destination copy. See [GitHub drain](docs/github-drain.md) for verification,
failure handling, and the remaining race with GitHub's unconditional deletion.

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

Run `hey-boss agent overview` on the Mac, or choose **Agent overview** from the new
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
`hey-boss agent list` and `hey-boss agent list --json` for a local read-only snapshot.
`hey-boss agent overview --json` reads the running window's current state through its
existing socket, including local and received server snapshots. Its `rows` reflect
the current search. It does not open the window, change selection,
refresh discovery, or start a network connection. Connected server companions can
forward this command through the existing tunnel; disconnected controls return an
error immediately and are never queued.

Discovery runs once when the overview opens, or when you choose **Refresh agents**
while it is open. There is no periodic refresh on the Mac or server broker.
Reading `agent overview --json` returns the cached view without launching scanners or SSH.
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

## Harvester

Run `hey-harvester` for the terminal dashboard. **Tab** and **Shift-Tab** switch
between local and remote machines from Hey Boss's existing SSH inventory.
Use **1–5** for Overview, Processes, Worktrees, Caches and Activity; **↑/↓** scroll
and **q** quits. Status refreshes every five seconds without blocking navigation.
The menu-bar **Harvester (Terminal)…** entry launches the same CLI.

**s** scans, **c** cleans eligible candidates after confirmation, **a** toggles
automatic maintenance, and **p/w/b/l** toggle process/worktree/cache/log cleanup.
In Worktrees, select a row and press **x** to request removal; **y** confirms.
This bypasses age only: local files, live processes, locks, primary checkouts and
detached unmerged commits remain protected, and named branches are retained.
The Processes page shows current-user PIDs, CPU usage, resident memory and executable
names; appearing in this inventory never authorizes termination. Activity shows
scan phases, cleanup decisions, preservation reasons and errors.

Every action targets the selected machine. Switching tabs leaves an in-flight
operation on its original machine. Failed SSH connections show an error and never
fall back to local cleanup. SSH reuses the companion connection where available.

```sh
hey-harvester                       # terminal dashboard
hey-harvester --host devbox          # select a remote tab
hey-harvester status --json          # metrics and latest counters
hey-harvester logs --limit 50        # recent activity
hey-harvester scan                   # inspect only
hey-harvester clean                  # clean eligible candidates
hey-harvester enable                 # schedule at login and every five minutes
hey-harvester disable
hey-harvester add-root /path/to/workspaces
hey-harvester remove-root /path/to/workspaces
hey-harvester configure --worktrees false
hey-harvester configure --caches false
hey-harvester configure --aggressive true  # destructive 24-hour expiration; SQLite retained
hey-harvester remove-worktree /absolute/path/to/checkout
hey-harvester hosts --json
hey-harvester --host devbox scan
hey-harvester --host devbox enable
```

`hey-harvester` is an independent Rust package in `packages/hey-harvester`, alongside
`hey-gh`. It reads the same `~/.hey-boss/config.json` machine inventory,
`~/.local/share/hey-boss/connections.json` connections, and
`~/.local/share/hey-boss/health` configuration/history as Hey Boss.
`HEY_BOSS_HEALTH_DIR` still overrides the maintenance directory. Existing settings,
observations, locks and history are preserved. `hey-boss health` remains a compatibility
command; `hey-boss health open` now opens the terminal dashboard.

Build with `cargo build --locked --release -p hey-harvester`, then run
`target/release/hey-harvester install`, or use `tools/install-harvester.sh local`.
Use `tools/install-harvester.sh HOST` for each SSH client. This builds only the
harvester, installs `~/.local/bin/hey-harvester` atomically under the maintenance
lock, and migrates an already enabled schedule without restarting Hey Boss services.
Add `~/.local/bin` to your shell's PATH. The host needs Rust, a C compiler and Git
(and lsof on macOS); Linux inspects procfs directly. Builds reuse
`~/.cache/hey-harvester/build` with two compiler jobs. Set
`HEY_BOSS_SSH_CONTROL_PATH` to reuse an SSH master during installation.
`tools/install-health-worker.sh` remains an alias for this installer.

`hey-harvester watch` runs scheduled maintenance in the foreground.
The existing macOS `local.hey-boss.health` LaunchAgent and Linux `hey-boss-health`
systemd timer names are retained to prevent duplicate schedules; they now invoke
`hey-harvester run`. Linux user timers require a user manager (login or lingering).
Disabled automatic maintenance stays disabled during installation.
`tools/machine-health.sh` runs a one-shot cleanup using the harvester release build.

With `configure --aggressive true`, linked worktrees expire after 24 hours without
source or Git activity, including locked, dirty, ignored and detached checkouts.
Worktrees used by a running Codex process are retained.
Primary checkouts stay intact. HEADs are pinned under `refs/cleanup/worktrees/`
before removal; uncommitted files are discarded. Discovery includes nested Codex
slots, configured workspaces, `/private/tmp`, `/tmp` and `/Users/Shared`.

Aggressive cache cleanup expires individual files after 24 hours across OS temp,
Chrome signing copies and caches, npm/Bun/Yarn/Python caches, `~/.cache`, and
project dependency/build/output directories. New siblings do not protect old
files. Directory discovery and file traversal save cursors between bounded runs,
so large caches make forward progress. Worker diagnostic log rotation is enabled
with this policy.

Aggressive process cleanup stops orphan developer runtimes after an hour, expired
developer workloads after a day, and automated browsers after an hour
(ten minutes under memory pressure). Memory pressure also expires older test and
developer workers; critical pressure can reclaim old Chrome renderers. Descendants
are stopped with TERM then KILL, rechecking identity before every signal. The
harvester ancestry, OS processes, Hables and Hey Boss database/control services
remain protected. Ordinary GUI browser roots are retained.

Codex, its ancestors/children and its live worktrees are excluded from forced
cleanup. An interactive CLI can receive one Ctrl-D through its iTerm2/tmux controller only after its open transcript
proves an hour of idle state and five minutes of unchanged process-tree CPU.
Working, app-server, ambiguous and recently active sessions are retained. The
harvester saves `codex-resume-<session>.json` in its health directory before
requesting exit. It verifies the terminal foreground process group and never uses
OS signals on Codex. Unmanaged terminals are retained. This lets
Codex perform its normal shutdown and print the resume command.

**All policies preserve SQLite files and WAL/SHM/journal sidecars**, including
database headers hidden behind ordinary filenames and all Hables paths. Cleanup
never recursively deletes a parent around a protected database. Such a worktree
can remain registered with only database files; Activity reports partial cleanup.
A process-inventory failure is reported but no longer prevents disk cleanup.
Status shows the selected policy, phase, last check and duration; `disable` stops
the scheduler before acquiring the maintenance lock.

The default conservative policy targets known Wrangler/Miniflare test browsers, workerd test
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
Process checks continue at the configured interval while the worktree scan runs,
so a large Git inventory cannot postpone harvesting for the entire disk scan.
The maintenance lock remains held until both finish; process observations and
cumulative cleanup counts are checkpointed after each check.
**In conservative mode, Codex, Claude, unknown processes, normal services, and
personal browsers are never automatic targets.** A disconnected or idle agent is not proof of abandonment.

The conservative worktree cleaner discovers repositories directly inside `~/Workspace` and
`~/.codex/worktrees` (or `$CODEX_HOME/worktrees`), plus configured roots. It only
removes linked checkouts that are merged or at least 7 days old with a verified
named branch retaining their commits. A merged checkout must have been inactive
for at least an hour. All removals require no active process, agent or
open file, no modified/untracked/ignored files, no locks, in-progress Git
operations or populated submodules. Merge status uses the locally recorded remote
default branch; detached unmerged commits are preserved. It rechecks eligibility before file-by-file removal and keeps branches. Missing registrations and primary checkouts are preserved.
Ignored files such as `.env` or build directories also prevent automatic removal.
Empty, uninitialized submodule directories do not block cleanup; submodule
contents and symlinked submodule paths remain protected.

The **Caches** tab lists known disposable candidates and preservation reasons.
Cache inspection runs before the slower Git inventory. Quiet observations allow
the actual duration of the previous completed check as well as the timer interval,
so large inventories do not continually reset the cleanup grace period.
Cleanup covers Chrome/Chrome Beta cache subdirectories, npm/pip/uv download
caches, macOS Chrome signing copies, Miniflare's randomly named `miniflare-<32 hex digits>`
directories, and stale hey-boss health test fixtures in the OS user temp directory.
Signing copies and Miniflare temp state must be inactive for an hour; other
candidates for a day. It checks ownership, recent changes throughout each tree,
open files, and stable identities across repeated observations, then rechecks
before removal. Symlinked roots, incomplete inspections and oversized trees are
preserved; symlinks inside caches are never followed.
The recognized user-owned Chrome signing-copy container may include its copied
root-owned executable; all directories must still belong to the current user.
Creating another hard link to a shared signing-copy file does not reset its age
or quiet observation; directory activity and file identity, mode, size and
modification time remain checked.
Personal browser profiles, history, cookies, bookmarks, offline storage, persistent
project `.wrangler` state and arbitrary project artifacts are not cleanup targets.
Disposable browser profiles inside abandoned Miniflare temp directories are included.
Use the cache checkbox or `configure --caches false` to disable.
The footer and CLI report measured **net free-space change** on the home volume.
This includes concurrent writes and shared APFS blocks, rather than summing
directory sizes that can substantially overstate reclaimed space.
Worker-log trimming is disabled by default; enable it separately with
`hey-harvester configure --logs true`. It trims `fleet-worker-<id>.log` diagnostic files above
128 MiB, retaining the latest 64 MiB in the same inode so existing append-only
workers keep running. Issue history, session transcripts and databases are
preserved. As with copy-truncate log rotation, concurrent diagnostic output can
race trimming; durable issue progress remains unchanged.

Scheduling is opt-in. The macOS job runs at standard priority with `nice 10`, not
launchd's background tier: that tier is throttled whenever builds or tests run, which
made process and worktree inspection exceed their timeouts on a busy Mac, so the
leaked browsers were never harvested. Re-running `hey-harvester enable` refreshes a stale
registration. Settings and the latest bounded runtime state live in
`~/.local/share/hey-boss/health` (`HEY_BOSS_HEALTH_DIR` overrides it). There are no
report files, notification posts, unbounded logs, or telemetry. The state contains
quiet-observation timestamps needed for safe cleanup, the current scan phase,
and a bounded activity history for the native log browser. Inspection failures
preserve the affected resources. On Linux, non-dumpable PAM, SSH transport and
credential daemons are kept running and excluded from checkout-use inspection;
their child jobs are inspected separately. Unknown protected processes and SFTP
jobs prevent cleanup when their open files cannot be checked.

## Secret input

`hey-boss notif secret --field API_KEY --env-file .env` opens masked native input and
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

## Pull request purposes

`hey-boss issue view NUMBER` shows the complete issue body, labels and creation
context, the latest status, PR URLs, linked artifact titles, and the latest 20
comments with the total comment count. Recent comments read in chronological order.
Large bodies may reduce the page size; the continuation offset always reflects
the comments actually returned.

Read comments separately with `hey-boss issue comments NUMBER --limit 20
--offset 20 --sort newest`. Use `--sort oldest` to read from the beginning;
sorting uses the comment ID to keep ties deterministic. Both sorts support
`--json` and report the total count and next offset. `issue history` remains
the audit trail for edits and lifecycle events.

Read a linked document with `hey-boss artifact view ID`, export its exact saved
Markdown to stdout with `hey-boss artifact export ID`, or materialize a new
private file with `hey-boss artifact export ID --output notes.md`. Existing
files are never overwritten. Use the same `--project` and `--host` as the issue
when reading across projects or devices.

Attached PRs have a purpose: `unspecified` (the default for older links), `fix`,
`prerequisite`, or `supporting-evidence`. Attach with
`hey-boss issue pr add NUMBER URL --purpose fix`, or change an existing link with
`hey-boss issue pr classify NUMBER URL --purpose supporting-evidence`.
Classification preserves the attachment author and date. Issue details, lists,
and `issue pr list` JSON expose `purpose`. Review every PR; use this metadata to
distinguish merge requirements from supporting material. It never automatically
closes an issue or assigns it to Boss.

## Issue subtasks

Create a child from the parent's Subtasks card or add an existing issue. Each
child keeps its own Markdown, labels, assignee, PRs and lifecycle. Parent links
and completion progress appear in the list; unlinking preserves the issue.
By default, sibling subtasks run sequentially in queue order, including nested work. Later
subtasks stay Blocked until earlier work is Ready (PR projects) or Closed; the parent follows its descendants.
Claims and worker prompts identify the parent, sequence position, and previous/next
subtasks so agents can read requirements and prior handoffs.

Choose **Explicit dependencies** in project settings, or run
`hey-boss issue settings set --subtask-scheduling explicit`, to keep grouping
while scheduling only declared blocker links. Parent completion still follows
its descendants. [Dependency notice guards](docs/dependency-notices.md) keep
older running clients from issuing obsolete sibling rework instructions.

Subtasks are scheduling dependencies: unfinished descendants put a parent in
**Blocked**. Creating, linking, or unlinking subtasks is rejected atomically if
reconciliation would release an existing claim, including an ancestor's claim.
This also applies to the claim owner. For follow-up organization while work is
claimed, use ownership-preserving mindmap nesting:

```sh
hey-boss mm issue 12 --id parent-work
hey-boss mm issue 15 --under parent-work
```

Mindmap nesting leaves issue state, assignees, and scheduling unchanged. To add a
scheduling dependency instead, have the owner explicitly release the claim first.

```sh
hey-boss issue subtask create 12 --title 'Implement the API' --body '## Requirements'
hey-boss issue subtask add 12 15
hey-boss issue subtask list 12
hey-boss issue subtask remove 12 15
```

Nested relationships, queue sorting and offline sync are covered in
[the issue guide](docs/issues.md#subtasks).

### Remote worker restart

Select worker checkouts with `hey-boss worker -C ~/Workspace/atlas -C ~/Workspace/beacon`.
Each repository contributes its project and runs agents in its own saved checkout.
`--cwd` and the existing `--directory` are aliases for `-C`; repeat `--project`
to restrict the selected projects. With `--host`, paths belong to that machine.
See [worker scope and restart behavior](docs/issues.md#automatic-codex-workers).

Run `hey-boss worker --host HOST restart WORKER_ID` on the fleet supervisor machine, or use Restart worker under **Manage workers** on `/agents`. Workers are grouped by device and show their projects, working directory, agent slots, and running state. A selected project limits this list to workers that can pick up its tasks; controls still affect the entire worker. Stopped workers are collapsed separately. Pause pickup lets current agents finish; restarting or stopping a worker stops its current agents. Enabled workers remain running in the background after their terminal closes. For a local worker use `hey-boss worker restart WORKER_ID`. These controls require `hey-boss fleet setup`. Check `hey-boss fleet status` for the durable signal acknowledgment; queued does not mean restarted.

The supervisor and companion remain alive. The companion stops the old worker and its owned Codex sessions, then starts a replacement with the same ID and saved settings. It acknowledges only after the replacement registers. A cross-process lock prevents reconciliation from launching a duplicate; interrupted requests replay safely and failures retry with a delay capped at five minutes. A new Stop request supersedes an unfinished restart. Restart explicitly cancels active sessions; Pause drains them. If the old worker or its sessions cannot stop, the companion reports the error and does not launch a duplicate.

Worker and browser reads use WAL snapshots; migration writes run only when needed. A temporary SQLite lock during worker status refresh retries without shutting down Codex sessions. Browser discovery uses its own connection. Git identity checks avoid enumerating all worktrees for ordinary checkouts. The manual claim window starts at the first model activity, with a separate fifteen-minute wait for model startup, so a queued model does not consume the claim deadline.

Issue lists and details show each issue's worker agent launch count. Hover or focus the muted web count for its explanation; CLI `issue list` and `issue view` include it, and JSON exposes `agent_launch_count`. Each actual process launch counts once, including retries and resumed sessions; reservations and failed process starts do not count. Launch history persists through reopening, deletion/restoration, project moves, and fleet sync. This count imposes no retry limit. Existing recorded launches are backfilled during upgrade.

In the issue list, the arrow beside an assigned agent opens that session's conversation in Agents, including its owning device and latest recorded attempt for the issue. The assignee badge still filters the list. Assignments without recent recorded history show an availability explanation.

Use **Move to draft** in an open or blocked issue's Readiness section, or
`hey-boss issue edit NUMBER --draft`, to refine its scope with agent pickup paused.
Drafting a blocked issue clears its manual block and keeps linked blockers and
subtasks. Marking it ready checks those dependencies again; unfinished work returns
it to Blocked. Drafting requires project drafts to be enabled and no assignment or
active worker reservation.

Quick add issues with **⌘⇧K** (Mac) or **Ctrl+Shift+K** from any web page,
including over an open editor. On Mac, **⌘⌃⌥⇧I** opens a native Spotlight-style
panel from any application without opening the browser; the menu bar also has
**Quick add issue…**. Enter a title
and press Enter to create it; Escape preserves the draft for the next opening.
The header's plus button works on touch screens.

Type `@` anywhere in the title to select a known project. Browse suggestions
with ↑/↓, then Enter, Tab, or a click selects a
project without creating the issue. Escape dismisses suggestions first; press it
again to close quick add. The native panel remembers the last successfully used
project; choose one with **@project** for the first issue. **⌘⇧B** toggles add to
bottom; issues otherwise go to the top. Closing preserves an unsent draft, and
failed submissions retain their request ID for safe retries. See
[native Quick Add](docs/native-quick-issue.md) for details.
Suggestions also match project IDs and safely quote
names with spaces or use full IDs for ambiguous names. Mentions such as
`@poe-code` resolve
without case sensitivity; use `@"Design Team"` for spaces, or a full project ID
such as `@github.com/kamilio/poe-code` when names are ambiguous. Mentions are removed
from the submitted title. Repeated mentions must identify the same project.
Unknown projects and conflicting mentions show an error; email addresses stay
unchanged, and `\@poe-code` keeps literal `@poe-code` text in the title.

Persistent project Markdown documents, revision-checked editing, comments and issue/mindmap links are available through **Artifacts** in the web menu and `hey-boss artifact`. See [Project artifacts](docs/artifacts.md).

Issues, mindmap nodes and artifacts also support disk-backed files, drag-and-drop upload and remote CLI downloads through `hey-boss attachment`. See [File attachments](docs/attachments.md).

### PR status and automatic completion

The fleet supervisor checks attached GitHub PRs every minute using the embedded
[hey-gh package](packages/hey-gh). It reads only PR metadata, deduplicates shared
URLs, and uses conditional requests with rate-limit backoff. Merged PRs retain
their final status without further polling; closed PRs are checked for reopening.
The PR list and task index show status icons, with delayed updates marked stale.

**Settings → Close tasks when their fix PRs merge** is enabled by default. A task
closes only after every PR classified as **Fix** is confirmed merged. Unspecified,
prerequisite, and supporting-evidence links never trigger completion. Closing
adds a history comment and releases dependent tasks through the normal workflow.
Disable automatic completion with `hey-boss settings set --auto-close-merged-prs false`;
status polling continues. The supervisor uses its existing `gh auth login`.

The standalone `hey-gh` CLI remains available. Source and companion upgrades
install it beside `hey-boss` and update an existing `~/.cargo/bin/hey-gh`. To
install it alone, run `cargo install --path packages/hey-gh --locked`.
