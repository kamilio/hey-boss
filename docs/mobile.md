# iPhone companion

`mobile/` contains a private Home Screen web app and a single-user Fly.io hub.
The UI uses Radix UI for accessible controls, `liquid-glass-react` for its floating
navigation, and translucent CSS surfaces. Safari and reduced-motion users receive
the simpler glass fallback; no remote fonts, analytics or third-party push service.

## Deploy and pair

Prerequisites: Node 22.13+, Xcode command-line tools, Fly CLI and an authenticated
Fly account. Fly hosting costs are separate from Apple Developer membership;
Apple membership is not needed for this web app.

```sh
cd mobile && npm ci && npm test && npm run build
cd ..
flyctl auth login
python3 tools/setup_mobile.py
```

To pair another phone later, run `python3 tools/setup_mobile.py --pair-only`.

The setup script creates the private hub, a 1 GB persistent volume, a random
bridge credential, and builds/installs the updated native daemon. It prints only
the five-minute pairing code, never the bridge secret. Use `--app NAME` to choose
a different Fly app. The service must run **one machine**, with `--ha=false`:
SQLite and the push outbox live on its persistent volume. Back up that volume.
The hub must not be exposed before its mandatory `HUB_TOKEN` is configured.

On iPhone (iOS 16.4+), visit the printed HTTPS URL, Add to Home Screen, open it
from there, enter the pairing code and enable notifications in Settings. The
session uses an HttpOnly Secure SameSite cookie; the server stores only its hash.
Pairing codes expire and can be used once. Web Push subscriptions and VAPID keys
persist across process restarts. Subscription endpoints are restricted to Apple.

Configuration is `~/Library/Application Support/hey-boss/mobile.json`, mode 0600.
The native app automatically publishes newly created notifications and questions
when configured. Its durable SQLite outbox retries interrupted publication. A
five-second sync retrieves phone outcomes and acknowledges only after local save.
No agent inventory scans are introduced by mobile sync.

## Quiet notification routing

Default routing is **When I’m away**, with a two-minute input-idle threshold.
Keyboard and mouse activity keep pushes on the Mac; lock/sleep state routes to
the phone immediately after the next heartbeat. A heartbeat older than 30 seconds
counts as offline, allowing phone delivery even when the laptop remains powered on.
The Mac shares only aggregate idle seconds and lock/sleep state with the existing
private hub; it does not record keystrokes, cursor positions, or screen content.
The phone inbox always syncs, regardless of whether push delivery is enabled.

Phone Settings offers **When away**, **Always**, and **Off**, with a
one-, two-, or five-minute away threshold. Settings persist on the hub. While the
Mac is active, queued pushes stay pending without consuming retry attempts.
Answered requests are suppressed. Updates older than 30 minutes stay in the inbox
without an old push; unanswered decisions remain eligible. Three or more queued
items become one summary rather than a burst of notifications.

Mac alert banners disappear after 12 seconds, and update banners after 20 seconds.
Hovering over a card or reading its document pauses the timer. This only hides the
banner: it does not mark the update as read or cancel its review. The menu-bar
**Inbox** shows the unread count and reopens hidden items; reopened items stay until
dismissed. Hidden banners stay hidden after restart. Questions remain until answered
or explicitly cancelled. A sender’s explicit `--autoclose` keeps its existing
completion behavior.

## One decision across devices

The Fly hub is authoritative for outcomes. Both Mac and phone use the same atomic
pending-to-terminal transition. Exactly one answer wins; competing answers receive
409 and the accepted outcome. The Mac adopts the accepted outcome, even if a phone
answer races a local click. Local waiting agents receive that winning answer.
A lost acknowledgement is replayed without replacing the answer.

When configured, a Mac answer requires the hub to be reachable. An unavailable
hub leaves the request pending and re-enables the answer controls for retry. This
avoids accepting conflicting offline answers. A phone answer can be saved while
the Mac is offline and is delivered when the Mac reconnects.

## iOS boundaries

Automatic push routing uses aggregate hardware input inactivity (`IOHIDSystem`
`HIDIdleTime`) and lock/sleep state. The previous CGEventSource sampler returned
more than three days of inactivity while actual HID inactivity was 0.02 seconds.
The Mac now publishes the corrected idle value and an explicit reliability flag
every five seconds. No subprocess, key/event monitor, screen recording,
Accessibility permission or privileged helper is used. Amphetamine keeping the
machine awake does not replace actual mouse/keyboard input in this signal.

Both the live user's and new-installation thresholds are ten minutes. Away
status requires another minute of continuous reliable samples, with no gap
over fifteen seconds. New input or unreliable/missing samples reset confirmation;
an old sample cannot confirm itself merely by aging. Unknown idle data stays
quiet. Lock/sleep or a heartbeat missing for two minutes also enables automatic
phone pushes. Explicit Always and Off modes remain available. The web header
shows Mac active, idle duration or confirming away; details show last input age.
The Mac menu bar preserves the existing speech-bubble icon and adds a small
activity badge: green active, hollow gray idle/unknown, amber confirming away,
purple confirmed away, gray locked/disconnected. Tooltip and menu describe the
state, phone routing and unread count. Updates reuse the existing heartbeat;
the badge redraws only when its state changes and never intercepts menu clicks.

