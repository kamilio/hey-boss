# Project shelf

Run `packages/git-shelf/install.sh` on a machine that uses the shelf. Put
`~/.local/bin` on PATH ahead of any older npm installation of `g`. The client
uses the installed Hey Boss CLI and its configured issue store. Enrollment is
local: the installer creates `~/.config/hey-boss/git-shelf`. Normal upgrades
build and verify the optional binary before installing it alongside the backend,
and restore the previous binary if publication fails. Machines without that file
skip the client. Remove the marker to stop updating it.

Run `g` or `g list` to see active commits in this checkout and saved commits
shared by the project's worktrees. Active commits are listed oldest first,
starting after the remote base. In the TUI, select an active commit and press
Ctrl+S to shelve it. Newer commits are replayed in order; if that conflicts,
the branch stays unchanged. Enter opens the selected diff. On a saved commit,
use the arrows to select a line and Ctrl+C to comment.

- `g stash-commit SHA --task 123`: save the current branch tip, attach the task, and remove
  the tip only after verifying that the stored patch reproduces its Git tree.
  Omit the task to shelve without one; use `g link SHA --task 123` later.
  Capture the SHA with `git rev-parse HEAD` after committing. Passing it
  guards against shelving a different tip; omitting it selects the current tip.
- `g open SHA`: read the saved diff and review comments. Local commits that
  haven't been shelved open in Git's normal diff viewer.
- `g apply-commit SHA`: cherry-pick into the current worktree. Conflicts use normal
  Git resolution: stage the fixes and run `git cherry-pick --continue`, or use
  `git cherry-pick --abort`. The saved commit and comments stay on the shelf.

Shelving and restoring require a clean index and tracked files. Unrelated
untracked files stay in place. Another clone can restore the exact commit if it
has the commit's parent. Root and merge commits cannot be shelved. Commit
reviews have the existing Hey Boss 12 MiB snapshot limit.
