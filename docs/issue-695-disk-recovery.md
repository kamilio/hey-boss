# Mac.lan disk exhaustion — issue 695

Observed on 2026-09-29 UTC (2026-09-28 CDT), using the existing SSH alias
`kamils-macbook-pro.local`. This is an environmental incident; no contributing
CLI defect was identified.

## Current status

At 02:04 UTC, verification was interrupted by an independent supervisor
upgrade. The resumed check found fresh heartbeats and no connection events
since the 01:52:09.282 reconnect, then stopped after six live samples when a
new supervisor epoch (`89813471c28ca34cc579f04b054cb4df`) reported a reconnect
at 02:03:54.306. This is a rollout boundary, not evidence of another ENOSPC
or heartbeat timeout. The intended 15-minute uninterrupted check did not
complete. The new supervisor reports build `9f723b4557ff38ee`; Mac.lan remains
connected on `638e6fc0d9eab8c4` with deployment `updating`. Do not run a competing
installer. Resume final stability and journal verification after this rollout
settles; issue 695 stays open in the meantime.

At 02:01:32 UTC, Mac.lan had 99,737,448 KiB free (about 95.1 GiB), journal
watermark 276160, and zero pending rows. All 276160 unique supervisor receipts
were present, minimum 1 and maximum 276160. Conflict counts and timestamps
were unchanged. The connected owner still reported original worker PID 18690,
five active slots, zero free slots and pickup enabled. This task performed no
further reclamation, wake, restart or worker-control action.

Direct installed-version reads confirmed the new local binary and previous
Mac.lan binary. Devbox's existing SSH control connection failed with a broken
pipe; its earlier successful audit remains historical. The rollout and its
verification belong to the existing deployment, not a new executable change
for this incident. The resumed monitor and audit commands exited; no temporary
files or browser sessions were created. Unrelated edits remain preserved.

## Earlier post-wake verification

Issue 695 remains open. The post-wake verification from 01:47:10.323 through
01:51:17.652 UTC failed after nine samples / 247.552 seconds. The supervisor
recorded another heartbeat timeout at 01:51:00.265; last heartbeat was
1790646643.933 and last sync 1790646643.109, with three last-reported changes.
The final power-history read showed no new sleep after the full wake described
below. Therefore that wake was insufficient and sleep does not explain every
failure. Do not repeat it automatically or infer a CLI defect without a
reproduction. Sustained resource/transport recovery is still required.

At 01:51:40 UTC, direct disk inventory showed 37,465,700 KiB free. The journal
watermark was 276096 with zero pending rows; all 276096 unique supervisor
receipts were present, minimum 1 and maximum 276096. The 639 supervisor and
355 companion conflicts and their timestamps remained unchanged. No journal,
claim, worker or saved worktree was discarded.

An independent deployment subsequently appeared in fleet history (installing
at 01:51:23.399, failed at 01:51:30.420). Desired build became
`9f723b4557ff38ee`; the connected supervisor and companion still reported
`638e6fc0d9eab8c4`, and the companion reported `Software deployment failed`.
This task launched no installer and did not retry or interfere with that
rollout. Earlier installation audits remain timestamped evidence, not proof
that this new deployment finished. Existing retry reconnected at 01:52:09.282,
which is not sufficient to establish durable recovery.

The post-wake monitor, two-second assertion and read-only SSH commands have
exited. No browser, test server or temporary file was created on this retry.
Unrelated checkout edits, including newly observed Swift changes, are retained.

## Sleep correlation and bounded wake recovery

Further read-only power-history inspection identified an environmental
interruption missed by the instantaneous power-assertion snapshots: Mac.lan
entered maintenance sleep at 01:37:16 UTC and woke at 01:37:30. The
01:37:28.117 heartbeat timeout fell inside that sleep interval. Its history
showed repeated maintenance-sleep/background-wake cycles, including a further
12-second sleep at 01:38:15. The local supervisor had no corresponding sleep.
The remote lid was closed; the power registry reported
`AppleClamshellCausesSleep = No`, with an existing Amphetamine assertion.
Those settings alone did not prove the machine stayed awake.

At 01:46:38 UTC, a bounded `/usr/bin/caffeinate -u -t 2` assertion on Mac.lan
promoted its background wake to **FullWake**, confirmed by the power log.
The command exited normally at 01:46:40. This non-disruptive wake changed no
persistent power setting, restarted no process, and left no task-owned
keep-awake process. Existing Amphetamine and fleet services were preserved.
This correlates the final timeout with actual host sleep; it does not prove
that every earlier SSH failure had the same cause or justify changing CLI
timeouts. The second deletion request remains cancelled.

## Earlier pre-closure failure

The immediate pre-closure guard found Mac.lan disconnected again at
01:37:28.117 UTC with **Companion heartbeat timed out**, after the successful
observation below. Its last heartbeat was 1790645833.058, last sync
1790645832.49, and last-reported pending count zero. Consequently the close
command was never invoked, and a fresh issue read confirmed #695 still open.
The five-minute pass is valid for its measured interval but does not establish
sustained recovery in light of this subsequent failure. No additional action
had been performed at that checkpoint; the cancelled deletion request stayed
cancelled. The subsequent sleep diagnosis and bounded wake recovery are
recorded above.

## Successful observation before the next failure

The five-minute observation passed at 01:36 UTC. The fresh observation from
01:31:03.463 through 01:36:07.950 had 11 connected samples over 304.788 seconds,
advancing heartbeat and sync, no errors, and a final pending count of zero.
Maximum sampled heartbeat age was 5.602 seconds; maximum sync age was
8.406 seconds. Retained event history covering 01:26:06.223–01:36:25.613
contained no disconnect or reconnect, including between the sampled checks.
The later pre-closure failure above prevents treating this as final recovery.

Direct disk readings during the successful check were 25,173,644 KiB free
at 01:31:39 and 24,700,988 KiB at 01:35:54 (about 23.6 GiB at the end).
The additional headroom came from external reclamation, not further deletion
by this task. Only the original explicitly approved 1.13 GiB deletion was
performed; the second request remains cancelled.

The final companion journal snapshot had watermark 276014, zero pending rows,
cursor 1175546, and last sync 1790645749.482. The supervisor held exactly
276014 unique receipts, with minimum 1 and maximum 276014, proving complete
receipt coverage through that watermark. Transient new outgoing changes
observed during the check drained normally. Unresolved conflicts remain
preserved at 639 supervisor and 355 companion, with the original timestamps.

Direct owner status during this check confirmed worker
`0fcf3068617e661427e6c332501e0a08`, original PID 18690, five active slots,
zero free slots, pickup enabled, and a connected supervisor. No worker,
claim, service, database, or saved worktree was reset. Existing automatic
reconnection recovered the transport; its earlier timeout cause remains
unproven, and no speculative code change was made.

All three installed builds remain verified as `638e6fc0d9eab8c4`. Only this
incident document changed; no executable/UI release or Fly deployment is
required. The desktop/mobile visual checks and cleanup recorded below remain
applicable. The final monitor and SSH reads exited normally and created no
new browser sessions or temporary files.

## Earlier observations

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
is absent. At that checkpoint, issue 695 remained open because sustained
fleet connectivity had not passed verification. The later successful check
and its subsequent pre-closure failure are recorded above.
