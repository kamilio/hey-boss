# Project discovery

Projects represent repositories, not individual checkouts. Git origins identify
the same project across machines and worktrees; repositories without an origin
use the machine ID and shared Git directory. Two unrelated repositories with the
same name remain separate projects.

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
