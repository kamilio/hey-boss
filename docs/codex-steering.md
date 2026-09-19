# Codex controls in the Agents overview

Expand a Codex row and click **Connect controls**. The inline panel displays the
saved objective and goal status, with **Re-enable goal** or **Pause goal**.
Re-enabling sends `thread/goal/set` with status `active`; pausing sends status
`paused`. Neither sends an objective or budget, so the server retains the saved
goal and accounting. No goal is cleared. If Codex refuses to reactivate a
terminal/budget-limited goal, the rejection is displayed instead of creating a
replacement or resetting its budget.

When the thread accepts input and has an active turn, write an instruction and
click **Send instruction** or press **⌘Return**. Acknowledgement clears the draft;
errors preserve it. Idle sessions are not automatically resumed or started.
If a connection drops during an action, refresh controls and check its state
before retrying: delivery may have happened before the acknowledgement was lost.

## Configure the owning server

Each machine keeps its own `~/.config/hey-boss/agent-control.json`:

```json
{
  "sockets": ["/absolute/path/to/owning-app-server.sock"]
}
```

Up to four owning endpoints are supported. Every configured endpoint must be
reachable so ownership can be verified; an ambiguous thread loaded on two
servers is rejected.

For future managed terminal sessions, run a persistent Codex app-server:

```sh
codex app-server --listen unix:///absolute/path/to/owning-app-server.sock
```

Connect the terminal client to that same server:

```sh
codex --remote unix:///absolute/path/to/owning-app-server.sock
```

Configure that socket in the file above. The server must remain running. This
integration performs a WebSocket handshake at `ws://localhost/rpc` over the
Unix socket using the standard Tungstenite implementation. The CLI proxy
forwards raw bytes and does not add that handshake; it is not used.

Remote rows use the existing SSH host configuration. Install the updated
Hey Boss CLI and control config on that host. Requests and instructions travel
via SSH stdin, never interpolated into shell commands. Host and session identity
are captured when the panel is created. A remote CLI without `agent-control`
shows a connection error rather than falling back to local control.

## Current sessions and limits

The current standalone CLI sessions have terminal stdin, and the default managed
control socket was absent during investigation. Their stored transcripts are
read-only discovery evidence, not a control connection. The Desktop app's private
IPC socket is not a verified app-server endpoint. Existing unmanaged sessions
cannot be controlled by this feature until their owning process provides a
supported endpoint. Starting a separate server and resuming their transcript
would not control the existing process, so Hey Boss never does this silently.

The adapter verifies the exact thread with `thread/loaded/list` and
`thread/read` before any change. Steering also checks the runtime input
capability and exact active turn ID, then requires the matching `turn/steer`
acknowledgement. Goal controls require an existing saved goal. Requests have
bounded deadlines and response sizes, and are not automatically retried.
No approval/tool-call notifications are answered by this observer connection.

Controls connect only when clicked, then refresh on explicit user actions.
They add no background polling or agent scans. Drafts survive row collapse,
search changes, and snapshot refresh within the open app process.

CLI interface (JSON stdin/output):

```sh
printf '{}' | hey-boss agent-control --thread THREAD_UUID inspect
printf '{}' | hey-boss agent-control --thread THREAD_UUID enable-goal
printf '{}' | hey-boss agent-control --thread THREAD_UUID disable-goal
```

Official reference: https://learn.chatgpt.com/docs/app-server#manage-a-thread-goal
and https://learn.chatgpt.com/docs/app-server#steer-an-active-turn.
