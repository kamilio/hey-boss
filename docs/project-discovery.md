# Project discovery

Run `hey-boss project init` inside a checkout to choose pull requests and
worktrees, preview the current Markdown prompts, and save the project in Builder.
Enter keeps existing choices; `q` or Ctrl-C cancels before saving. Rerunning
preserves custom prompts and checks for concurrent settings changes.
For scripts, pass `--yes --prs true --worktree true` (use `false` for either
choice); `--project` selects a name or full ID and `--json` returns settings.

Only `hey-boss project init` registers new projects. Cancelling initialization,
reading project data, sending notifications, and observing running agents never
create registry entries. Existing registry rows remain usable.

Commands resolve the current Git repository (including subdirectories and linked
worktrees) or the nearest registered local parent directory. Unrelated folders
with the same basename do not select each other's project. When no registered
project matches, the command fails: use `--project <name-or-id>` to select an
existing project, or run `hey-boss project init` in the project directory.

Builder lists initialized projects and projects with saved issues, artifacts,
mindmap nodes, or worker settings. Empty legacy discoveries stay out of the
picker. Their identities and all saved data are retained. Hidden projects stay hidden.

Project names are unique identifiers (case insensitive). Explicit selectors can
use an existing name or compatibility storage ID. Quick Issue submissions use
existing names. Git origins and local paths remain compatibility storage IDs.
Worker dashboards, terminal titles and plain-text status show project names;
JSON retains the storage keys for compatibility.

Chief runs an organizing pass outside issue concurrency and resumes its saved
conversation about an hour after the pass finishes. It has no elapsed-time limit.
Its owning worker shows Chief in Active agents, with the session, process and
latest activity. Other workers on the same machine do not start or display a
second Chief. Ownership stays with that worker between passes; if it is stopped,
another worker can resume the saved conversation.

The web Agents page keeps Chief above Active agents, with its owning worker and
device, running or waiting status, minutes until the next pass, and the last
pass's result and time. Its saved conversation is readable on desktop and paired
devices, including earlier passes in the same thread. Chief does not count toward
issue agent slots and has no issue takeover or steering controls.

The registry rejects duplicate names even when another process writes directly.
Name reuse is silent: it creates neither another project nor a warning record.
Upgrades discard old warnings about aliases that never became projects. Paired
devices also deduplicate their inventory and accept names as issue destinations.

On upgrade, existing duplicate names choose one stable destination, preferring
saved undeleted issues, then artifacts and mindmap data, then the oldest entry.
The picker shows that destination once. Other legacy rows and all their saved
data remain accessible through their full IDs. The CLI and registry response
retain legacy history warnings; web pages show only the selected destination.
They are not silently merged, renumbered or deleted. Use the project name for new
work. Hidden destinations stay hidden when another identity is discovered.

Automatic agent discovery only refreshes activity for registered projects. It
ignores home directories and local temporary folders,
including macOS per-user temporary directories. Live agent tests previously
registered folders such as `hey-boss-live-controls-*` as permanent projects,
which filled the project picker with test runs. Empty discovered entries
are omitted from project lists on desktop and paired devices. Registry rows
are retained, and temporary projects with any saved issues (including deleted
issues), artifacts, mindmap nodes, project settings, or worker settings remain
accessible. The same visibility rule applies to named projects and repository origins.

Worker project settings control whether agents create worktrees. For this project,
keep `worktree_enabled` and `prs_enabled` disabled: work in the existing checkout,
commit changes, and push to main. This does not delete old worktrees or interrupt
agents that are already using them.
