# Mac.lan disk exhaustion — issue 695

Incident and verification: 2026-09-29 UTC (2026-09-28 CDT). Owning companion:
`Mac.lan`, SSH alias `kamils-macbook-pro.local`, node
`46068e91e3e854de85c6199b6e797be4`.

## Diagnosis and permitted recovery

Bounded read-only disk, inode, quota, APFS, directory-size and open-file
inventory found the 494.4 GB APFS container 99.9% occupied, with about 468 GB
on the Data volume. Available blocks fell from 411 MiB to 135 MiB. Inodes
remained available; there was no user quota or Data-volume snapshot. The
companion repeatedly disconnected with OS error 28. No contributing CLI
defect was established; no speculative application change was made.

The owner approved request `085030d2-6890-49a8-8131-83b2ce858ee1` for deletion
of **only** `~/Workspace/poe-code-extract-column-997/node_modules`. Read-only
checks established that it was an unused real directory, Git-ignored, without
tracked files. Its source worktree and Git history were preserved. That
worktree needs dependency installation before future use. Deletion recovered
1.13 GiB (140,316 to 1,326,496 KiB free), but active work consumed that initial
headroom within about a minute, so the first recovery checks correctly failed.

Additional space was subsequently reclaimed externally. This task did not
perform that reclamation. At 03:30:20 UTC, the Data volume had 178,456,100 KiB
free (about 170.2 GiB), with 62% used.

The second proposed deletion/pickup-pause request,
`2f46d586-e1be-4af3-93a2-e3bee5075fb4`, was cancelled and independently read
back as cancelled once additional deletion was no longer justified. Neither
its deletion nor its worker pause was executed. Nothing was removed from
`poe-code-3` or `poe-code-extract-jq-into-974`. Cancellation is not approval;
this request must not be replayed.

No database, journal, credential, active claim, conflict or saved source
worktree was discarded. No new worker, force reset or claim release was used.
Productive workers were not restarted or paused.

## Transport diagnosis and sustained verification

Early checks failed with ENOSPC, followed by heartbeat/SSH timeouts even after
free space improved. Heavy host load was observed but did not establish the
cause of those later timeouts. Power history placed the 01:37:28.117 heartbeat
timeout inside maintenance sleep from 01:37:16 to 01:37:30. One bounded
`caffeinate -u -t 2` assertion at 01:46:38 produced a confirmed FullWake and
exited at 01:46:40. It changed no persistent power setting. A subsequent
heartbeat timeout without another sleep proved that sleep did not explain
every failure and that the short wake alone was insufficient.

Further checks were interrupted by independently initiated software rollouts.
Those epoch/reconnect boundaries were recorded separately from incident
failures. The existing automatic retry mechanism restored each connection.

After the rollouts settled, the uninterrupted verification **passed**:

- Last planned reconnect: 02:50:51.974 UTC.
- Final sample: 03:05:54.853 UTC; 902.471 seconds measured from reconnect.
- 29 live samples, all connected, with advancing heartbeat and sync.
- Supervisor epoch remained `2d5dca4918fa9bb7dad784896789c878`.
- No unexpected reconnect, heartbeat timeout, ENOSPC or connection error.
- Maximum sampled heartbeat age 5.318 seconds; sync age 5.551 seconds.
- Final reported pending count zero; the monitor exited successfully.

Subsequent live reads through 03:28 UTC still showed the same connected epoch,
fresh heartbeat/sync and no connection error. New outgoing changes drained;
the direct 03:30:20 snapshot had zero pending changes.

## Durable reconciliation and worker preservation

At 03:30:20 UTC the companion had journal watermark **276487**, zero outbox
rows, cursor 1194235 and last sync 1790652619.008. The supervisor independently
held exactly 276487 unique receipts, minimum sequence 1 and maximum 276487.
This proves gap-free durable receipt coverage through that watermark,
including the originally reported 113 pending changes and later backlogs.

Pre-existing unresolved conflicts remain preserved:

| Store | Unresolved | Latest conflict timestamp (milliseconds) |
| --- | ---: | ---: |
| Supervisor | 639 | 1790621906225 |
| Companion | 355 | 1790621906436 |

Their counts and latest timestamps did not change during recovery. They
include ownership/allocation disagreements, concurrent edits, offline status,
legacy constraints and an offline project registration; they were not dropped
to make synchronization appear successful.

