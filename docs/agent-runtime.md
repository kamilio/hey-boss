# Owned agent runtime

`hey_boss::agent_runtime` is the provider-neutral control API. `AgentSession`
starts and owns one process group. `Provider::{Codex, Claude, Pi}` selects the
protocol, independently of issue queues, worker settings, task labels and fleet
routing. Existing issue workers still launch Codex. Their JSONL transport and
executable discovery now use the shared implementation.

| Operation | Codex | Claude Code | Pi |
| --- | --- | --- | --- |
| Start/resume | app-server thread/start or thread/resume | stream-json with exact --resume ID | RPC with exact --session file |
| Activity | item events | SDK messages and partial stream events | message/tool RPC events |
| Completion | turn/completed | parent result, accounting for tracked delegated tasks | agent_settled, after retries and queued work |
| Steering | expected-turn direct input | queued next user turn | before next model call |
| Interrupt | turn/interrupt | SDK interrupt; stop when queued input exists | clear_queue then abort |
| Tool approvals | command/file/network and turn-scoped permissions | can_use_tool; original input, no permanent grant | no native permission broker |
| Human input | unsupported requests exposed as Input | unsupported controls exposed as Input | extension select/confirm/input/editor, including cancellation |
| Native goal | get/set saved native goal | none | none |
| Native output schema | per-turn or launch default | --json-schema at launch | none; use prompt format |

Capabilities describe actual provider behavior. A queued Claude user message is
not acknowledged direct steering. The caller receives the delivery mode. Claude
queued messages have distinct local turn guards; Pi internal tool-loop turns are
not client turns. Native goals are explicitly Codex-only; callers can implement
continuation independently of native provider support.

Persist the entire `SessionRef`: provider, ID, and Pi's exact session file. Pi
resume verifies the file header and ID before spawning. All adapters reject a
provider mismatch or a server that resumes a different ID. None falls back to
the latest conversation or silently starts a fresh session. Claude's session
reference arrives in its first SDK session message. Pi's session file may not
exist until its first completed message; later Session events update the reference.

`inspect` drains observed events and refreshes Pi state. `state` returns the last
observation even after disconnect. Controls require the active turn guard;
Codex additionally enforces that guard at the server. This API only controls its
owned children, never arbitrary discovered terminal sessions. The existing
configured-socket Codex control API keeps its original ownership protections.

Approvals and input requests remain pending until an explicit response. Boolean
approval accepts only supported request-scoped decisions, and Codex permissions
are limited to the current turn. Unknown requests, cancelled request IDs, invalid
extension choices and repeat responses cannot grant access. Raw provider events
remain available through `Other`; integrations must not interpret them as approval
or task completion. Child environments remove inherited agent/session identity.
No permission bypass flags are added.

Executable lookup preserves Codex's existing search paths and adds equivalent
Claude/Pi lookup. `HEY_BOSS_CODEX`, `HEY_BOSS_CLAUDE` and `HEY_BOSS_PI` require
absolute executable paths. Launch accepts an explicit binary and environment for
embedding and isolated tests. Transport uses LF-delimited JSONL, bounded records
and queues, nonblocking writes with a deadline, and correlated acknowledgments.
Disconnect, malformed records and uncertain acknowledgments never imply success.
After an uncertain action, review the last state and saved session before stopping
and resuming; do not automatically replay the action. Stop/drop terminate and reap
the owned process group, including tools surviving their parent.

## Verification

`cargo test --locked --test agent_runtime --test issues_worker` checks the three
protocols, resume identity, guarded/queued steering, explicit approval decline,
Pi settled retries and extension cancellation, malformed streams, and existing
Codex worker lifecycle behavior. The fixture never calls a model.

The opt-in real CLI test makes two small text-only model requests per provider:

```sh
cargo test --locked --test agent_runtime real_agents_complete_and_resume_the_exact_conversation -- --ignored --nocapture
```

It verifies successful completion, exact session reuse and preserved conversation
content. It passed locally with Codex 0.155.1, Claude Code 2.1.278 and Pi 0.84.4;
use the installed CLI versions reported by the test environment when reproducing.

## Provider-neutral goals

`ManagedGoal` supplies continuation without requiring a native goal service.
Create it with the full objective, call `start`, then feed each ordered session
event to `observe`. A completed turn containing a valid JSON `status`/`summary`
report completes or blocks the goal. Ordinary turn completion sends a continuation
in the same session; failed/interrupted turns block it. Approvals/input still go
to the embedding application's explicit decision handler. This controller does
not also activate Codex's automatic native goals, avoiding duplicate continuation.

`pause` records paused intent before interrupting, so late events cannot reactivate
it. `start` explicitly re-enables a paused/blocked goal; complete goals reject
restart. Serialize the controller with serde and persist it alongside the entire
session reference. On process recovery, launch the exact reference and call
`resume`; the old turn guard is discarded while the objective and completed-turn
accounting are retained. A goal attached to one owned process cannot control a
second process. `last_usage` retains the provider's last usage payload; it does
not invent a cumulative token count across differing provider accounting formats.
