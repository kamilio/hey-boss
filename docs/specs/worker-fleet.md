# Worker Fleet Specification

Status: Accepted

Implemented Through: Not applicable

Purpose: Keep independently running workers synchronized, configured, and observable across connected and disconnected machines.

## Normative Language

MUST, MUST NOT, SHOULD, and MAY describe required, prohibited, recommended, and optional behavior. Implementation-defined policies MUST be documented.

## Problem Statement

Separate machine queues make existing work invisible and give users no reliable view of connectivity or configuration. A temporary network failure must not discard work or require manually restarting workers.

## Goals and Non-Goals

The fleet MUST distribute desired configuration automatically, synchronize durable issue replicas, retain offline work, report connections and activity, and accept worker signals. The supervisor owns canonical queue order and conflict arbitration. Companions own their local execution processes and durable outgoing changes. Shared SQLite files over a network and consensus among multiple supervisors are outside this contract.

## System Overview and Domain Model

The execution hierarchy is **Supervisor → Worker → Agent**. The supervisor
coordinates the fleet and shared queue. A worker owns coding-agent processes,
issue pickup, lifecycle and retries. An agent is a Codex coding session working
on an issue. A companion is the per-machine service that synchronizes replicas
and applies worker controls. CLI help, web labels, errors and documentation MUST
use these terms consistently.

One supervisor maintains a configured inventory of SSH machines. Each companion has a stable machine ID, a local issue replica, local worker processes, an outgoing change journal, and an applied configuration revision. Workers retain stable IDs across restarts. An allocation identifies an issue by project ID and issue number and grants exclusive pickup to a machine. Connection state is distinct from worker execution state.

## Protocol

The CLI MUST accept `fleet supervisor` and `fleet companion`; legacy `fleet
controller` and `fleet agent` commands MUST remain aliases. Persisted roles,
service registrations, locks and protocol-v1 fields MUST remain compatible with
existing installations. Status MUST expose the supervisor identity and connection
using the new terminology while retaining legacy identity/connection keys for
older clients. Worker stop/restart MUST leave the fleet supervisor running.

The transport MUST authenticate with configured SSH credentials and validate host keys. Protocol version 1 uses newline-delimited JSON over a persistent, bidirectional SSH channel. Frames MUST be bounded to 16 MiB. A companion sends `hello`, `heartbeat`, and `ack` messages. Heartbeats include durable outgoing changes, worker activity, configuration revision, and the pull cursor. The supervisor sends `configure`, `ping`, `pull`, and `signal` messages. Pulls carry journal receipts and a full initial snapshot or incremental canonical changes. Every signal has a stable request ID; replay MUST NOT apply it twice. Events have monotonic sequence numbers within a supervisor epoch. Connections MUST use a five-second heartbeat and become disconnected after fifteen seconds without a valid response.

Companions supporting streamed pulls MUST advertise `pull_gzip_chunks` in
`hello.capabilities`. The supervisor MAY send `pull_begin`, ordered `pull_chunk`
frames and `pull_end` for these peers. The transfer uses gzip-compressed JSON
and base64 chunks; each chunk contains at most 4 MiB of compressed bytes.
Every frame MUST remain within the 16 MiB limit, including a single domain row
larger than that limit. Small incremental pulls MUST retain the ordinary `pull`
format. For peers without the capability, an oversized pull MUST fail with an
upgrade error before any partial frame is sent.

The companion MUST stage a streamed pull privately and verify its transfer ID,
chunk order, byte and chunk counts, SHA-256 digest, and complete gzip stream
before applying it. A malformed, interrupted or overlapping pull MUST leave
the replica and its cursor unchanged. Complete pulls MUST apply atomically and
preserve local edits made while the transfer was in progress. Temporary transfer
data MUST be removed when the connection closes. Transfer verification MUST NOT
be treated as acknowledgment of a committed replica cursor.
While verification or atomic application takes longer than a heartbeat interval,
the companion MUST send progress responses every five seconds. These responses
MUST NOT advance the acknowledged cursor. The supervisor MUST measure companion
silence after completing its own outgoing transfer.

The supervisor SHOULD update heartbeat timestamps without rewriting durable
machine snapshots. Identical machine updates SHOULD NOT require a database
writer lock. Connection, configuration and worker changes MUST remain durable;
a failed save MUST remain eligible for retry even when subsequent fields are
identical. Every successful snapshot save MUST include the latest heartbeat.

Each machine activity poll MUST read worker capacity, runs, chief activity and
upgrade state from one coherent database snapshot. A poll SHOULD read the
bounded worker overview once rather than repeat it for each worker. Machine
activity polling SHOULD NOT load unrelated public-status data, such as the
durable outgoing journal count. Existing machine activity fields and bounded
recent history MUST remain compatible with older peers.

## Configuration

The supervisor MUST derive its inventory from the existing machine configuration and distribute the supervisor identity, companion role, configuration revision, desired worker settings, and software build. Invalid configuration MUST leave the previous valid configuration active and expose an error. Companions MUST persist configuration atomically. The supervisor MUST reconcile reachable machines on startup, reconnect, configuration changes, and source changes. Deployment failures MUST be visible and retried with backoff. Explicit deployment MUST remain available.

## Synchronization and Offline Processing

