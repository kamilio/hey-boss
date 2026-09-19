# Agent discovery notes

Investigated on 2026-09-14 with running Codex and Claude sessions on the Mac.

## Sources inspected

- [abtop Codex collector](https://github.com/graykode/abtop/blob/main/src/collector/codex.rs): process-to-open-rollout matching and support for both legacy events and newer `item_completed` records. Its comments identify the newer schema around Codex 0.149. The new schema includes `UserMessage`, `AgentMessage`, and `CommandExecution`; an agent message with phase `final_answer` can close a turn without a separate `task_complete`.
- [abtop Claude collector](https://github.com/graykode/abtop/blob/main/src/collector/claude.rs): PID-specific session JSON metadata, exact session-ID transcript matching, and caution about guessing the latest transcript when several agents share a project.
- [CodexMonitor](https://github.com/Dimillian/CodexMonitor/blob/main/src-tauri/src/shared/codex_core.rs): uses managed app-server sessions and RPC. Starting another app-server does not give access to another process's live runtime; hey-boss therefore keeps existing agents untouched.
- [Official Codex app-server events](https://developers.openai.com/codex/app-server#events): documents turn and item lifecycle events for a client attached to that server. Persisted rollout events are a separate, version-sensitive interface.

## Implemented discovery

Only same-user running processes named Codex or Claude are candidates. Renderer,
code-mode, and crash helper processes are excluded. `lsof` on macOS or `/proc/PID/fd`
on Linux links Codex processes to exact open session files. A process may own
multiple loaded sessions, including idle ones; session count is not process count.
Historical logs without a process match are never displayed as running agents.

Claude's `~/.claude/sessions/PID.json` is accepted only when PID and process start
time match. Its explicit status supplies Working / Idle / Waiting for input.
The exact session ID locates its transcript. Empty sessions can initially have
no metadata or transcript, so a process is shown with task unavailable until that
information exists. `.key` files and authentication tokens are not used.

Transcripts are read once, then incrementally from complete JSONL record boundaries.
Versioned filenames keep older binaries from replacing the updated parser’s cache. The private display-summary cache is `~/.local/share/hey-boss/agents-cache-v6.json`.
File replacement, truncation, and partial final records are handled. No raw reasoning,
tool arguments, tool outputs, or complete transcripts are retained in the cache or
sent to the Mac. The native scanner reader caps its JSON output at 8 MiB; oversized results become a recoverable discovery error and retain the last snapshot. The latest user request, tool name, and explicit lifecycle state
are used; an open process alone is not evidence of a current task.

Server snapshots ride the existing SSH-forwarded Unix socket every 15 seconds;
they do not initiate SSH, invoke SFT login, or enter the durable notification queue.
The Mac marks a server snapshot stale after 35 seconds without a new receipt,
using its own clock. Last-known server rows remain visible with Offline / stale labels. Local discovery older than 30 seconds uses Discovery stale, so scanner delays are distinguishable from a remote disconnect.

## Activity and grouping

The task remains separate from the latest public assistant update and tool action. XML-prefixed automatic context, AGENTS instructions and compacted-history headers are omitted from summaries, preserving the existing public task. Completed Codex messages replace the previous progress update, and a new turn clears old progress. Typed reasoning/thinking blocks and messages explicitly marked with analysis, reasoning or thinking channels are excluded. Display summaries are capped at 220 characters; repository origins discard user information, query strings and fragments, and normalize default HTTP/HTTPS/SSH ports.

Repository grouping uses normalized origin identity across connected hosts, falling back to local Git common-directory identity when no origin exists. Worktree grouping uses the exact host and worktree path. Switching grouping preserves agent membership and selection. Collapsing a group hides its rows without changing the underlying membership.

Native preview checks include separate two-line limits for task/progress, selected-row contrast, search-empty and no-agent explanations, and an inspector that expands on selection. The audited overview and discovery builds are installed on the Mac and devbox. Runtime evidence and ongoing test scope are recorded in [reliability.md](reliability.md).

Internal Codex guardian/approval-review, review and compaction sessions are excluded using `session_meta.source`, including incremental cached reads. Ordinary spawned worker sessions remain eligible. The scanner does not infer user-facing agent activity from internal approval payloads. Cache version 6 invalidates summaries produced before explicit private-channel filtering; version 5 introduced the internal-session distinction.
