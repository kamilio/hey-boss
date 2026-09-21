# Mobile hosting cost and storage

Checked on 2026-09-21 against [Fly resource pricing](https://fly.io/docs/about/pricing/), the app's machine configuration, volume inventory, and IP inventory. These are resource estimates, not an account invoice or a guaranteed spending cap; other apps, organization plans, tax, credits, and the account's bandwidth plan can change the bill.

## Current resource budget

`hey-boss-mobile-kamil` runs **one shared-cpu-1x, 256 MB machine in Chicago (`ord`)**, continuously. Shared IPv4 and IPv6 are allocated; there is no paid dedicated IPv4. Always-on operation is intentional for notification delivery and the supervisor bridge. There is no autoscaler and deployment uses `--ha=false` to avoid adding a second relay.

| Resource | Expected charge |
| --- | --- |
| One Chicago 256 MB shared machine | $2.43 per 30 days; about $2.51 for 31 days |
| Persistent Fly volumes | $0 after removing the legacy 1 GB volume |
| New volume snapshots | $0; no volume remains to back up |
| Shared IPv4 / IPv6 | $0 |
| TLS certificate | Usually $0 within the organization's first ten single-hostname certificates; otherwise $0.10/month |
| Internet egress under granular North America pricing | $0.02/GB; 1 GB = $0.02, 10 GB = $0.20, 100 GB = $2 |
| Cross-region private traffic under granular North America pricing | $0.006/GB; no cross-region app dependency is configured |
| Apple Web Push | No separate Apple Developer membership or per-push fee |

**Budget roughly $3/month for this app at light personal usage.** This is not a hard maximum. Fly bills actual egress, and continuous push/bridge connections prevent relying on automatic shutdown savings. Inbound traffic is free. The bridge's regular heartbeats and changed-state checkpoints contribute outbound bytes; checkpoint payloads transfer only after a durable relay mutation, not on every heartbeat. Large documents and attachment downloads can dominate traffic. No new database, Redis, paid messaging provider, dedicated IP, or additional permanent machine is introduced.

The previous 1 GB volume cost $0.15/month even while detached. Snapshot storage is $0.08/GB-month after the organization's first 10 GB of snapshots; deleting a volume may leave retained snapshots until Fly expires them. Build machines/build services can contribute deployment-time charges; the estimate above covers the running app, not unrelated builders or other apps. Fly's dashboard **Billing / Cost Explorer** is the source for the actual organization invoice and whether granular or older bandwidth pricing applies. An older bandwidth plan may include allowances and uses its own rates; no plan switch is performed here.

Inspect resource count after deployment:

```sh
fly machine list --app hey-boss-mobile-kamil
fly volumes list --app hey-boss-mobile-kamil
fly ips list --app hey-boss-mobile-kamil
```

Unexpected cost changes to look for are an extra permanent machine, more RAM, a dedicated IPv4, a recreated or detached volume, accumulated snapshots, a paid organization plan, a running builder, or sustained large outbound downloads. Fly budget alerts are notifications, not a hard spending cap.

## Where data lives

The authoritative issue database, artifacts, attachments, issue history, and mutation retry journal remain on the supervisor and fleet devices. Shared web issue requests cross the authenticated bridge on demand. Their payload and result exist only for an in-flight HTTP request on Fly; they are removed when the request completes, disconnects, times out, or the service stops. Fly does not write a user database, document, attachment, or relay checkpoint to its disk.

Notifications, device pairing hashes, push subscriptions, routing preferences, and stable VAPID keys are held in Fly RAM while serving the app. Their durable checkpoint is **`mobile-relay.json` beside the supervisor's `issues.db`**, created atomically with mode 0600 and fsynced before acknowledging successful pairing, answers, read receipts, settings, or native publication. It contains private control material and notification content: back it up with the local issue/notification stores, and never add it to Git or print it. Fly's `HUB_TOKEN` remains a deployment secret, not user issue content.

A relay restart restores that checkpoint from the supervisor. Cold startup and durable phone mutations require the supervisor online. If it cannot save the change, the UI receives a retryable failure instead of claiming success. Accepted issue edits are already saved in the authoritative issue store; an uncertain retry uses the same key and returns the original result. A timed-out or disconnected write may still finish on the supervisor, so retry the same action instead of creating a second one.

This is **no durable user data on Fly**, not zero cloud processing: HTTPS requests, notification delivery, and brief RAM buffers necessarily pass through Fly and Apple. Application logs omit payloads, credentials, subscriptions, and document bodies. Fly/Apple still have their ordinary transport metadata. Fly image layers contain application code and static assets, not user content.
