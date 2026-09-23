# Nested verification reservations

## Issue 136 investigation — September 23, 2026

The preserved poe2 issue 27 agent command explicitly nested two blocking locks:

```text
lockf … slot.1 lockf … slot.2 env POE2_PRE_COMMIT_SLOT_HELD=1 … checks && commit
```

Its stated intent was to reserve both existing slots and serialize Turbo after
earlier check timeouts. This was agent-authored command composition, not an
extra reservation introduced by Hey Boss's worker scheduler. The slot-held
marker was applied after the second lock; it cannot make raw `lockf` reentrant.
The standard poe2 `scripts/with-concurrency-slot.sh` already exports that marker
to nested wrapper invocations so one validation tree uses one admission.

The original observations, from poe2 issue 27 comments 7461/7521, issue 37
comments 7504/7507, and the owner's retained command and status, were:

| Process | Relationship | Observed state | Payload |
| --- | --- | --- | --- |
| 25643 | Outer lock process, PPID 1 | Held slot.1 | No checks started |
| 44503 | Child of 25643 | Waited for slot.2 | No checks started |
| 61325 | Independent issue 20 lock owner | Held slot.2 | Enabled commit/build/tests running |

The owner subsequently verified its own two-process group contained only the
two lock wrappers, released that reservation, and resumed a normal hook-enabled
single-slot commit. Issue 37 comment 7516 and issue 27's later status record
that correction. This investigation did not modify those processes, worktrees,
locks, hooks, or worker controls. Historical PIDs are not current identities.

## Reproduce without using production capacity

From this source checkout, run the Rust probe against the repository wrapper to exercise the actual
admission implementation. It uses only its own temporary pool and process groups:

```sh
rustc --edition 2024 tools/investigate_verification_slots.rs -o /tmp/verification-slots
/tmp/verification-slots /absolute/path/to/poe2/scripts/with-concurrency-slot.sh
rm /tmp/verification-slots
```

The three scenarios require normal child exits and fresh completion markers:

1. A peer holds slot.2; an outer holder reserves slot.1 while its child waits
   for slot.2. Both slots are busy without the nested payload being admitted.
   Releasing the fixture peer normally admits and completes that payload.
2. The standard wrapper completes through slot.1 while the peer retains slot.2.
3. Two nested invocations of the standard wrapper also complete through slot.1
   while the peer retains slot.2, proving inherited admission works.

The controls check that a third independent acquisition is refused while both
slots are occupied, and that each job releases only its own reservation. Every
scenario verifies both slots are available afterward. The probe prints holder
and waiter PIDs and requires `COMPLETE: 3/3` in addition to exit 0. Timeout,
signal, missing payload marker, or unexpected lock status fails the probe.
Its cleanup affects only processes and files it created. macOS uses `lockf`;
Linux uses `flock`.

The September 23 run completed **3/3** on this Mac (`lockf`) and connected
devbox (`flock`), with normal exits and all fresh payload markers. The wrapper's
SHA-256 was `a316d9d7ad113dd725aef900325428568b00616e2faf061e561bb19c144b4c8d`.
A private copy with the inherited-marker export deliberately removed passed
the first two scenarios, then failed nested admission with a queue timeout and
missing payload evidence (exit 1, no completion summary). Its private peer and
pool were cleaned up. That negative control verifies the probe rejects the
failure this guidance is intended to prevent.

## Diagnose an existing queue without changing it

Use the **actual pool configured by the owning command**, including any
`POE2_PRE_COMMIT_SEM_DIR` override. On macOS, these read-only commands help:

```sh
lsof /actual/pool/slot.1 /actual/pool/slot.2
ps -p HOLDER_PID,WAITER_PID -o pid,ppid,pgid,etime,comm
pgrep -P HOLDER_PID
pgrep -P WAITER_PID
```

In `lsof`'s FD column, a lock indicator such as `W` establishes a whole-file
write lock at that instant; `u` only describes read/write access. A process
merely opening a slot file does not prove it owns the lock. On Linux, consult
`/proc/locks` or `lslocks`, matching device/inode and PID; blocked `/proc/locks`
entries carry `->`. `flock` ownership can survive through inherited descriptors,
so a process name alone is insufficient there too.

Correlate the observed lock with process ancestry, the saved invocation, and
fresh payload output. Report `holder(slot.1)=PID → child waiting(slot.2)=PID;
holder(slot.2)=PID; checks=not started` only when evidence supports each part.
Otherwise label the relationship or admission state unknown. Record the time
and recheck because processes can exit and PIDs can be reused. An empty log or
PPID 1 does not establish abandonment or permission to release anything.

## Prevention and scope

Installed Hey Boss worker defaults and its canonical skill direct agents to use
the project's admission wrapper once and preserve inherited ownership. They
forbid extra reservations as a workaround for slow checks unless exclusivity
is an explicit task requirement. Truly exclusive work needs the owning
project's coordinated exclusive-admission design; nesting scarce locks is not
an atomic reservation. Never forge a held marker, increase capacity, remove
lock files, skip hooks, or treat incomplete checks as successful to escape a queue.

This is guidance for command generation, not a runtime lock broker or a promise
to intercept arbitrary shell commands. Authored prompt overrides remain intact.
The existing live incident was corrected by its owner; no scheduler concurrency
or upstream semaphore implementation change is justified by this evidence.
