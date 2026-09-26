# Worktree ownership and recovery

## Reopened investigation — September 25, 2026

The new poe-code reports describe missing checkouts and validation evidence around
11:32–11:36 CDT, including `poe-code-overnight-2` and `poe-code-fleet-3`. Both paths
were still absent on this Mac during the follow-up; `poe-code` was present.
Existence now does not establish recovery of the original index or uncommitted
work. Recovery refs, surviving checkouts and other owners were left untouched.

The later aggressive harvester implementation (`3f47521`, committed at 12:34 CDT)
introduced a separate removal path that ignored Git ownership locks and dirty
files after 24 hours. It recursively deleted checkout contents and administrative
metadata, retaining only HEAD in a recovery ref. `retire_missing` could also delete
the surviving index of a missing checkout. This is a reproducible protection gap,
but its commit timestamp is later than the new reports; it does not attribute
those earlier losses to this implementation. The retained local health activity
covered only recent cycles, with no receipt identifying those deletions.

Aggressive cleanup must preserve the same ownership, active-process, index and
uncommitted-state boundaries as ordinary cleanup. A missing directory is never
permission to discard its Git metadata. Age and a saved HEAD cannot replace the
index, untracked implementation, or validation receipts. The regression fixtures
use private repositories only; they do not reproduce deletion on production paths.

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

## Installation verification — September 25, 2026

Commit `a07020b979b55ae53c4e360ac29acb641469ef05` is published on main.
Its completed GitHub runs are [Check 36088860139](https://github.com/kamilio/hey-boss/actions/runs/36088860139)
and [Worker TUI 36088860144](https://github.com/kamilio/hey-boss/actions/runs/36088860144):
19/19 macOS steps, 8/8 Linux steps, 9/9 mobile steps, and 21/21 TUI steps
on each platform. Every step was completed successfully; no cancelled or skipped
step was counted as validation. These results cover this commit, not later main
changes, whose separate CI failures were observed during the handoff.

The original fleet upgrade ended with SIGTERM and is incomplete. A subsequent
read-only fleet audit found the main CLI already updated on this MacBook,
`devbox`, and `kamils-macbook-pro.local` to build `e20efe1410f1cf31`, commit
`4254d7d84fe4479154ecdd3855cc10ac2723a678`, which includes the ownership fix.
The installed CLI probe completed normally with 4/4 checks on each host.
Installed canonical skill hashes also match. A fresh browser run using the
installed CLI completed 33/33 checks across 320/390/768/1440-pixel widths in
both themes; the native audit completed with four ownership-detail captures
at 820/920 points. The full selected ownership reason remains readable and
wraps without clipping. The isolated browser fixture has no inbox service.

Fly release 211 was deployed from the validated commit with a normal exit,
completed machine checks, and image digest
`sha256:6bf1ba856422381f2568e82b99697836fe056986f7713661ef27b969eb5e2c4f`.
The fresh machine audit reports it started and passing; `/healthz` returns
`{"ok":true}`.

**Standalone health workers must be verified separately.** Remote health
commands prefer `~/.local/bin/hey-boss-health` over the main CLI. All three
standalone workers initially remained at `d7552906194359d1`; the ownership
probe failed because the old worker omitted the lock reason. Updating only
the main CLI therefore does not complete this issue's installation.

Both Mac standalone workers were subsequently installed at `9b70030bd678bbda`
from the validated release. Each final installer command exited normally after
30/30 health tests, 2/2 selected hey-gh tests, the release build, and the explicit
installation marker. Their installed ownership probes completed 4/4. Earlier
Mac installer attempts ended with shell errors after replacement when their
script was changed during execution; those attempts were not accepted. The final
runs used an unchanged script. The local rerun waited normally for both Cargo's
build cache and an active harvester's maintenance lock; neither owner was stopped
and no lock was released by this task.

During verification, main advanced through `b6019be` and extracted maintenance
into `packages/hey-harvester`. Its `health/worktrees.rs` is byte-for-byte identical
to the validated implementation. The installation probe now also supports the
standalone `hey-harvester` command shape; its previous `health` prefix was
rejected before exercising any checks. After adaptation, the installed local and
devbox harvesters completed 4/4 checks, and the legacy Mac worker completed 4/4.
This verifies the new entry point but does not establish that every existing
client and schedule has stopped using the old helper.

At that handoff, devbox's legacy installer validation was blocked: 31/35 health tests
passed, while four real-process/worktree tests could not inspect a non-dumpable
process through `/proc`. The worker correctly preserved worktrees when process
visibility was incomplete. Linux CI runs these tests with privileged inspection;
devbox requires a sudo password and rejects unprivileged PID namespaces. Neither
test failure nor an unsuccessful privileged-runner attempt permits installation.
The old standalone devbox worker remains installed. Provide an authorized test
runner with the necessary process visibility, rerun all required health tests and
the installer, then require the standalone worker's 4/4 ownership probe before
closing issue 147. Alternatively, finish and verify the harvester migration,
including legacy callers, so the outdated helper is no longer selected.
Do not stop the unrelated process or weaken cleanup checks.

## Harvester migration and terminal details — September 25, 2026

The migration alternative above has now been verified. Both Mac launch agents
and devbox's systemd service invoke `~/.local/bin/hey-harvester run`. The installed
Hey Boss clients on all three machines prefer harvester over the obsolete helper.
`tools/harvester_routing_install_checks.mjs` executes each installed client's
generated remote command inside a disposable HOME with synthetic workers. Its
three checks cover local-bin precedence, cargo-bin precedence, and failure without
falling back to the legacy worker. The stale helper fails the negative control.
No production schedule, maintenance setting, or worktree is mutated by this probe.

Visual testing of the extracted terminal dashboard found another visibility gap:
long worktree paths clipped the owner and preservation reason. Enter now opens
the selected entry in a wrapped details pane. Arrow keys and Page Up/Down scroll
to the final line; Escape returns to the list. Cleanup keys do not dispatch from
details. A Rust rendering regression covers 120x24, 80x24, 48x20 and scrolling at
48x14. `tools/harvester_ownership_terminal_checks.mjs` exercises the actual binary
in a real PTY with synthetic remote status, captures those layouts, cancels a
removal confirmation, and requires a normal exit with no maintenance mutations.
