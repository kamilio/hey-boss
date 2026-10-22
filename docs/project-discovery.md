# Project discovery

Run `hey-boss project init` inside a checkout to choose pull requests and
worktrees, preview the current Markdown prompts, and save the project in Builder.
Enter keeps existing choices; `q` or Ctrl-C cancels before saving. Rerunning
preserves custom prompts and checks for concurrent settings changes.
For scripts, pass `--yes --prs true --worktree true` (use `false` for either
choice); `--project` selects a name or full ID and `--json` returns settings.
The wizard also offers drafts; use `--drafts false` to disable new drafts for
that project. Existing drafts remain saved. Omitting `--drafts` in scripts keeps
the saved choice (enabled for new projects). For an initialized project,
`hey-boss issue settings set --no-drafts` changes only this setting.

Artifacts follow the same rule: initialization accepts
`--artifacts false`; settings accepts `--no-artifacts`. Omitted choices preserve saved values and default to enabled for
new projects. Disabled creation exits nonzero with a project-specific message
pointing to regular tasks. Existing content remains available.

All workflow and resource switches live in Builder → Project settings →
Configuration. Prompt editors stay in their
own tabs.

To separate two checkouts of the same repository, initialize the second with
`hey-boss project init --project poe2-legacy` and choose its own workflow, such as
no pull requests. A custom identity is saved in a local `.hey-boss` JSON file at
the checkout root. Ordinary repository-based projects need no file. The wizard
also accepts a project name when no `--project` is supplied. Initialization does
not switch Git branches or start workers.

Commands in that checkout and its subdirectories use the saved identity; an
explicit `--project` still takes precedence. Each Git worktree uses its own
repository identity unless explicitly initialized with a local `.hey-boss`;
the main checkout's marker does not affect other worktrees. Invalid files fail
instead of silently selecting another project. Keep this machine-local file out
of Git. Reinitializing with the ordinary project removes an unnecessary override;
cancelled setup leaves the file untouched.

Only `hey-boss project init` registers new projects. Cancelling initialization,
reading project data, sending notifications, and observing running agents never
create registry entries. Existing registry rows remain usable.

Commands resolve the current Git repository (including subdirectories and linked
worktrees) or the nearest registered local parent directory. Unrelated folders
with the same basename do not select each other's project. When no registered
project matches, the command fails: use `--project <name-or-id>` to select an
existing project, or run `hey-boss project init` in the project directory.

Builder lists initialized projects and projects with saved issues, artifacts,
or worker settings. Empty legacy discoveries stay out of the
picker. Their identities and all saved data are retained. Hidden projects stay hidden.

Project names are unique identifiers (case insensitive). Explicit selectors can
use an existing name or compatibility storage ID. Quick Issue submissions use
existing names. Git origins and local paths remain compatibility storage IDs.
Worker dashboards, terminal titles and plain-text status show project names;
JSON retains the storage keys for compatibility.

The registry rejects duplicate names even when another process writes directly.
Name reuse is silent: it creates neither another project nor a warning record.
Upgrades discard old warnings about aliases that never became projects. Paired
devices also deduplicate their inventory and accept names as issue destinations.

On upgrade, existing duplicate names choose one stable destination, preferring
saved undeleted issues, then artifacts, then the oldest entry.
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
issues), artifacts, project settings, or worker settings remain
accessible. The same visibility rule applies to named projects and repository origins.

Worker project settings control whether agents create worktrees. For this project,
keep `worktree_enabled` and `prs_enabled` disabled: work in the existing checkout,
commit changes, and push to main. This does not delete old worktrees or interrupt
agents that are already using them.
