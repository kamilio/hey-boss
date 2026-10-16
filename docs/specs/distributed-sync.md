# Distributed execution over SSH

Every enrolled machine durably accepts local task changes and schedules local
work without contacting another machine. Each pickup first publishes a machine
reservation, waits for it to settle, then publishes launch intent before starting
the agent. The agent claims the task afterward. Every replica chooses the same
winner: launched work before pending work, then causal ticket order, machine and
attempt ID. Concurrent launches have a stable tie-break; wall-clock timestamps
cannot establish which disconnected machine physically launched first.

A partition may allow overlapping agents. After synchronization, the losing
machine stops its own agent through normal cancellation, preserving saved work.
No imported fact directly controls another machine's process. Scheduling uses
the same rules regardless of connectivity; there is no central permission gate.

Sync exchanges run independently over persistent SSH connections. The shared
exchange interval is two seconds. Settling allows two traversals of a two-hop
relay, including measured exchange duration; expiry allows three such settling
windows without renewal. Timers use local boot-aware monotonic clocks. Each
machine publishes a durable increasing presence sequence with its live run IDs.
Presence is replaceable, outside the immutable history, and relays subtract
elapsed time instead of restarting its lifetime. Replayed presence cannot renew
an expired reservation. Dead worker processes release their reservations; a lost
machine expires at its peers. Restart never revives a previous boot's timers.

The UI presents agents and machine reservations uniformly, including elapsed
time and overlapping-agent counts. The web server's location does not define a
special category of work.

The existing main-to-devbox SSH connections carry synchronization. Main forwards
facts from all origins and supplies the UI; it grants no permission to work.
Devboxes need no direct connectivity. While main is unavailable, changes remain
local until an SSH path returns. No data or coordination uses Fly or another
cloud relay. Retiring remote infrastructure is a separate operator action.

The embedded replication engine uses the pinned open-source superfly/cr-sqlite
fork. Application facts have immutable content identities. A retained local
delivery log and atomic receipts provide resumable, duplicate-safe forwarding;
cr-sqlite's sparse db_version is never treated as a contiguous receive cursor.
Task revisions and attempt results remain available even when current views pick
one conflicting field value. Completion is tied to the requirements used.

Application changes and their outgoing journal commit together locally. Import
checkpoints commit with reduced facts and pending projection work. Replication storage and application
storage may recover independently without losing an acknowledged mutation.
Credentials, process identities, filesystem locks and machine-local checkouts
are never imported as another machine's executable state.

Archive storage changes are independent of logical requirement edits. Cold
headers stay compact; warming an issue restores history in resumable pages
before publishing its hot body. History already cold at enrollment supplies
clock-zero baselines, preserving later edits and deletion tombstones. Incoming
pages wait for locally captured revisions at the end of the journal before
updating application views.

Rollout is additive: install capture, backfill, synchronize in shadow mode, verify
coverage, then switch scheduling and disable legacy mutation replication in a
short cutover. Preserve active workers, pending changes and conflict history.
Rollout verifies active data sync on every destination, including matching builds.
Health expires after 30 seconds without local preparation or a completed exchange;
an open companion connection cannot hide shadow mode, pending data or rejections.
Restricted peer SSH keys run the same continuous protocol as controller links.
An older peer cannot enter the distributed protocol. Reverting a binary must not
discard facts accepted since cutover. Restores must not silently rewind a peer's
delivery identity.

`fleet sync-reconcile` previews retained legacy conflicts; `--apply` recovers
missing progress history and records proven obsolete intent without deleting
original payloads. Archived history and deletion tombstones are checked first.
The verified main YAML takes precedence over rejected offline worker controls.
Ambiguous content stays unresolved and visible in the fleet attention banner.

Required validation: main unavailable; two disconnected machines picking up the
same task; reconnect in both orders; process restart after local commit and after
remote commit before acknowledgment; duplicate and missing batches; requirements
changed during an attempt; offline deletion; conflicting relationships; large
records; attachments and archived inputs; old-client fencing; worker-preserving
upgrade; all reachable enrolled machines running the verified build.
