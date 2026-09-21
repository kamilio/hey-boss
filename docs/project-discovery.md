# Project discovery

Project names are unique identifiers (case insensitive). A folder, repository,
worktree, notification or explicit project selector with an existing name reuses
that project's destination instead of creating another project. Quick Issue
submissions use names. Git origins and local paths remain compatibility storage
IDs, not separate destinations with the same name.

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

Automatic agent discovery ignores home directories and local temporary folders,
including macOS per-user temporary directories. Live agent tests previously
registered folders such as `hey-boss-live-controls-*` as permanent projects,
which filled the project picker with test runs. Existing empty temporary entries
are now omitted from project lists on desktop and paired devices. Registry rows
are retained, and temporary projects with any saved issues (including deleted
issues), artifacts, mindmap nodes, project settings, or worker settings remain
accessible. Named projects and repository origins are unaffected.

Worker project settings control whether agents create worktrees. For this project,
keep `worktree_enabled` and `prs_enabled` disabled: work in the existing checkout,
commit changes, and push to main. This does not delete old worktrees or interrupt
agents that are already using them.