The native benchmark measured 200 HID reads: mean 0.0204 ms, p95 0.0553 ms
on the development Mac (`out/hey-boss-hid-audit.log`).

Automatic-mode pushes wait at least 30 seconds after cloud enqueue, using a
durable retry timestamp. The sender rechecks presence and terminal task state
before each delivery batch, including after an earlier asynchronous push.
Opening/answering on the Mac during that grace period suppresses the queued
push. Already submitted Apple pushes cannot be recalled by this grace period.

Opening an individual alert or update from a notification, inbox card, or its
main link saves an idempotent read receipt and clears the Mac card on the next
sync (normally within five seconds). Opening a question never answers it.
Offline read receipts are stored in IndexedDB and retried when the app reconnects.
Receipt requests have a 2.5-second deadline and do not delay opening the reader;
offline retry stops at the first connection failure rather than flooding attempts.
Foreground API requests have a ten-second deadline, and overlapping refreshes
are coalesced. Connection errors clear after a successful reconnect.
Pull down from the top of Inbox or Activity to refresh. A resisted drag reveals
a release indicator and spinner; short, sideways, cancelled and multitouch
gestures do not refresh. Reader/settings and interactive controls do not capture
this gesture. Repeated pulls reuse the active refresh; an accessible refresh
button also supports pointer and keyboard use.
There is no separate Mark as read action. Activity keeps compact rows with the
accepted result and which device handled it; opening a row shows its document.
Unread alerts and updates remain visible on the Mac until opened or dismissed,
matching the phone inbox. The old automatic 12/20-second banner hiding is removed;
startup restores legacy unread items with hidden banners. Explicit `--autoclose`
still completes the item and propagates that outcome through the shared state.
Activity groups completed requests by local calendar day and shows completion
times. Legacy records without a completion timestamp appear under Earlier.
Light and dark appearances follow the device, including Markdown and glass
controls. Reduced motion and reduced transparency preferences are respected.
Text answers save as local drafts until submitted or the request is handled;
disconnecting clears drafts. Unexpected render errors show a recovery view.

Push previews convert Markdown to readable plain text, retaining lists,
checklists, code text and link labels. Apple does not render rich Markdown in
notification banners. The app's reader renders GFM headings, emphasis, tables,
checklists, quotes, links and fenced code. Raw HTML is ignored and unsafe link
protocols are disabled. Remote images are explicit links so merely reading a
document does not fetch trackers. Full UTF-8 Markdown (up to 1 MiB) is transferred;
inbox and native sync responses use bounded previews/outcomes.

Documents above 16,000 characters parse in a dedicated worker. Lists, tables
and document blocks append in memoized batches while preserving semantic DOM
structure, selection and ordered-list numbering. The worker uses a DOM-free
entity decoder and is permitted only from this app's origin by CSP.

WebKit iPhone 15 browser measurements on the local production build: a
185,019-byte checklist previously caused an 871 ms maximum animation-frame
gap; worker parsing and progressive rendering reduced the observed gap to
56 ms. A 1,036,021-byte document rendered all 28,000 checklist items without
errors. Its full 30-second probe observed occasional frame gaps of 85–155 ms
across batching revisions; this extreme case is not uniformly smooth. These
are browser measurements on the development Mac, not physical iPhone FPS.
Mixed worker-path checks preserved tables, entities and internal footnotes,
started ordered lists at 3, blocked unsafe links and raw HTML, and loaded no
remote images. Screenshot: `output/playwright/mobile-worker-markdown-final.png`.

Home Screen and notification icons reuse Hey Boss's existing SF Symbol identity,
with manifest icons and an Apple touch icon. iOS controls its “from Hey Boss”
attribution and can cache the Home Screen icon from installation.
Browser tabs use a versioned 32-pixel PNG and a standard multi-size ICO fallback,
exported from the existing icon by `tools/export_favicons.py`.
An existing Home Screen install may need re-adding to adopt a changed icon; this can require
pairing and enabling notifications again.

These are real Apple Web Push notifications and can arrive while the app is
closed. Approval buttons and text input appear inside the web app after tapping
the notification. iOS Web Push does not provide native notification action buttons.

Silent withdrawal pushes are not sent. Resolved queued pushes are suppressed;
resolved delivered notifications are cleared when the app refreshes in the foreground or on a subsequent legitimate push.
Opening a delivered notification checks the authoritative state, so a stale Lock
Screen card cannot accept a second answer. Immediate Lock Screen withdrawal is
not guaranteed by iOS Web Push. Real iPhone push delivery requires a device test.

## Checks

```sh
cd mobile && npm test && npm run build
cd ..
xcrun swiftc -O -parse-as-library -D HEY_BOSS_MOBILE_AUDIT \
  hey_boss_daemon.swift mobile/native_audit.swift -o out/hey-boss-mobile-audit
out/hey-boss-mobile-audit
```

The native integration audit starts an isolated local hub and proves Mac-first,
phone-first, replay/ack and offline refusal. The server tests prove atomic answers,
authentication, origin protection, pairing expiry/use, and push outbox handling.

Push sender identity uses `PUBLIC_ORIGIN` (the deployed HTTPS app URL) unless
`VAPID_SUBJECT` supplies a real HTTPS URL or contact email. Placeholder sender
addresses can cause Apple to reject delivery with `403 BadJwtToken`. Delivery
failures log only status, transport code and attempt number, never subscriptions
or private keys.