Companions MUST pull canonical changes whenever a connection is available, after uploading durable local changes. Journal writes MUST commit in the same transaction as the domain change. Acknowledgments MUST be durable; replay after acknowledgment loss MUST not duplicate comments, events, or issue mutations. Incoming synchronization MUST not generate outgoing echoes.

The supervisor MUST retain the latest 10,000 canonical journal entries and prune
older entries in batches of at most 1,000. Pruning MUST commit a durable cursor
floor atomically with deletion. A cursor below that floor or above the durable
journal high watermark MUST receive a current snapshot. Snapshot cursors and
future journal sequences MUST NOT regress when history is deleted. Pruning MUST
NOT delete companion outgoing changes, replay receipts or conflict evidence.
When no entries exceed retention, maintenance SHOULD remain a database reader
and MUST NOT wait for an unrelated writer to finish. Compaction MUST preserve
the canonical snapshot, journal high watermark and replay identities.

While a local update awaits its receipt, incoming pulls MUST preserve fields
changed by that update and merge canonical values for other fields. Pending
creations and deletions MUST retain their whole local row until acknowledgment.
Same-field changes MUST remain pending for canonical conflict arbitration;
merging incoming data MUST NOT alter the saved outgoing mutation or create echoes.

Companions MAY continue active work and pick up previously allocated work offline. Allocations MUST NOT expire solely because a machine disconnects. The supervisor and other companions MUST exclude another machine's allocations from pickup. Unallocated replicated issues MUST NOT be launched offline. Explicit human reassignment MAY revoke ownership and MUST be observable after synchronization. Offline issue creation MUST use supervisor-reserved number ranges; exhaustion MUST produce a visible error rather than collide with another machine.

Concurrent changes to different fields MAY merge. Conflicting changes to the same field MUST be retained durably for review and MUST NOT silently overwrite canonical data. Offline completion MUST NOT close an issue whose requirements or ownership changed on the supervisor. Pending changes MUST survive companion, supervisor, and machine restarts. Local checkouts and process metadata MUST remain machine-specific. Fleet database operations MUST use the CLI bundled SQLite, version 3.51.3 or later, rather than the machine Python SQLite library.

## Signals and Recovery

Pause MUST stop new pickup and retain active sessions. Resume MUST enable pickup. Stop MUST stop owned sessions before releasing capacity. Restart MUST stop the previous worker and its owned agents before starting its replacement with the same worker ID and settings. It MUST leave the supervisor and companion running and MUST NOT acknowledge before the exact replacement registers. Lifecycle mutations MUST serialize across companion processes. Interrupted stop/start phases MUST survive process exits and exclude ordinary reconciliation until replay completes. Retry delays MUST be bounded to five minutes. A newer explicit control MUST supersede unfinished prior intent, and replay MUST NOT rewrite newer desired state. SSH heartbeats MUST remain responsive during a restart. Signals issued while disconnected MUST be queued and show pending state until acknowledged. A disconnected companion MUST continue its saved desired state without inventing new signals.

## Observability and Web Application

The web view MUST show all configured machines, including offline machines; connection state; last heartbeat and synchronization; desired and applied configuration/build; active capacity; tasks; recent events; pending changes; conflicts; and signal acknowledgments. Worker history MUST be separated from active capacity. Browser mutations MUST use same-origin CSRF protection. Connected companions MUST send activity updates without requiring manual refresh.

## Failure Model

An unreachable host MUST remain visible and reconnect with bounded backoff. A protocol mismatch MUST show an upgrade error. A failed deployment MUST retain the working installation. A failed sync MUST retain the journal. Configuration or code changes MUST NOT cancel active work merely to deploy an update; automatic replacement drains active jobs first. An explicit restart is allowed to cancel owned sessions.

## Test and Validation Matrix

| Contract | Required evidence |
| --- | --- |
| Terminology and compatibility | Supervisor/companion help and legacy aliases; new status labels and old saved roles; worker restart preserves supervisor |
| Durable offline changes | Disconnect, mutate and restart, reconnect, verify exactly one canonical result |
| Exclusive pickup | Supervisor and two replicas compete; only allocated machine reserves |
| Replay safety | Lose acknowledgment and replay; comments and mutations remain unique |
| Streamed snapshots | Oversized rows, interrupted transfer, concurrent local edit, malformed chunk order, digest and gzip validation |
| Journal retention | Bounded batches, durable floor rollback, stale-cursor snapshot, stable high watermark, companion protection, receipt replay after pruning, idle maintenance during another write and unchanged snapshot after compaction |
| Conflicts | Concurrent same-field edits and changed requirements reject overwrite/closure |
| Configuration | Revision change reaches companion, survives restart, and queues offline |
| Signals | Pause/resume/stop/restart acknowledgments and duplicate signal replay |
| Connectivity | Heartbeat, EOF, timeout, and reconnect state transitions |
| Machine persistence | Heartbeat-only updates retain current liveness without rewriting snapshots; identical updates during another write; failed-save retry retains worker and configuration state |
| Machine activity polling | Linear overview work for 100 workers, coherent capacity and activity during a concurrent WAL write, public-status activity parity and legacy upgrade marker migration |
| Web application | Responsive layout, accessible controls, live updates and CSRF rejection |

## Conformance Criteria

Conformance requires every MUST and MUST NOT above to pass the validation matrix. This accepted contract describes the requested iteration; it is not a claim that the preexisting independent-queue implementation conforms.
