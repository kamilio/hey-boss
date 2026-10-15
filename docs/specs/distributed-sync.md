# Distributed execution over SSH

Every enrolled machine durably accepts local task changes and schedules local
work without contacting another machine. Overlapping attempts are allowed.
Only the machine owning a process may operate that process. A received remote
attempt never replaces, cancels, or resumes a local attempt.

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
checkpoints commit with projected changes. Replication storage and application
storage may recover independently without losing an acknowledged mutation.
Credentials, process identities, filesystem locks and machine-local checkouts
are never imported as another machine's executable state.

Rollout is additive: install capture, backfill, synchronize in shadow mode, verify
coverage, then switch scheduling and disable legacy mutation replication in a
short cutover. Preserve active workers, pending changes and conflict history.
An older peer cannot enter the distributed protocol. Reverting a binary must not
discard facts accepted since cutover. Restores must not silently rewind a peer's
delivery identity.

Required validation: main unavailable; two disconnected machines picking up the
same task; reconnect in both orders; process restart after local commit and after
remote commit before acknowledgment; duplicate and missing batches; requirements
changed during an attempt; offline deletion; conflicting relationships; large
records; attachments and archived inputs; old-client fencing; worker-preserving
upgrade; all reachable enrolled machines running the verified build.
