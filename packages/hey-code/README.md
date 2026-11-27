# hey-code

An embeddable Codex/Claude chat window for an agent orchestrator: transcript, streaming replies, tool activity, composer, goals, approvals/questions, and nested subagent chats. It follows T3’s centered 46rem conversation and rounded bottom composer. No chat list, document/change previews, or PR controls.

This package is the conversation **component**, not a session manager. The host reads past or ongoing sessions, publishes snapshots, and implements the actions it owns. Merely opening a transcript never starts or resumes an agent. Controls without a host action are unavailable. Claude goals require orchestration in the host; Claude has no native goal API in the existing Rust runtime.

```js
import '@hey-boss/hey-code';
import { codexTranscript, runtimeEvent } from '@hey-boss/hey-code/transcript';

const view = document.querySelector('hey-code');
view.chat = {
  id: session.id, title: 'Conversation', provider: 'codex', status: 'idle',
  items: codexTranscript(records),
};
view.actions = {
  send: ({ threadId, text, delivery }) => orchestrator.send(threadId, text, delivery),
  stop: ({ threadId }) => orchestrator.stop(threadId),
  goal: ({ threadId, objective, status }) => orchestrator.setGoal(threadId, objective, status),
  respond: ({ threadId, requestId, ...answer }) => orchestrator.respond(threadId, requestId, answer),
};
// On each serialized hey_boss::agent_runtime::Event, in order:
view.chat = runtimeEvent(view.chat, event);
```

Give `<hey-code>` a container height. Import the module once; its shadow root loads the adjacent `hey-code.css`. Keep that asset next to the module when bundling. `theme="light"` or `theme="dark"` overrides the system preference. The `--hc-*` CSS properties customize colors.

See [the types](hey-code.d.ts) for snapshots and actions. Assign a new snapshot after host state changes; treat items as immutable. `send` resolves on host acceptance, not turn completion. Rejections display an error and preserve the draft. In-flight actions retain the original thread ID when the selected chat changes. Drafts use session storage, separated by provider and chat ID. The host owns retries, command deduplication, subscription reconnection, and durable messages.

- History: `codexTranscript(records)` reads rollout records; `codexThread(thread)` reads app-server thread history; `claudeTranscript(records)` reads Claude JSONL/SDK records. Pass parsed records, not file paths. When paging JSONL, pass the page’s absolute record index as the second argument so fallback IDs remain stable. Page histories in the host; set `hasMore` and implement `loadOlder`, prepending older items before resolving. Preserve IDs across pages.
- Live controls: `canSteer: true` offers “Send now” and “Queue next turn.” Otherwise active replies queue. An idle/past conversation sends normally; the host resumes the exact session. Use `readOnly` to explain when an externally owned session cannot be controlled.
- Subagents: supply `subagents` with IDs, titles, statuses, and optional child `chat` snapshots. Expanded children use the same actions with their own thread IDs. `openSubagent` lets the host fetch a child on demand. Runtime events provide native task status; the host supplies child transcripts and reconciles tool-call IDs with provider session IDs.
- Goals: `goal` receives an objective and `active`, `paused`, or `complete`. Publish the actual result back in the snapshot; the UI never invents execution state. Usage is displayed when supplied.
- Requests: `respond` receives either `allow` or answers keyed by question ID. Translate answers to the provider’s native response format in the host. Raw request details are available in a collapsed disclosure.

Run `npm ci --ignore-scripts`, `npm test`, and `npm run test:browser` here. Browser checks use installed Chrome and Playwright WebKit (`npx playwright install webkit` if needed). `fixture.html` is a synthetic host used by those checks, never a live agent connection. Screenshots go under `out/hey-code/` at the repo root.

Layout reference: [T3 Code](https://github.com/pingdotgg/t3code), commit `4358c929b91627ebd3e0f9f7e020f8a1d71b8e87`. Its MIT notice is retained in [T3-LICENSE](T3-LICENSE).
