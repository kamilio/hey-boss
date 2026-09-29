# Mac.lan disk exhaustion — issue 695

Observed on 2026-09-29 UTC (2026-09-28 CDT), using the existing SSH alias
`kamils-macbook-pro.local`. This is an environmental incident; no contributing
CLI defect was identified.

## Diagnosis and approved recovery

Bounded, read-only `df`, `quota`, `diskutil`, directory-size and open-file
inventory found the 494.4 GB APFS container 99.9% occupied. The Data volume
consumed about 468 GB. Available space fluctuated from 411 MiB down to 135 MiB;
inodes remained available, the user had no quota, and the Data volume had no
APFS snapshots. The companion repeatedly exited with OS error 28. A temporary
reconnection and an empty journal did not establish recovery: it disconnected
again while storage remained exhausted.

The owner approved Hey Boss request `085030d2-6890-49a8-8131-83b2ce858ee1`
with **Approve exact deletion**. Only
`~/Workspace/poe-code-extract-column-997/node_modules` on Mac.lan was removed.
It was a real directory, ignored by Git, with no tracked files. Open-file and
working-directory checks found no use of that worktree; its tracked source was
clean and issue 997 was unassigned. Its dependencies must be reinstalled before
using that worktree again.

The deletion increased available space from 140,316 KiB to 1,326,496 KiB
(about 1.13 GiB recovered). The source worktree and Git history remained intact.
No database, journal, credential, claim or process was deleted or reset. No
worker was started, paused or restarted during this first action. Reconnection
used the existing fleet retry loop.

That first recovery was insufficient: by 00:09:52 UTC the companion had
disconnected with OS error 28 again, and a 00:10:21 disk check found only
139,016 KiB available. The worker continued launching tasks. The remaining
space was consumed within about a minute, so this brief reconnection does not
meet the stable-recovery acceptance criterion.

The 19-sample observation from 00:09:52 through 00:12:56 UTC failed: the
companion briefly reconnected and drained pending changes, then disconnected
again. The incident remains open.

Further approval request `2f46d586-e1be-4af3-93a2-e3bee5075fb4` is pending.
The proposed action is to pause new pickups on existing worker
`0fcf3068617e661427e6c332501e0a08`, preserve its active sessions, and remove only
the unused `~/Workspace/poe-code-extract-jq-into-974/node_modules` (1.12 GiB).
New pickups would remain paused until the owner resumes them. The original
request also listed `poe-code-3/node_modules`; a deeper check found active
process working directories there, so it was excluded before any action.
Correction notification `595501c8-968b-4465-989a-15f9395d0d32` records that
reduced scope. Nothing in `poe-code-3` was deleted.

## Journal and conflict verification

Before deletion, a read-only companion database snapshot showed journal
watermark 275153 and zero outstanding rows. The supervisor had exactly 275153
durable receipts spanning sequences 1 through 275153, with no sequence gaps.
All 2000 receipts after the latest conflicting sequence (273153) were applied.
This covers the original reported backlog of 113 changes and subsequent work.

The 639 unresolved supervisor conflicts and 355 unresolved companion conflicts
predate this incident; their latest timestamps were 1790621906225 and
1790621906436 milliseconds respectively. They remain unresolved and preserved.
Companion conflict reasons include allocation/ownership disagreements (249),
concurrent field/configuration changes (40), offline status changes (41),
legacy state constraints (24), and offline project registration (1).

Direct owner status at 00:09:57 UTC reported the same worker PID 18690, five
occupied slots and zero free slots. These are timestamped observations, not a
claim of current capacity while the supervisor reports disconnection.

## Installed software and visual checks

Read-only installation audits found the MacBook, Mac.lan and devbox current at
commit `a996bc002d1df0d5dbc6dc743facd82837eeff53`, build
`638e6fc0d9eab8c4`. This recovery changes no executable or web assets and requires
no software reinstall or Fly release.

The installed worker dashboard was inspected in an isolated Chrome session at
1440 × 1000 and 390 × 844, in light and dark appearance. The expanded fleet
warning and Needs attention filter correctly identified Mac.lan as offline,
with last-known activity instead of current available capacity. Screenshots
were visually inspected; no horizontal overflow or console warnings/errors
were found.

The issue-specific Chrome session, temporary web server, screenshots and
Playwright snapshots were removed after review. Existing checkout edits and
artifacts from other tasks were left intact.
