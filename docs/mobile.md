# iPhone companion

`mobile/` is a private Home Screen web app with an authenticated Fly relay. Inbox contains unread updates and questions, with **Unread / History** views inside it. Issues contains project work. Agents are available through existing saved conversation links, but have no entry in the phone's primary navigation. Notification settings remain under the header gear.

Issues ships the regular web interface directly: project switching, hidden-project restoration, text/tag/assignee filters, open/blocked/closed/deleted states, queue ordering, full issue details, Markdown editing and preview, drafts, comments and resolution, assignment, lifecycle actions, transfer, activity, progress, subtasks, PR links, artifacts, and attachments. Project documents and mindmaps are secondary project resources. The same source assets supply desktop and phone behavior; the phone does not have a separate issue action implementation.

## Storage and availability

**No durable user data is stored on Fly.** The authoritative issue database, documents, attachment files, and retry journal stay on the supervisor and fleet devices. Issue browsing and mutations go through an in-memory request bridge to the supervisor. Requests disappear when completed, disconnected, timed out, or stopped. The server sets `Cache-Control: no-store`; it does not put issue responses or attachments in a service-worker cache.

Fly holds notification routing and delivery state in RAM. Its durable private checkpoint, `mobile-relay.json`, lives beside the supervisor's `issues.db`. Successful pairing, push-subscription changes, notification answers/read receipts, settings changes, and native publication are acknowledged only after the supervisor atomically saves and fsyncs the checkpoint. That file includes pairing hashes, push subscriptions, notification bodies/outcomes, routing preferences, and stable VAPID keys. It is mode 0600: back it up locally and never print it or put it in Git.

A restarted relay restores that local checkpoint before accepting phone changes. The supervisor must be online to browse issues and acknowledge durable notification actions. If it cannot respond/save, the phone gets a retryable connection error. An uncertain issue mutation may already have completed locally: retry the same content with the same request ID; the authoritative store deduplicates it. Explicit conflict/validation errors preserve editable content. A disconnected or timed-out HTTP request does not reverse a change already committed on the supervisor.

This policy removes the old Fly SQLite volume and the old offline cloud queue. Unsaved issue drafts on HTTPS remain in the current tab, matching the regular private mobile web interface; keep the tab open while reconnecting. Notification answer drafts and offline read receipts retain their existing local browser storage behavior. Existing issue-creation clients are supported through the checkpointed legacy transport, but the new Issues screen uses the regular editor.

See [hosting costs and storage boundaries](mobile-costs.md) for the resource inventory, current rates, expected budget, and billing limits.

## Deploy and pair

Prerequisites: Node 22.13+, Rust, Xcode command-line tools on the Mac, Fly CLI with an authenticated account, and the installed fleet supervisor. The legacy provisioning helper still uses the existing Python tooling; the relay and new supervisor implementation are JavaScript and Rust. Apple Developer membership is not required for this web app.

```sh
cd mobile
npm ci
npm test
npm run build
cd ..
hey-boss upgrade --source "$PWD"
fly deploy --config mobile/fly.toml --dockerfile mobile/Dockerfile --ha=false
```

Keep exactly **one** relay machine. Pairing and active transport state are coordinated with one supervisor; multiple independent relay machines are unsupported. `auto_stop_machines = "off"` keeps pushes and bridge requests available. Deployments do not create volumes. Existing installations must migrate their legacy hub checkpoint to the supervisor before detaching/removing their Fly volume; verify pairing, stable VAPID keys, notification outcomes, and a process restart before deleting the legacy copy. A detached volume still incurs charges.

The existing provisioning helper `tools/setup_mobile.py` no longer creates a Fly volume. It configures the mandatory `HUB_TOKEN` and local `mobile.json` without printing bridge secrets. Upgrade/install the fleet supervisor before first use. `--pair-only` prints a five-minute single-use pairing code for another phone. `mobile.json` is mode 0600 beside the authoritative issue store and holds the relay origin and bridge credential.

On iOS 16.4+, open the HTTPS app, **Add to Home Screen**, launch it there, enter the pairing code, and enable notifications in Settings. The session uses an HttpOnly Secure SameSite cookie, and only its hash appears in the local checkpoint. Subscription endpoints are restricted to Apple. Pairing and VAPID keys survive relay process restarts through supervisor restoration. The local native SQLite outbox retries interrupted publication, and phone outcomes are applied locally before acknowledgment.

The regular `/issues` route and older root issue hashes both open the same issue browser. Copied phone issue links also work with `hey-boss lookup`. Existing `/agents/session`, `/project-resource`, and artifact links remain available.

## Notifications and safety

Automatic routing defaults to ten minutes without mouse or keyboard input, confirmed for another minute of continuous reliable samples. Lock/sleep or a two-minute heartbeat disconnect also enables phone pushes. Missing or unreliable idle readings stay quiet. **Always** and **Off** override automatic routing. Only aggregate inactivity and lock/sleep state are shared; no keys, app contents, or screen recording are captured.

Automatic-mode pushes wait at least thirty seconds after enqueue. The sender checks current task and presence state before each batch; three or more queued items become one digest. Answered requests do not push. Updates older than thirty minutes remain in Inbox without generating an old push. Failed delivery uses bounded retry/backoff; expired subscriptions are removed.

Mac and phone answers share one atomic pending-to-terminal transition; one answer wins and competing answers receive the accepted outcome. A successful cloud response now also means the supervisor has saved the relay checkpoint. Opening alerts and updates records an idempotent read receipt; opening a question never answers it. Pending questions/reviews cleared from Inbox are cancelled without an answer or approval. History retains the accepted result and handling device.

Delivered Apple pushes cannot be recalled immediately. Resolved queued pushes are suppressed, and delivered notifications are cleared on foreground refresh or another legitimate push. iOS does not offer approval buttons or text input in a Web Push banner; those controls appear after opening the app. Real physical iPhone push delivery requires a device check.

Inbox pull-to-refresh supports deliberate vertical drags from the top and has an accessible refresh button. Reader/settings/interactive controls do not capture the gesture. Light/dark appearance follows the device; reduced motion/transparency preferences are respected. Large notification Markdown uses worker parsing and progressive rendering. Raw HTML and unsafe link protocols are blocked; remote images are links rather than automatic tracker fetches.

Application logs omit payloads, credentials, subscriptions, and document bodies. Push failure logs include only status, transport code, and attempt number. Fly and Apple still process HTTPS/push traffic and ordinary transport metadata; the policy concerns durable user-content storage on Fly, not zero cloud processing.

## Verification

```sh
cd mobile
npm test
npm run build
cd ..
cargo test --lib fleet::native::mobile
cargo test --test lookup
```

Regression coverage includes pairing restoration, accepted answers after restart, durable acknowledgment barriers, authentication/origin checks, transient request cleanup, bounded requests, notification routing, atomic competing answers, attachments, and stable mutation retries. Visual verification uses synthetic issues in isolated databases and checks phone/desktop layouts, light/dark modes, filtering, state browsing, Markdown preview, editing, comments, attachment upload, activity, subtasks, and reconnect behavior. Close only task-owned browser sessions and remove temporary previews/reports after verification.
