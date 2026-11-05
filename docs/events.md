# Operational events

`hey-boss events` reads the latest 50 facts available on this machine, including
facts received from peers. Reading does not remove events or claim work.

```sh
hey-boss events types
hey-boss events --since 2h --machine devbox
hey-boss events follow --type 'agent.*'
hey-boss events --type machine.disk_low --since 7d --json
hey-boss events show EVENT_ID
hey-boss events list --after REPLICA:POSITION --limit 100 --json
hey-boss events follow --after REPLICA:POSITION --json
```

Filters include `--project NAME_OR_ID`, `--subject EXACT_ID`, `--machine HOST_OR_ID`
(also `local`), `--type TYPE` (also namespace `.*`), and `--since RFC3339_OR_AGE`.
No project filter means all projects. Machine selects the reporting origin;
for connectivity events the subject is the observed host. Since filters occurrence
time, accepting ages in minutes, hours, or days. Other filters match exactly.

Without `--since` or `--after`, a list shows recent matching events in arrival
order. With either flag it reads forward; `has_more` indicates another page.
Save `cursor` after processing the page. JSON follow outputs one event per line
and a separate `checkpoint` after each batch, even when filters hide all arrivals.
Ctrl+C stops cleanly. `types` lists built-in producers and observed extension types,
with retained counts respecting filters. Zero does not prove fleet-wide absence.

## Contract

Events contain a globally unique `id`, namespaced `type`, payload `version`,
reporting machine `origin`, Unix-millisecond occurrence `time`, optional `project`,
stable `subject`, and structured `data`. Built-in producers use version 1. Unknown
types and payload versions survive replication. The maximum event size is 16 KiB.

Rust producers call `event_log::append(&connection, &event)`. It joins a business
transaction or commits its own. Event and outbox commit together without waiting
for the network. Retrying an identical event is a no-op; reusing its ID with
different content fails. Allocate or derive the ID once per source fact and reuse
the entire event on retry. Existing CRDT transport carries events through peers
and relays. Import never re-emits; crash replay uses the same content hash.

Cursors describe **arrival order in one database**, not occurrence time or a
fleet-wide order. Old events arriving after reconnect remain visible after the
checkpoint. Cursors survive reopening that database but fail on another replica
or ahead of its history. A consumer moving machines must replay and deduplicate
by event ID. Checkpoint after processing; a crash can repeat earlier deliveries.
Changing filters requires a new read to find previously excluded history.

Output reports local coverage and pending publication. Disconnected machines may
have unsent events. There is no global order, broker, consumer group, destructive
consumption, or exactly-once execution of downstream actions.

## Initial sources

| Types | Evidence |
| --- | --- |
| `agent.started`, `agent.finished`, `agent.retry_scheduled` | Managed issue-worker run, harness session, issue, provider, outcome, retry deadline/count |
| `agent.tool_failed`, `agent.input_requested` | Codex/Claude/Pi issue workers: tool/request identity, exit code where available, selected error classification |
| `job.queued`, `job.started`, `job.finished`, `job.skipped` | Saved scheduler transitions, per-machine children and aggregate completion, execution machine/session |
| `machine.disk_low`, `machine.disk_recovered` | Available and total bytes on sampled local volumes |
| `peer.connected`, `peer.disconnected` | Reporter's observation of an outgoing CRDT route |
| `error.observed` | Enabled local collectors: source, group reference, count, selected error classification |

Lifecycle facts commit with saved state; duplicate callbacks do not duplicate them.
Tool/input observations can be skipped with a diagnostic if the database is busy,
so logging contention does not terminate a running agent.
Job execution observations become scheduler events when the authority accepts the
durable execution report. Finished events carry their outcome in `data.state`.

Every minute the fleet service samples the database volume and distinct volumes
used by up to 100 active local issue-worker directories. Low means below 2 GiB
available **or** 5% free; recovery requires at least 3 GiB **and** 7%. A first healthy
sample establishes a baseline. Saved state prevents repeated events on restart.
Coverage excludes other volumes, inode exhaustion, quotas, and changes between
samples. A completely full database volume may be unable to record its own fault.
Connectivity is also sampled once a minute; a disconnected route is an observation
by that reporter, not a global verdict that the remote machine is down.

Error collection remains opt-in. New accepted errors queue selected evidence in
the inbox transaction; fleet maintenance bridges it into this log with retry-safe
IDs. Existing raw history is not backfilled. Raw messages, stacks, request bodies,
prompts, tool arguments and responses are not copied by these producers. Inspect
a group on its collector with `hey-boss errors --project PROJECT show GROUP_ID`.

This layer does not ingest every desktop chat, unmanaged agent session, or proxy
trace. Proxy/application adapters can use existing normalized error ingestion.
AI interpretation, prompts, alerts, and automatic remediation are future consumers;
none run here.

## Retention and rollout

Maintenance retires payloads after 30 days of local receipt. Retirement replicates
and wins over stale replay, so another replica can expire a payload earlier than
30 days after this machine received it. Compact identities and retirement records
remain indefinitely; metadata storage is not constant. Pruned payload space is
reusable, but files need not immediately shrink. Work runs in bounded batches.

Resuming behind an expired position returns `event_gap`. Start a new read to
acknowledge the gap; pages include `expired_through` when older history expired.
Keep aggregates elsewhere if they must outlive retention.

CRDT handshake version 6 rejects older peers before journal exchange because older
binaries cannot index these records. Upgrade every fleet machine together. Local
writes remain durable during rollout. The application/companion protocol is unchanged.
