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

Further approval request `2f46d586-e1be-4af3-93a2-e3bee5075fb4` was left pending
during the disk outage and later cancelled without execution (see below).
The proposed action was to pause new pickups on existing worker
`0fcf3068617e661427e6c332501e0a08`, preserve its active sessions, and remove only
the unused `~/Workspace/poe-code-extract-jq-into-974/node_modules` (1.12 GiB).
New pickups would remain paused until the owner resumes them. The original
request also listed `poe-code-3/node_modules`; a deeper check found active
process working directories there, so it was excluded before any action.
Correction notification `595501c8-968b-4465-989a-15f9395d0d32` records that
reduced scope. Nothing in `poe-code-3` was deleted.

On resumption, the issue state and approval were read before any mutation.
The first deletion remained complete and the second target remained untouched.
At 00:16:49 UTC, disk headroom had temporarily recovered to 1,533,672 KiB,
but fell to 1,224,728 KiB at 00:18:54 and 889,452 KiB at 00:20:15. A second
19-sample connection check ran from 00:17:36 to 00:20:43 (186.642 seconds).
Although the first 18 samples were connected with advancing heartbeat/sync,
the final sample disconnected with OS error 28. Recovery therefore remains
incomplete; the same approval is pending, with no new approval request,
additional deletion or worker-control action performed on resumption.

A later resumed check found larger temporary headroom: 2,982,868 KiB at
00:28:05 UTC. A planned five-minute observation began at 00:28:46 with
6,196,696 KiB free, peaked at 6,820,032 KiB, then stopped after eight samples
(214.15 seconds) at 00:32:20 with a **companion heartbeat timeout**, despite
5,099,824 KiB still free. This is a distinct observed failure, not evidence of
another ENOSPC at that instant or proof of a particular CLI defect. The existing
retry reconnected by 00:33:45 with a fresh sync and zero reported pending
changes, but the full stability check had failed. No additional cleanup or
worker control was performed. The same approval remained pending; revalidate
the need for its deletion before using it now that disk headroom has improved.

At 01:17 UTC, space had recovered externally; this task performed no additional
reclamation. Another planned five-minute check ran from 01:17:57.935 through
01:21:36.130 UTC, stopping after eight samples (218.195 seconds). Available
space stayed above 27,044,820 KiB and reached 33,165,528 KiB. The final sample
nevertheless reported an **SSH server-not-responding timeout**, with
30,849,080 KiB still available and three last-reported pending changes.
The retained event timestamps place the disconnect at 01:21:18.642 and the
automatic reconnect at 01:22:24.002. Stable recovery remains unverified.

A direct owner check at 01:22:43 UTC found 30,723,560 KiB available. The
one-minute load average was 38.72 on ten logical CPUs; a subsequent snapshot
showed 1,898 MiB of swap in use. Sleep was inhibited by existing power
assertions. These observations warrant resource/transport investigation but
do not establish the cause of the SSH timeout or a contributing CLI defect.
No power setting, worker, service, or authentication configuration was changed.

The additional deletion was no longer justified by current free space, so
request `2f46d586-e1be-4af3-93a2-e3bee5075fb4` was dismissed and independently
verified **cancelled**. Cancellation is not approval: neither the proposed
deletion nor the pickup pause was executed. Resume with current read-only
resource and transport diagnosis; obtain explicit authority for any newly
proposed deletion or disruptive action. Do not replay the cancelled request.

## Journal and conflict verification

Before deletion, a read-only companion database snapshot showed journal
watermark 275153 and zero outstanding rows. The supervisor had exactly 275153
durable receipts spanning sequences 1 through 275153, with no sequence gaps.
All 2000 receipts after the latest conflicting sequence (273153) were applied.
This covers the original reported backlog of 113 changes and subsequent work.
The resumed check extended gap-free receipt coverage through sequence 275260;
the companion had zero pending rows at that snapshot. Conflict counts and
their latest timestamps were unchanged.
The next resumed read extended complete receipt coverage through 275349,
including the 40 changes observed pending at 00:24 UTC.

The 639 unresolved supervisor conflicts and 355 unresolved companion conflicts
predate this incident; their latest timestamps were 1790621906225 and
1790621906436 milliseconds respectively. They remain unresolved and preserved.
Companion conflict reasons include allocation/ownership disagreements (249),
concurrent field/configuration changes (40), offline status changes (41),
legacy state constraints (24), and offline project registration (1).

Direct owner status at 00:09:57 UTC reported the same worker PID 18690, five
occupied slots and zero free slots. These are timestamped observations, not a
claim of current capacity while the supervisor reports disconnection.
Another direct owner snapshot at 00:17:02 UTC confirmed PID 18690 with all
five slots occupied and pickup enabled, before the later disconnection.
At 00:28:49 UTC, direct owner status again confirmed that same PID, five active
slots, pickup enabled, and zero pending changes.

After the latest timeout, the direct owner snapshot had journal watermark
275932, zero pending rows, cursor 1175150, and last sync 1790644963.702.
The supervisor held exactly 275932 unique receipts spanning sequences 1
through 275932, including the new backlog. Both conflict counts and their
latest timestamps remained unchanged. The owner again reported original
worker PID 18690, five active slots, zero free slots, and pickup enabled;
this is a timestamped observation, not a guarantee of later capacity.

## Installed software and visual checks

Read-only installation audits found the MacBook, Mac.lan and devbox current at
commit `a996bc002d1df0d5dbc6dc743facd82837eeff53`, build
`638e6fc0d9eab8c4`. This recovery changes no executable or web assets and requires
no software reinstall or Fly release.

A later audit refresh hung after its direct child exited, matching existing
open issue #471. Only the task-owned read-only audit process was terminated.
Bounded direct version checks reconfirmed the same build on the MacBook and
Mac.lan; devbox instead failed with `Connection closed by UNKNOWN port 65535`.
Its earlier successful audit must not be presented as current reachability.
No SSH trust setting, authentication helper, service or live build was changed.

The final direct audit reused devbox's existing authenticated SSH control
socket, with configuration loading disabled for that one read to avoid
invoking a new authentication helper. It succeeded and reconfirmed build
`638e6fc0d9eab8c4`; current direct version checks therefore cover all three
machines. No executable change was introduced by this continuation.

The installed worker dashboard was inspected in an isolated Chrome session at
1440 × 1000 and 390 × 844, in light and dark appearance. The expanded fleet
warning and Needs attention filter correctly identified Mac.lan as offline,
with last-known activity instead of current available capacity. Screenshots
were visually inspected; no horizontal overflow or console warnings/errors
were found.
The resumed session also verified the connected desktop and phone layouts,
including dark appearance: current capacity replaced last-known activity and
the stale offline warning disappeared. These layouts likewise had no overflow
or console errors. The subsequent transport failure was detected by CLI
observation and is not hidden by the successful visual checks.

The issue-specific Chrome session, temporary web server, screenshots and
Playwright snapshots were removed after review. Existing checkout edits and
artifacts from other tasks were left intact.
The resumed monitors and read-only SSH checks have exited; Playwright lists
no browsers, port 4789 has no listener, and the issue-specific output directory
is absent. Issue 695 remains open because sustained fleet connectivity has
not passed verification, despite recovered disk space and reconciled journals.
