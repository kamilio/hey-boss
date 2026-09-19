# Worker Fleet Specification

Status: Accepted

Implemented Through: Not applicable

Purpose: Keep independently running workers synchronized, configured, and observable across connected and disconnected machines.

## Normative Language

MUST, MUST NOT, SHOULD, and MAY describe required, prohibited, recommended, and optional behavior. Implementation-defined policies MUST be documented.

## Problem Statement

Separate machine queues make existing work invisible and give users no reliable view of connectivity or configuration. A temporary network failure must not discard work or require manually restarting workers.

## Goals and Non-Goals

The fleet MUST distribute desired configuration automatically, synchronize durable issue replicas, retain offline work, report connections and activity, and accept worker signals. The controller owns canonical queue order and conflict arbitration. Agents own their local execution processes and durable outgoing changes. Shared SQLite files over a network and consensus among multiple controllers are outside this contract.

## System Overview and Domain Model

One controller maintains a configured inventory of SSH machines. Each agent has a stable machine ID, a local issue replica, local worker processes, an outgoing change journal, and an applied configuration revision. Workers retain stable IDs across restarts. An allocation identifies an issue by project ID and issue number and grants exclusive pickup to a machine. Connection state is distinct from worker execution state.

## Protocol

The transport MUST authenticate with configured SSH credentials and validate host keys. Protocol version 1 uses newline-delimited JSON over a persistent, bidirectional SSH channel. Frames MUST be bounded to 16 MiB. An agent sends `hello`, `heartbeat`, and `ack` messages. Heartbeats include durable outgoing changes, worker activity, configuration revision, and the pull cursor. The controller sends `configure`, `ping`, `pull`, and `signal` messages. Pulls carry journal receipts and a full initial snapshot or incremental canonical changes. Every signal has a stable request ID; replay MUST NOT apply it twice. Events have monotonic sequence numbers within a controller epoch. Connections MUST use a five-second heartbeat and become disconnected after fifteen seconds without a valid response.

## Configuration

The controller MUST derive its inventory from the existing machine configuration and distribute the controller identity, agent role, configuration revision, desired worker settings, and software build. Invalid configuration MUST leave the previous valid configuration active and expose an error. Agents MUST persist configuration atomically. The controller MUST reconcile reachable machines on startup, reconnect, configuration changes, and source changes. Deployment failures MUST be visible and retried with backoff. Explicit deployment MUST remain available.

## Synchronization and Offline Processing

Agents MUST pull canonical changes whenever a connection is available, after uploading durable local changes. Journal writes MUST commit in the same transaction as the domain change. Acknowledgments MUST be durable; replay after acknowledgment loss MUST not duplicate comments, events, or issue mutations. Incoming synchronization MUST not generate outgoing echoes.

Agents MAY continue active work and pick up previously allocated work offline. Allocations MUST NOT expire solely because a machine disconnects. The controller and other agents MUST exclude another machine's allocations from pickup. Unallocated replicated issues MUST NOT be launched offline. Explicit human reassignment MAY revoke ownership and MUST be observable after synchronization. Offline issue creation MUST use controller-reserved number ranges; exhaustion MUST produce a visible error rather than collide with another machine.

Concurrent changes to different fields MAY merge. Conflicting changes to the same field MUST be retained durably for review and MUST NOT silently overwrite canonical data. Offline completion MUST NOT close an issue whose requirements or ownership changed on the controller. Pending changes MUST survive agent, controller, and machine restarts. Local checkouts and process metadata MUST remain machine-specific. Fleet database operations MUST use the CLI bundled SQLite, version 3.51.3 or later, rather than the machine Python SQLite library.

## Signals and Recovery

Pause MUST stop new pickup and retain active sessions. Resume MUST enable pickup. Stop MUST stop owned sessions before releasing capacity. Restart MUST stop the previous supervisor before starting its replacement with the same worker ID and settings. Signals issued while disconnected MUST be queued and show pending state until acknowledged. A disconnected agent MUST continue its saved desired state without inventing new signals.

## Observability and Web Application

The web view MUST show all configured machines, including offline machines; connection state; last heartbeat and synchronization; desired and applied configuration/build; active capacity; tasks; recent events; pending changes; conflicts; and signal acknowledgments. Worker history MUST be separated from active capacity. Browser mutations MUST use same-origin CSRF protection. Connected agents MUST send activity updates without requiring manual refresh.

## Failure Model

An unreachable host MUST remain visible and reconnect with bounded backoff. A protocol mismatch MUST show an upgrade error. A failed deployment MUST retain the working installation. A failed sync MUST retain the journal. Configuration or code changes MUST NOT cancel active work merely to deploy an update; automatic replacement drains active jobs first. An explicit restart is allowed to cancel owned sessions.

## Test and Validation Matrix

| Contract | Required evidence |
| --- | --- |
| Durable offline changes | Disconnect, mutate and restart, reconnect, verify exactly one canonical result |
| Exclusive pickup | Controller and two replicas compete; only allocated machine reserves |
| Replay safety | Lose acknowledgment and replay; comments and mutations remain unique |
| Conflicts | Concurrent same-field edits and changed requirements reject overwrite/closure |
| Configuration | Revision change reaches agent, survives restart, and queues offline |
| Signals | Pause/resume/stop/restart acknowledgments and duplicate signal replay |
| Connectivity | Heartbeat, EOF, timeout, and reconnect state transitions |
| Web application | Responsive layout, accessible controls, live updates and CSRF rejection |

## Conformance Criteria

Conformance requires every MUST and MUST NOT above to pass the validation matrix. This accepted contract describes the requested iteration; it is not a claim that the preexisting independent-queue implementation conforms.
