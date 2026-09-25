# Worktree ownership and recovery

## Issue 147 investigation — September 24, 2026

Two owners reported losing both a temporary checkout and its Git registration:
`/private/tmp/poe2-negative43-0924` during a queued hook, and
`/private/tmp/poe2-usage38-refresh-0924` after publication. The latter does not
establish interference with active validation. No deleting process has been
identified. A removed registration alone cannot distinguish `git worktree remove`
from directory deletion followed by pruning, or direct metadata deletion.

The retained Machine Health records explicitly preserve **both paths** as
“Outside configured roots or symlinked” at Unix timestamps 1790295472,
1790298065, and 1790300010. Its configured roots were `~/Workspace` and
`~/.codex/worktrees`, excluding `/private/tmp`. The reviewed production removal
paths are `health::worktrees::clean` and `remove_one`: both recheck activity,
registration, index state and locks before ordinary `git worktree remove`;
neither forces removal, prunes registrations, nor removes branches. Cache cleanup
only targets recognized cache/test directories, not arbitrary `poe2-*` trees.
The worker delegates worktree lifecycle commands to its agent; it has no automatic
worktree-removal routine. The poe2 admission wrapper does not remove worktrees.
Health records these item decisions when the scan finishes; those timestamps
do not prove the directories still existed at that exact moment.

A targeted read of the September 24 local Codex command records found owner
recovery and loss reports, but no command attributing either deletion. This is
bounded negative evidence, not proof about other sessions, applications, earlier
code, or unrecorded shell commands. Git locks protect cooperating Git/health
cleanup; they cannot prevent arbitrary filesystem deletion or forced unlocking.

## Before starting work or entering validation

Use the project's stable workspace path. Create a new linked checkout atomically
locked with `git worktree add --lock --reason 'ISSUE; OWNER SESSION; active work'`.
For an existing checkout, inspect `git worktree list --porcelain`, its branch,
HEAD, staged and unstaged changes, and lock reason before reusing it. If it is
unlocked and belongs to this task, use `git worktree lock --reason` before editing
or queuing validation. Never replace another owner's lock.

Record the project/issue, session, branch, absolute path, HEAD and validation
receipt location in the issue's progress history. Git's lock reason appears in
Machine Health's Worktrees preservation reason, so a queued or interrupted owner
remains visible even without an open file or running agent. Keep the lock through
queueing, checks, publication and interruption. It reserves no validation capacity;
use the project's single admission path as usual.

## Resume without losing work

First inspect the saved branch, registration, lock reason, index and receipt.
Check the recorded validation process and its descendants before launching
anything. A quiet log, an old PID, an interrupted chat, or a missing UI row is not
proof that validation ended. Reuse a live validation job and preserve its genuine
admission ownership; do not launch duplicate validators.

If the directory survives, preserve staged/unstaged files and inspect or repair
the registration with Git's normal repair workflow after confirming identity.
Do not reset, clean, prune, force-add, or recreate over it. If both directory and
registration are missing, the owner may recreate the **verified existing branch**
in a new locked workspace path after confirming no surviving validator still
depends on the lost checkout. Branch commits can be recovered this way; a branch
does not preserve a deleted index or uncommitted files. Report that limitation
and retain any surviving index, patches and receipts. This investigation did not
modify either reported worktree, branch, claim, validator, or receipt.

## Completed-task cleanup

Only the task owner releases its lock after checking normal validation completion,
publication requirements, and absence of live or queued descendants. Keep receipts
outside the checkout. Recheck branch/HEAD and staged, unstaged, untracked and
ignored files. Then explicitly unlock only that worktree and use ordinary
`git worktree remove` or `hey-boss health remove-worktree ABSOLUTE_PATH`.
Health cleanup still refuses unsafe state and retains the named branch. Do not
use a double force, broad glob, recursive temporary-root cleanup, or prune to
work around a lock. Interruption leaves ownership intact for safe continuation.

## Regression scope

`health::worktrees::tests::ownership_lock_survives_queued_active_and_interrupted_use`
uses a private repository. It checks automatic eligibility, explicit health
removal, and actual Git removal while the owner is queued elsewhere, active,
and interrupted. It verifies the lock reason, registration, branch, index bytes,
staged content and external receipt survive. After the fixture owner completes,
it unlocks explicitly and verifies safe removal retains the branch and receipt.
No production validators or cleanup candidates are used. This tests the discovered
cooperating removal path and ownership gap, not an unproven historical actor.

The September 24 Mac verification completed all five worktree tests. The old
installed CLI failed the installation probe for missing ownership details; the
new build completed all four checks. The native audit exposed truncation of the
selected ownership reason; word wrapping fixes it, with measured text height and
screenshots at 820/920 points in light/dark appearances. The project-settings
preview completed 33 browser checks at 320/390/768/1440 pixels in both themes.
The unrelated inbox service was deliberately absent from that disposable fixture;
its 503 responses were fixture limitations, not a check of inbox availability.