Direct connected-owner checks confirmed original worker
`0fcf3068617e661427e6c332501e0a08`, PID **18690**, with five active slots out
of five and pickup enabled. The live fleet read at 03:11 UTC agreed. Its
original running build was preserved while installed CLI versions advanced.
Capacity was interpreted only from connected-owner observations.

## Installation and deployment

Both completed Mac installers were followed to successful process exit, then
verified from their installed receipts. MacBook generation **229** and Mac.lan
generation **169** installed main commit
`2fa7764afec55ec82310062775b2093eb4ff68af`, build `e1ac14bb1a905784`.
Subsequent independent development continues; these are timestamped verified
installations rather than a promise that main stops advancing.

An explicit-source devbox upgrade accidentally included unrelated dirty
working files, installing development build `5fb769d3260db937`, generation
144. This was identified and disclosed. A subsequent explicit-source local
build was stopped before publication; the installed MacBook remained on main.
Restoration used standard `hey-boss upgrade --host devbox`, which archives
committed main and enforces installation ordering. Direct installed readback
verified devbox generation **145**, main commit
`b681404a7a953ed95872e557f1a3cd73fb5129d5`, build `4e80efc8aec6c8a7`.
No force/downgrade guard was bypassed.

The first standard retry hit the known exited-SSH-child wait problem (#471).
After reconciling the remote receipt and absence of a remote installer, only
the task-owned stalled client was stopped. A temporary SSH wrapper reuses the
existing authenticated devbox control socket, without changing persistent
SSH/authentication/trust settings. The corrected command was followed through
its existing handle; it was not blindly replayed. After devbox installation,
the final local audit waited for a separate MacBook installer to release its
installation lock. The command then exited: devbox succeeded; the local
attempt stopped before publication with `Desktop staging or rollback already
exists; inspect it before retrying`. Final guarded readback confirmed the
MacBook's intact main build `e1ac14bb1a905784`, generation 229.

Read-only inspection found only `Hey Boss.app.upgrade-previous`, created at
02:28:03 UTC beside the Homebrew app. This rollback predates this task's
corrected rollout and was preserved. The automatic updater independently
retried; its process was not stopped. At 03:37 UTC a separate newer companion
rollout reported `Fleet subprocess timed out`, while the installed main build
remained connected with fresh sync and zero pending changes. These newer
rollout failures are not reported as successful installs. This incident
introduced no executable changes requiring that newer release; the recovery
and existing installed builds were verified independently.

Fly deployment completed successfully for `hey-boss-mobile-kamil`, machine
`811de36a995918` in `ord`, using its existing immutable image:
`registry.fly.io/hey-boss-mobile-kamil@sha256:cc1cd8a4873d95751ead833b3ae9920e9960210a025a24b631e5bc37485c4f76`.
The rolling deployment, smoke/machine checks and DNS checks passed. Readback
confirmed the started machine and passing health check; the public `/healthz`
returned `{"ok":true}`. Pending unrelated local UI edits were not deployed.

## Visual verification and cleanup

The installed dashboard was tested in isolated Chrome at **1440 × 1000** and
**390 × 844**, in light and dark appearance. Screenshots were actually viewed.
Offline views correctly showed last-known activity and the expanded fleet
warning; connected views replaced that with current Mac.lan capacity and
removed the warning. The Needs attention filter correctly became empty.
There was no horizontal overflow and no browser console warning or error.
The final installed-dashboard refresh covered all four viewport/appearance
combinations. Committed web assets were unchanged across subsequent CLI-only
updates, so those visual results remained applicable.

Task-owned Chrome sessions, the temporary dashboard server on port 4789,
screenshots and Playwright snapshots were removed after review. Playwright
confirmed the issue-specific sessions were closed. A later cleanup check found
only the unrelated attached `poe-timeout-hey` session, which was preserved.
Existing unrelated browsers, working
changes and artifacts were preserved. The optional npm wrapper's DNS failure
was bypassed using the already-installed Playwright CLI; that failed command
also exited. Stopped task-owned installer snapshots were individually removed.
The corrected installer exited, its snapshot was automatically removed, and
the temporary SSH wrapper was removed. No task-owned installer, monitoring,
keep-awake or test-server process remains. Independent application services,
automatic updaters and productive workers were preserved.

Earlier failed checks and continuation details remain in this document's Git
history and the issue comments; the successful measured interval above
supersedes the earlier incomplete stability results.
