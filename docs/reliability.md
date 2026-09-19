# Reliability work

The September 14 preview crash occurred in `auditAgentOverview`: startup ran an audit that force-wrote a screenshot using a source-relative path. Launching through an app bundle changed the working directory. Preview startup now constructs fixtures without running audits or writing files. Screenshot export is optional and reports failure.

The production daemon and companion LaunchAgents were unloaded during investigation. Production must remain stopped until the replacement has passed validation. The remote queue continues to retain notifications independently.

## Failure handling

- Incoming notification requests validate required fields, expiry and link schemes. Unknown commands and task IDs return an error response.
- JSON decoding failures return an error; socket readers have an 8 MiB limit, a 10 second total deadline and at most 16 concurrent readers.
- SQLite open, preparation, reads and writes throw recoverable errors. Writes are acknowledged only after successful persistence.
- Dismissal uses a transaction. Failed writes roll back before waiter replies or UI removal.
- Damaged pending records are retained and skipped with a diagnostic, rather than deleting history or terminating the application.
- Missing launch configuration produces a diagnostic and a clean startup return.
- Weak UI callbacks, links and table row indexes are guarded. Cocoa coder initializers fail rather than trap.
- Attributed-text ranges use UTF-16 lengths, including tasks containing emoji.

## Validation

The Swift audit exercises invalid commands, unknown tasks, missing fields, unsafe links, invalid expiry, corrupted history, unavailable database paths and read-only storage, followed by the existing notification/answer/history/overview checks. The audit uses a temporary directory independent of its launch working directory.

These checks address known application failure paths. They do not establish that OS/framework failures, resource exhaustion or all future changes can never crash. New regressions must remain part of CI.

## Verified installation state

The hardened macOS daemon and CLI were installed atomically with backups under `out/daemon-before-reliability` and `out/cli-before-reliability`. Both LaunchAgents remain unloaded. The controlled preview was stopped after light/dark and repository/worktree checks; no new diagnostic report appeared.

All Rust tests passed (13 library, 13 binary, VPN retry integration and durable broker integration), Clippy passed with warnings denied, and formatting/diff checks passed. With the daemon stopped, the installed CLI now returns exit code 1 and `Connection refused`, with no panic. The milestone notification therefore was not delivered.

Native screenshots are under `out/overview-screenshots`: `03-repaired-repository-light.png`, `04-worktree-light.png`, and `05-worktree-dark.png`. The broader eight-hour refinement goal remains active.

## Follow-on preview iteration

Added membership/selection invariants across all three grouping modes, collapsed-group membership checks, empty-search checks and out-of-range table callback checks. The latest Swift audit passed from `/tmp`. Fifteen Rust library tests and Clippy passed for activity/privacy/repository-origin refinements.

Screenshots 06–11 record the task hierarchy regression and correction, selected-row contrast, filtered/no-agent states, many sessions in a compact window, and the collapsed inspector. Screenshot 06 showed unintended single-line truncation; screenshot 07 confirms restored wrapping and selected contrast. Screenshot 10 exposed the blank inspector region; screenshot 11 verifies its collapse.

Automatic approval review rejected one preview launch because constraint source had changed after the preview build. The source was audited, the preview rebuilt from that exact source, and the subsequent launch succeeded. The preview was stopped after checks. No production service was re-enabled.

## Live-data inspection

Replaced the selected-agent label with a native selectable, scrollable text view: task and public progress use primary typography; paths and session evidence use secondary typography. The audit confirms the full task/session remain in the detail document.

Extreme or non-finite activity timestamps are rendered safely without unchecked integer conversion. Remote snapshot freshness still uses Mac receipt time, allowing server clock skew independently of activity timestamps.

Screenshot 12 verifies the scrollable inspector. Screenshot 13 exposed internal approval-review sessions in live data. Codex metadata identified the unwanted session as `subagent.other = guardian`; exclusion is now cached explicitly and internal text is not summarized. Screenshot 14 confirms the corrected live dark overview. The live scan went from 25 to 24 rows and reported zero internal approval rows.

Seventeen library tests, Clippy and the latest Swift audit passed. The full Rust suite also passed before the guardian-only discovery change. The updated scanner is installed; latest native UI changes remain staged. Preview and production are stopped, with no automatic SSH service enabled.

## Bounded transcripts and durable rejection handling

Transcript readers retain at most 4 MiB for one JSONL record. Larger records are streamed past without retaining their contents; the next complete record and partial-line offset remain usable. Guardian exclusion is tested across cache serialization/reload.

Broker socket/setup/serialization failures are recoverable. An immediately closed client on macOS can cause socket timeout setup to return EINVAL; that client is dropped without stopping the broker. Invalid creation requests are rejected before entering the durable queue. A legacy item rejected by the Mac is retained as terminal `error`, so later notifications continue replaying. Both offline restart/answer persistence and legacy-rejection ordering integration tests passed, along with 18 library tests and Clippy.

`tools/soak_daemon.py` loads only `local.hey-boss-reliability-test`, uses separate private state and completed synthetic records, never sends `overview` or creates notifications, and unloads its test service on completion/failure. It checks answers, cancellation, malformed requests, missing tasks, liveness and RSS. A 30-second smoke run passed 66 requests with no failures; peak measured RSS was 59,984 KiB.

A longer run started at 04:00:04 UTC and is scheduled through the eight-hour goal deadline, 11:18:45 UTC. Frozen binaries, source hashes and per-minute samples are under `out/daemon-soak/a99d70a5-2c46-476a-ab41-8420835eb40d`. This run is still active; its outcome is unproven until its terminal report and cleanup are inspected. Production and automatic SSH services remain unloaded.

## Connection settings and startup cleanup

Screenshots 15–18 verify the native connection settings sheet, inline validation, preview-only Save and dark appearance. Preview Save validates without changing configuration, launching services or connecting SSH; the real configuration hash stayed unchanged. Production Save runs configuration asynchronously with a 30-second timeout and guards stale completion callbacks.

A failed SQLite initializer now relies on its destructor for exactly one close. A redundant catch-path close was removed after verifying Swift runs deinit when initialized state subsequently throws. The audit repeats incompatible-schema opens to exercise that cleanup path.

CLI creation validation now returns normal errors for blank metadata, invalid link schemes/labels and nonpositive, nonfinite or extreme autoclose durations. CLI, server broker and daemon cap autoclose at 31,536,000 seconds (one year), preventing extreme finite values from reaching native timer arithmetic. Tests cover NaN, zero, huge finite values and invalid links; the daemon audit also rejects huge durations.

The full Rust suite passed after this change: 18 library tests, 14 binary tests and three transport integration tests. Production and preview remain stopped while updated native audits/builds are verified. The ongoing soak uses a frozen earlier core-runtime binary, so its results do not claim coverage of later settings or schema-failure changes.

The updated native audit passed from `/tmp`, including repeated schema failures and huge expiry input. Clippy passed with warnings denied. Optimized daemon and CLI replacements were installed atomically with backups, while production and automatic connection stayed unloaded. Installed CLI smoke checks returned ordinary errors for blank project and `--autoclose 1e308`.

Screenshots 19 and 20 verify the selected worktree inspector and repository collapse. Collapsing the selected group clears its hidden selection and returns the inspector to its compact empty state. The isolated soak reported 2,915 requests with zero failures at 04:22:07 UTC; this is an interim sample, not a final soak result.

## Quiet ScaleFT authentication

The devbox SSH alias invokes `/usr/local/bin/sft proxycommand`. SSH `BatchMode=yes` does not control that proxy's browser behavior. Inspection of installed ScaleFT 1.103.2 found its explicit diagnostic `not launching browser as SFT_NO_BROWSER is set`. Unattended SSH now sets `SFT_NO_BROWSER=1` and `SSH_ASKPASS_REQUIRE=never`, retaining strict host verification and bounded attempts. A command-construction regression checks those settings. One 15-second-bounded readiness probe succeeded without login UI. The Mac automatic connector remains unloaded during crash validation.

## Server request limits and offline cancellation

The server now caps active client handlers at 64, releases capacity when a handler fails, and uses fallible thread creation. Its 8 MiB request bound has a total 10-second deadline; trickling bytes cannot reset that deadline. Regression checks cover deadline expiry, malformed input and capacity recovery. Durable queue replay/answer persistence integrations still pass.

A queued question can now be hidden while disconnected. The server durably marks it `cancelled` immediately, with no result; `wait` returns cancellation after restart and the question remains ineligible for replay. Previously an offline hide returned `pending` and allowed the question to appear on reconnection. The new integration test covers this before/after behavior.

The updated devbox service was observed active with MainPID 2296327 after the initial server update. Its skill SHA-256 matched canonical source and Mac: `6e1b7beed00743766bf5fe5652e402552c414329340b581571cb92bd9bd6cbdf`. Scanner evidence showed 16 rows, 13 with Git context and no warnings. Later request-limit/cancellation changes remain staged until the next verified installation.

## Bounded icon decoding

Raster icon loading checks ImageIO pixel properties without caching full-resolution pixels, rejects dimensions above 16,384 or pixel area above 16,777,216, and creates a thumbnail at most 128 pixels before native drawing. Compressed input is capped again after reading to guard a file-size race. PDF stays vector-backed and is rendered into the bounded 128-pixel bitmap.

The new audit builds a valid 4097×4097 grayscale PNG using zlib; its compressed size is only a few kilobytes. ImageIO confirms its dimensions, and the loader rejects it before thumbnail decoding. Existing snapshot aspect/alpha/storage tests and PDF acceptance pass. Initial audit failures were a malformed synthetic PNG and recognition of PDF as a source with no raster properties; both were resolved before installation. The optimized complete native audit passed from `/tmp`.

Screenshots 21–23 check many-session dark appearance, keyboard selection and scrolling while retaining the selected inspector. They exposed lexical PID ordering; directory/host names now use native natural comparison and PIDs use numeric ordering, retaining working-first behavior. Scanner timeouts also escalate to SIGKILL after two seconds if termination is ignored.

The first latest-build runtime smoke could not create its 106-byte Unix-socket path. It unloaded cleanly with zero requests and no daemon log. The harness now rejects oversized paths before loading; a shorter `out/smoke` path started successfully with PID 28780 under `local.hey-boss-reliability-current`. It preserves the original long soak under its original label, and records binary/harness hashes automatically.

After the second devbox update, the service was active with MainPID 2303842. A live offline smoke asserted no bridge existed, queued one synthetic question, hid it, and received durable `cancelled` from `wait`, with no result. The synthetic question was never delivered. Latest Mac CLI includes request limits, quiet ScaleFT and offline cancellation; the full suite (18 library, 18 binary and four transport integration tests) and Clippy passed.

Latest-core runtime smoke passed 473 requests in 212 seconds, zero failures, peak RSS 87,312 KiB, and unloaded its separate LaunchAgent successfully. Its frozen binary includes bounded icon loading but precedes the natural-order/scanner-timeout-only refinement. That refinement passed the full native audit and optimized builds. Updated native binaries are installed with backups; production remains unloaded.

Screenshot 24 confirms numeric PID ordering (100, 104, 108, 112, 116, 120) in the many-agent dark case. Preview was stopped after capture; production and automatic connector remain unloaded. The original soak continues independently toward 11:18:45 UTC.

## Compact layout iteration

Screenshot 25 exposed cramped directory headers and filenames splitting awkwardly across lines at 1000×620. Directory and action cells now use separate primary/secondary labels: branch remains one line, action has two lines and its age remains separate. Per-column minimum widths preserve readable labels and state. Screenshot 26 confirmed that hierarchy improvement but exposed horizontal overflow under uniform column resizing. Native sequential column resizing now absorbs reduced width across available columns. The complete native audit passed after both refinements.

Screenshot 27 checks the compact dark empty state: no stale agent rows, zero-agent guidance and visible connection state. It precedes the sequential-resize correction and is retained as iteration evidence.

Screenshot 28 shows sequential column resizing alone did not remove overflow. Inspection found the table document view had no width autoresizing mask, so its frame did not follow the resized clip view. Width autoresizing is now enabled; this correction remains subject to a fresh compact screenshot before being called verified. The native audit passed after that source change.

Screenshot 29 confirmed the remaining overflow after width autoresizing. Temporary native-fixture measurements showed the minimum column sum 870 resulted in an inset table width of 987 within a 950-point clip view. The budget is now reduced to include AppKit inset spacing, while retaining the 240-point task minimum and readable directory/action labels. Minimum window width now matches the 1000-point compact fixture. Temporary diagnostic instrumentation was removed; rendered verification is still required.

Screenshot 30 reduced the compact overflow to a few points. The live vertical scroller consumes width absent from the hidden fixture's 950-point clip view, so the minimum column budget reserves that additional margin. Unneeded scrollers now auto-hide. This source refinement passed the full native audit; fresh rendered verification is pending.

Screenshot 31 verifies the compact layout fits with all six columns and no horizontal scrollbar. The scroll area exposes only vertical scrolling in native AX state. Directory/action labels and their branch/age lines remain readable.

Screenshot 32 exposed a wide-layout defect: sequential resizing gave spare width to Host while tasks remained narrow. Task and progress is now the only column eligible for automatic resizing; supporting columns retain their readable widths and can still be resized manually. The full native audit passed; compact/wide rendered checks remain pending for this final resize policy.

Screenshots 33 and 34 prove the flexible-task policy at wide and compact sizes. Wide rows display full task/progress text; compact rows keep all supporting columns and only vertical scrolling. Auto-resizing eligibility is restricted to Task and progress; all columns remain manually resizable.

Saved notification timers are now bounded independently of request validation. Invalid saved autoclose values produce no timer and retain a manually dismissible card; extreme/nonfinite saved expiry cannot create an unbounded interval. The audit covers normal remaining time, huge/infinite expiry, expired values and invalid durations. Saved grouping indexes are clamped to existing segments. Wrapped task/progress/action labels indicate shortened content with last-line ellipses. The complete native audit passed after these changes.

## Controlled production restoration

The exact restoration candidate passed 187 requests in an 82-second isolated smoke, zero failures, peak RSS 95,216 KiB, and unloaded cleanly. Screenshot 35 confirms compact ellipses while retaining wrapping and column fit.

Before restart, a read-only SQLite backup preserved 1,810 history rows under `out/production-restore-20260915T050113Z`, with installed binary/source hashes. Production was bootstrapped using the verified replacement. Launchd reported PID 38719, one run and no exits; `hey-boss overview` returned `ok`. The VPN-aware connector was restored with the existing config; its SHA-256 stayed `f937b4711346a2fbfed56a019eeca65335ba87828d727a76a737932c9074882d`. Connector PID 51408 reached `connected` with zero failures. The one brief restoration milestone alert auto-closed and its stored status became `ok` while production retained the same PID.

Native automation could not identify the unbundled production daemon by app name. Automatic approval review rejected opening its executable path because that could start a duplicate instance and disrupt the running launchd service. No duplicate was launched. Production checks use existing launchd/socket state, while native visual evidence comes from the matching audited preview, including live-local scanner fixtures.

A read-only devbox check observed the bridge present, 13 durable queue records, 12 mapped to Mac, two terminal records (one overlaps a mapped record), and zero awaiting replay. Production retained PID 38719 and one run with no exits after nearly six minutes; connector remained connected with zero failures. No extra test question was sent to the user. The original long soak reached 8,734 requests with zero failures at 05:06:16 UTC and remains active.

## Read-only overview inspection

Added `hey-boss overview --json` to inspect existing native state without opening an executable or creating another daemon. The snapshot contains local and received remote data, filtered rows, grouping, selection and connection state; it does not activate a window, refresh discovery or probe the network. Encoding is fallible and capped at 3 MiB. Server controls forward immediately and return a recoverable error offline, never entering the durable notification queue.

The complete native audit passed from `/tmp`, including snapshot field/count checks and unchanged selection/window visibility. The full Rust suite passed (18 library, 18 binary, five integration tests), and Clippy passed with warnings denied. An initial integration fixture omitted the required `sync` field; this was corrected and the focused and full suites passed. Optimized native and CLI snapshot candidates are built but not yet installed; production continues to run the verified restoration binary. At 05:13 UTC the original frozen core soak had passed 9,658 requests with zero failures.

## Verified snapshot installation and current-build soak

At 05:15 UTC the connector and production LaunchAgents were unloaded for a controlled replacement. Previous binaries, hashes and a read-only SQLite history backup were preserved under `out/snapshot-install-20260915T051459Z`. Audited native and optimized CLI candidates were replaced atomically, then the existing production socket LaunchAgent was bootstrapped. PID 46880 reported one run and no exits. `overview --json` initially reported scanning, then 24 local agents, 20 with Git and 20 with tasks.

Devbox was updated from current sources and its broker became active with PID 2341390. The connector passed the two-sample VPN gate and reached connected with zero failures. The Mac received 16 live remote rows, producing 40 total. The same overview command invoked on devbox returned those 40 rows through the existing bridge. Canonical, Mac and devbox skill hashes match `6e1b7beed00743766bf5fe5652e402552c414329340b581571cb92bd9bd6cbdf`. No production executable was opened through CUA.

Screenshots 36 and 37 verify the final compact dark selection and empty state. At 05:17:22 UTC a second isolated soak began against the exact snapshot candidate, PID 65164, label `local.hey-boss-reliability-snapshot`, state `out/smoke/b423c7fe-a5a2-4041-bc54-21cfdd0c3a62`, ending at 11:18:45 UTC. Its initial 11 requests passed; results remain pending. The earlier frozen core soak remains independent and active.

## Delivered-question cancellation consistency

Inspection of live session review text prompted checking `hide` on a replayed question. Current code recovered from the old crash but rejected the operation; the broker could cache that error while the Mac question remained pending. Native `hide` now uses the same transactional dismissal path as Close all: prompt/approval become cancelled, waiters receive no answer, and visible questions are removed only after durable storage succeeds. Already-terminal records retain their result/status.

The native audit passed with new actual socket-response checks for hiding both prompt and approval, completion of pending waiters and cleared visible questions. Broker integration now explicitly replays a question, maps `hide` to its Mac ID, caches cancellation and verifies it after restart. All four broker integration tests passed. Optimized cancellation daemon/preview candidates compiled successfully; this change is not yet installed or covered by either frozen soak. The preview live fixture now runs fresh local discovery each pulse rather than aging an unrefreshed snapshot; production already refreshes independently.

At 05:19 UTC the original soak passed 10,450 requests with zero failures; the snapshot-build soak passed 275 requests with zero failures.

## Remote-path main-thread stall

Cancellation candidates were installed at 05:20 UTC with backups/history snapshot under `out/cancel-install-20260915T052046Z`; native PID 98502 started. The connector encountered one banner timeout, observed its persisted bounded cooldown, then recovered automatically to 40 rows. No forced retry was issued.

A direct overview snapshot subsequently timed out. A two-second process sample in `out/production-responsive-sample.txt` showed the main thread in row sorting, repeatedly constructing file URLs and calling `lstat` on display paths. Remote paths must not be probed against Mac mounts. Project/repository/worktree display names now use pure path-string parsing, with audit coverage for root, trailing slash, empty and unavailable-mount paths. The full native audit passed from `/tmp`, optimized native replacement compiled and was installed while retaining the existing connector process. Backup binary is `out/hey-boss-daemon.before-lexical`. A repeated socket-latency check is running across refreshes; results remain pending. Existing frozen soaks do not cover this refinement.

The installed lexical-path build passed 20 consecutive overview snapshot requests across refreshes: maximum 0.006 seconds, mean 0.003 seconds, 41–42 actual rows as running sessions changed. Launchd PID 35491 reported one run and no exits; the retained connector returned connected with zero failures. The separate cancellation-build smoke completed 363 requests in 161 seconds with zero failures, peak RSS 97,232 KiB, then unloaded cleanly. It predates the lexical-path fix.

## Live repository/worktree screenshot verification

The lexical-path preview uses fresh actual local scans every 15 seconds. Screenshot 38 shows six live sessions grouped under their shared normalized repository origin despite three different working directories. Screenshot 39 switches to Worktree, producing groups of 1, 1 and 4. Native accessibility confirms PID 22403 remains selected, and its inspector retains the actual directory/repository/branch/worktree details. These screenshots cover current real-session behavior in addition to synthetic grouping audit counts.

Socket hardening now uses monotonic uptime for the total ten-second read budget, avoiding wall-clock jumps in timeout arithmetic. Response writes gain a ten-second cancellation deadline starting only when a response is sent, so pending questions retain their unbounded user-answer wait. A new native socket-pair audit exercises a stalled request and a one-MiB response whose peer never reads until the deadline. Compilation/audit and installation are pending for this refinement.

The initial stalled-socket audit exited on a test assertion before installation; production stayed on the healthy lexical-path build. The revised fixture explicitly constrains SO_SNDBUF to 4096 bytes and prints timing/closure diagnostics. This deadline candidate remains held pending a passing full native audit; compilation alone is not verification.

Diagnostics showed the stalled read returned at 10.001 seconds, but draining the response exactly at its asynchronous cancellation deadline let the whole response finish before cancellation. The audit now keeps the peer unread for one additional second before draining, removing that race; production still uses the previously audited build.

## Verified response deadline and distribution artifacts

The revised native audit passed: stalled input returned at 10.001 seconds, and a nonreading response peer observed closure with only 4096 bytes buffered after allowing the asynchronous cancellation deadline to execute. The full native audit passed. The audited deadline candidate was installed at 05:31 UTC while retaining the connector; launchd PID 89351 reported one run and no exits. Twenty repeated actual snapshot requests averaged 0.004 seconds, maximum 0.022 seconds, with 41 live combined rows.

A third independent frozen soak began at 05:31:28 UTC against the exact installed deadline candidate, session 1395, PID 89447, state `out/smoke/88904aab-6db7-44d1-aded-8ff6e706caa8`, label `local.hey-boss-reliability-deadline`, deadline 11:18:45 UTC. Original and snapshot soaks remain independent.

Cargo package verification exposed a missing README-linked discovery document. Cargo and release source archives now include discovery/reliability docs and the Python harness. Package listing initially exposed generated Python bytecode when including the whole tools tree; the include pattern now selects Python source only, and release lists the exact harness file. The final Cargo package contains 25 files, includes the linked docs, and excludes bytecode. Release source type-check passed; no release was published. Formatting was corrected after the earlier integration fixture edits.

## Clear local discovery state

Local stale rows now say Discovery stale, while remote stale rows retain Offline / stale. The selectable inspector includes effective state alongside detection evidence. Native audits passed for both labels and all existing failure/socket/grouping checks. Optimized native and preview state candidates compiled; the native candidate was installed with backup `out/hey-boss-daemon.before-state` while retaining the connector. Frozen soaks precede this wording-only refinement.

`docs/verification.md` records the original eight-hour requirements, evidence collected and remaining completion checks. Outdated stopped/pending-deployment text in the discovery guide was corrected. The final elapsed-period and installed-build checks remain pending.

## Bounded scanner output candidate

The native scanner previously read its child stdout to EOF without a byte limit. It now reads in 64-KiB chunks with an 8-MiB total cap. Oversized output closes the read pipe, terminates the child, uses a two-second SIGKILL fallback if necessary, and retains last known local data with an explanatory stale-data error. A native audit accepts exactly 8 MiB and rejects 8 MiB plus one byte. Compilation and full native audit are pending; production stays on the audited state-label build.

The scanner-cap native and preview candidates compiled. The complete native audit passed, including exact 8-MiB acceptance and one-byte-over rejection plus socket/storage/grouping checks. The verified native candidate was installed while retaining the connector, with backup `out/hey-boss-daemon.before-scanner-cap`. Existing frozen soaks precede this scanner-output refinement; final live discovery and an exact-build smoke remain pending.

## Scanner-cap live and rendered verification

The installed scanner-cap build returned 40 combined local/server rows, scanning false, no scanner error and a live devbox connection. A matching preview was updated only after its previous process quit; read-only process inspection confirmed termination when CUA reported a quit-related procNotFound error. Screenshot 40 shows the updated stale-remote inspector with state, directory/repository/worktree/session evidence and disabled Open project.

An exact scanner-cap runtime smoke began at 05:37:39 UTC, session 45014, PID 43925, label `local.hey-boss-reliability-scanner`, state `out/smoke/5bedfee4-718e-4ea7-b6ec-bc733b1b0a04`, ending 05:42 UTC. Initial 11 requests passed; completion is pending.

## Separate durable answer and cancellation coverage

Broker regression tests now cover two distinct delivered-question outcomes. `wait` maps the remote ID to a Mac status request, returns the Unicode answer Résumé α, and preserves its exact terminal result through disconnection/restart. `hide` maps to the Mac hide request, returns cancelled without a result, and likewise remains cancelled after restart. The bridge peer acknowledges incidental agent telemetry so these cases do not depend on telemetry timing. All five broker integration tests passed. Clippy verification is pending for the test refactor; shipped binaries are unchanged.

The exact scanner-cap smoke completed at 05:42 UTC: 572 requests in 260 seconds, zero failures, peak RSS 92,704 KiB. Its isolated LaunchAgent unloaded successfully. Clippy passed with warnings denied after the separate durable-answer/cancellation test refactor; formatting and diff checks passed. All 40 numbered PNG screenshots were confirmed on disk. Production retained PID 40858 during the smoke; the three earlier frozen long soaks remain independent and active until 11:18:45 UTC.

## Native Find shortcut candidate

CUA verification from a selected row found Command-F did not focus the overview search field. The native overview window now handles the standard Command-F combination, focuses search and selects its text. The callback captures the overview weakly; other key equivalents use AppKit behavior. Matching preview/native audit builds are compiling; rendered keyboard verification and installation remain pending.

The full native audit passed after the Find shortcut change. CUA then verified Command-F from an actual selected agent row, typing atlas to filter, and Command-F followed by release replacing the existing query. Screenshot 41 captures the keyboard-driven filter and native focus ring. The optimized native candidate compiled and was installed while retaining the connector, with backup `out/hey-boss-daemon.before-find`. This keyboard-only refinement follows the frozen soaks; the preceding scanner-cap runtime smoke remains the most recent exact-core smoke.

## Keyboard group-boundary verification and screenshot review block

CUA verified Down from the last hey-boss row skips the nonselectable atlas group header, selects PID 102 and updates the inspector to the idle release task. Down again selects the synthetic session-less Claude PID 103, shows unavailable task/session details and disables Copy session ID. The compact/dark menu state was applied.

Automatic approval review rejected emitting screenshot 42 because visible task/repository details might be nonpublic. Source and accessibility checks confirmed fake PIDs 101–104 and example paths, but the fixture-only retry was also rejected as possible disclosure/bypass. Screenshot emission is paused; no further retry will occur without explicit approval for visible task/repository details in this UI review. This does not block native keyboard, code or reliability verification. Screenshot count remains 41. An essential approval request was sent through hey-boss.

## Persisted cooldown recovery candidate

The automatic connector previously accepted an arbitrarily distant saved retry timestamp, which could suppress reconnection indefinitely after corrupted state. Each loop now bounds saved retry time to the existing generated maximum of 915 seconds ahead, including backward-clock adjustment recovery. Retry timestamp addition saturates safely. Generic configuration/status serialization failures now return errors rather than unwrap.

The absent-VPN/saved-cooldown integration adds a corrupt-cooldown case with retry_at=u64::MAX, verifies no SSH attempt across the second DNS sample and checks the recovered deadline remains in the future but within 915 seconds. Builds and verification are pending; the running connector remains unchanged and healthy. Screenshot approval request remains pending; no further screenshot emission was attempted.

The cooldown-recovery integration passed: absent VPN, ordinary saved cooldown and corrupt u64::MAX cooldown produced no SSH request across the second DNS sample; corrupt state recovered to a bounded future deadline. The full Rust suite passed (18 library, 18 binary, six integration tests), Clippy passed with warnings denied, and format/diff checks passed. Optimized CLI compiled and was installed atomically with backup `out/hey-boss-cli.before-cooldown`. Only the connector was gracefully unloaded/rebootstrapped; the native daemon and history were retained. Connection recovery is pending. Mac automatic-connection code is the only runtime change; remote broker behavior is unchanged.

The connector reload initially returned launchctl bootstrap error 5 during teardown. Inspection confirmed the old job was absent; a sequential retry succeeded. New connector PID 61112 reached connected with zero failures and restored 40 rows; native PID 10354 stayed running. Existing config SHA-256 remained unchanged.

Configuration bootstrap now retries only launchd exit code 5 with bounded local delays totalling at most 8.5 seconds. Success ends retries immediately; other errors return their exit status and stderr without retry. A fake-launchctl process regression exercises two teardown EIO failures followed by success and proves an exit-64 error is attempted only once. The focused process regression and Clippy passed; optimized build verification is pending for this follow-on change; the installed CLI contains the already-verified cooldown recovery.

## Real configuration reload and DNS validation parity

The optimized bootstrap-retry CLI compiled and was installed with backup `out/hey-boss-cli.before-bootstrap-retry`. One real `companion configure devbox --vpn-domain quora.net` completed successfully through the save/reload path. Connector PID 81602 reached connected with zero failures; native PID 10354 stayed running. The config hash remained unchanged.

CLI VPN-domain validation now matches the native settings panel: optional trailing root dot, nonempty labels, ASCII alphanumeric/hyphen content, no leading/trailing hyphens, 63-byte label and 253-byte domain limits. Malformed domains fail before writing configuration or changing services. Binary tests cover malformed labels and accepted uppercase/trailing-dot input. Binary tests/build are pending for this parity refinement; Clippy passed with warnings denied.

The DNS-validation parity candidate passed all 19 binary tests and Clippy with warnings denied, and optimized compilation succeeded. The CLI was installed atomically with backup `out/hey-boss-cli.before-dns-parity`. The installed command rejected `companion configure devbox --vpn-domain .` with an ordinary error before any state/service change. Config hash remained unchanged, native PID 10354 and connector PID 81602 remained running, and the connector stayed connected with zero failures. No extra connector restart was needed for this validation-only change.

## 06:01 UTC release and continued-run verification

The latest full Rust suite passed: 18 library tests, 19 binary tests, and six transport/autoconnect integration tests. The absent-VPN and corrupt-future-cooldown integration completed its 17-second observation without SSH requests. Clippy with warnings denied, formatting and diff whitespace checks passed. Cargo packaged and compiled the 26-file source archive, including all three verification/discovery/reliability documents and the Python soak harness, with no bytecode. Release Swift source type-check passed; an initial invocation incorrectly used parse-as-library on this top-level script and was corrected without changing source.

Installed native SHA-256 matches the audited Find candidate: 6565b1bb63e1e196b07b89ff8f27b54e62cf3e0119614d96dce7153d91b19032. Canonical and globally installed Mac skill hashes still match. Production launchd PID 10354 reports one run and never exited; connector status remains connected with zero failures and no retry timestamp.

All three frozen soaks were re-polled through their original live session handles. At 05:59 UTC the original core had 15,730 requests with zero failures, the snapshot build 5,566 with zero failures, and the deadline build 3,707 with zero failures. None was restarted. These remain interim samples, not terminal results or coverage of later builds. Screenshot approval remains pending, and capture stays paused.

## 06:02 UTC latest-source native audit and actual discovery

A freshly compiled optimized native audit from current source passed from /tmp, including recoverable malformed requests, damaged/read-only storage, cancellation and history preservation, settings validation, grouping/search/selection, scanner byte caps, and monotonic socket deadlines. The stalled read returned after 10.002 seconds and the unread reply closed after only 4096 buffered bytes. Daemon source contains no forced try, fatalError, precondition or assert calls.

The installed read-only snapshot contained 25 local agents (21 tasks and 21 Git contexts) and 16 remote agents (8 tasks and 13 Git contexts), with no scanner warnings and no stale rows. Aggregate evidence excludes private task text from this report. The devbox global skill SHA-256 matches canonical and Mac copies. An initial health query used the nonexistent hey-boss-server.service name; its inactive response does not describe the actual companion service, which is checked separately by its source-defined name.

The actual devbox hey-boss-companion.service reports active, MainPID 2341390, and zero systemd restarts. No service or connector was restarted during these checks.

## 06:04 UTC documentation and verified soak wait

README now explicitly describes Repository, Worktree and Ungrouped views, normalized-origin versus host/checkout identity, retained selection, inspector Git details, and local Discovery stale labels. This is documentation-only and passes diff whitespace checks. After a one-minute observation interval, all original soak handles were re-polled without restarts; the production and connector PIDs remained live alongside all three isolated daemons.

## 06:05 UTC fallible broker startup workers

Code review found the two fixed telemetry/replay startup workers still used std::thread::spawn, which panics if the OS cannot create a thread. Both now use named Builder::spawn calls and propagate ordinary startup errors. Client handlers were already fallible and bounded. All five companion integration tests passed after this change, including replay, Unicode answers, offline cancellation, rejected legacy records, and immediate overview controls. Clippy with warnings denied passed. Release build and controlled installed-version updates remain pending; production native source is unchanged and frozen soaks retain their original scope.

The optimized worker-spawn CLI was installed atomically on the Mac with backup out/hey-boss-cli.before-worker-spawn. The source-defined companion installer successfully built and installed the CLI and global skill on devbox. The existing Mac connector stayed connected with zero failures and retry_at zero; no connector or native daemon restart was performed. Devbox service health is rechecked after its controlled installer restart.

After installation, devbox companion service is active with MainPID 2376536 and zero automatic systemd restarts.

## 06:07 UTC exact installed-build long soak

Post-update combined native snapshot shows 24 local and 16 remote agents, no scanner warnings, and zero stale rows. The updated devbox broker continues delivering through the existing connection.

A fourth independent soak now exercises frozen copies of the currently installed native daemon (including scanner cap, lexical paths, stale labels and Command-F) and latest Mac CLI (including fallible server startup workers). It started at 06:07:13 UTC and targets the original 11:18:45 deadline. Label local.hey-boss-reliability-installed, daemon PID 87450, harness session 93708, state out/smoke/7940efe9-dc47-4464-8451-759a8aa8a652. Frozen binaries are out/hey-boss-daemon.installed-soak and out/hey-boss-installed-soak-cli; manifest records their SHA-256 and harness hash. Its first sample passed 11 requests, zero failures. This adds exact installed-build coverage without replacing or altering any previous soak. It cannot honestly be called an eight-hour soak.

## 06:08 UTC screenshot evidence integrity

Added docs/screenshots.md as a review index separating defect-investigation captures from final policy, actual grouping, stale inspector and Find verification. Checked all 41 consecutive original screenshot files without emitting any image. Native capture output is JPEG despite .png filenames; original names remain preserved, and the local manifest records actual JPEG format, dimensions, sizes and SHA-256 hashes. The initial PNG-header check correctly failed on that format mismatch; no screenshots were altered or recaptured. Screenshot 42 remains absent and capture approval pending.

## 06:10 UTC post-update response and refresh observation

Sixteen read-only installed overview requests over approximately one minute all succeeded. Mean response 0.019171 seconds, maximum 0.071427 seconds; 5 distinct remote snapshot receipts, zero stale samples and zero warning samples. Details are in ignored out/post-worker-latency.json and contain only timing/count/timestamp evidence. Connector status remained connected, failures zero, retry_at zero. This verifies continued transport after the controlled server update without extra SSH probes.

All four soak handles and six Mac PIDs (native production, connector and four isolated daemons) remained live during the observation. Exact-installed soak passed 275 requests at its two-minute sample with zero failures; earlier frozen soaks also retain zero failures. Terminal outcomes remain pending until the original deadline.

## 06:23 UTC saved server-record memory bound

Code review found queue replay loaded saved JSON with unbounded fs::read despite bounded network input. Queue entries now reject file metadata beyond 32 MiB and independently cap the read at that limit plus one byte to cover growth after the metadata check. The limit leaves headroom for an 8-MiB original request, an 8-MiB terminal response and serialization overhead. Damaged oversized records remain on disk, return a recoverable status/hide error, and are skipped by replay without blocking later files.

A sparse 32-MiB-plus-one record test passed, proving rejection and preservation without reading the giant payload. All four broker unit tests and Clippy with warnings denied passed. Companion integration and optimized build outcomes are recorded separately after completion. Native source is unchanged; the exact-native soak remains applicable, while its frozen CLI predates this broker-only refinement.

All five companion integration tests passed after the saved-record cap, and the optimized release build succeeded. Mac CLI installed atomically with backup out/hey-boss-cli.before-entry-cap; devbox companion installation built and installed the matching source successfully. Existing Mac connector remained connected with zero failures and retry_at zero. A post-update installed overview query succeeded with fresh remote rows. Native binary and all four frozen soaks were preserved.

## 06:26 UTC native accessibility interaction verification

Without capturing or emitting screenshots, the current synthetic preview retained selected session-less Claude PID 103 and its inspector across Repository → Worktree → Ungrouped. Copy session ID remained disabled and unavailable task/session details stayed explicit. Claude filtering followed by Command-F and typing notes reduced the table to that matching selected row; clearing search, restoring All agents and Repository returned all five fixtures and retained selection. Fresh accessibility state verified every transition. Production app and configuration were untouched.

## 06:27 UTC latest full-suite and package verification

After saved-record hardening, full cargo test --locked passed: 18 library tests, 20 binary tests and six integration tests, including the 17-second absent-VPN/corrupt-cooldown observation. Cargo package compiled its 27-file source archive successfully. The listing includes docs/screenshots.md and all authoritative documents, Rust/Swift sources and Python harness, and excludes private out captures and bytecode. Formatting and diff whitespace checks passed. No native changes or soak restarts accompanied this verification.

## 06:36 UTC actual durable server state and transient discovery follow-up

One bounded read-only unattended SSH query found the updated devbox service active, MainPID 2389146, zero automatic systemd restarts. Sixteen durable JSON records were present: fifteen mapped, two terminal, zero awaiting replay, zero damaged and zero oversized. Server overview --json forwarded successfully and returned 36 rows through the existing bridge.

That point-in-time forwarded snapshot showed twenty local rows temporarily stale while remote rows stayed fresh. The next Mac query recovered naturally to twenty-one local rows, local snapshot age 8.9 seconds, no warnings and no stale rows. No restart or forced refresh was used. A subsequent minute of 31 read-only samples found maximum local age 11.218 seconds, no local or remote stale samples, no warnings, and maximum response 0.058827 seconds. Evidence remains under ignored out/server-post-entry-cap-evidence.json, out/local-discovery-delay-evidence.json and out/local-delay-followup.json. This transient observation is preserved rather than omitted from the final reliability scope.

## 06:39 UTC exact-native thirty-minute checkpoint

The fourth soak, frozen from the installed native build, passed 3,971 requests at its 30-minute sample with zero failures; its later 32-minute sample passed 4,235, peak RSS unchanged at 98,128 KiB and current RSS 69,664 KiB. The original core, snapshot and deadline frozen runs also remain live with zero failures. Production launchd still reports runs one, PID 10354 and never exited. Connector remains connected, failures zero, retry_at zero. These are interim observations and do not replace terminal outcomes or imply that the newer broker-only CLI changes were soaked.

## 06:46 UTC production one-hour and installed-hash checkpoint

Current production native PID 10354 has run for 1:00:45 on a single launch; launchd reports never exited. Its SHA-256 remains identical to the frozen exact-native soak (6565b1bb63e1e196b07b89ff8f27b54e62cf3e0119614d96dce7153d91b19032). Installed Mac CLI matches latest saved-record-cap release (7ae3b2848c64920995816d959cb0e2102ed4992fb5a376e1447ccb3377330693). The fourth soak passed 5,159 requests at 39 minutes, zero failures; all earlier frozen runs remain live and failure-free. These are measured checkpoints, not a guarantee against all possible future failures.

## 06:55 UTC quiet healthy-connection observation

Reviewed connector logging and retry source without server probing. Healthy status stayed connected with failures zero and retry_at zero, while the autoconnect log's hash remained unchanged for 157 seconds. No log content was emitted. This confirms no logged state changes or retry failures during the interval; it is not a packet-level claim of zero traffic, since live snapshots intentionally use the existing tunnel. The separate absent-VPN/cooldown integration remains the evidence that those gates send no SSH requests.

## 07:01 UTC original-core three-hour checkpoint

The original frozen core soak passed 23,782 requests over 10,825 seconds (just over three hours), zero failures and peak RSS unchanged at 107,264 KiB. This build predates the later scanner/lexical-path/socket/UI fixes and cannot stand in for their coverage. The exact installed native build independently passed 7,007 requests over 3,187 seconds, zero failures. All four original handles remain live; production still reports one launch, PID 10354, never exited. Terminal results and final completion audit remain pending.

## 07:08 UTC exact-installed-native one-hour checkpoint

The fourth frozen soak passed 7,931 requests over 3,608 seconds, zero failures; its subsequent 61-minute sample passed 8,063 with zero failures. Peak RSS remained 98,128 KiB; current RSS 53,984 KiB. The native binary matches installed production and includes scanner output cap, lexical remote names, bounded socket replies, stale-state labels and Find shortcut. Its frozen CLI predates only the later broker record cap, so this is exact-native coverage, not a soak of every latest Rust change. All earlier handles remain live and failure-free. Production remains one launch/no exits; current overview shows 36 rows with no warnings or stale data, connector connected with zero failures.

## 07:19 UTC startup archive-memory review

Reviewed Database.pending and Store.restore against current source. Startup selects only pending rows in SQLite row order; completed history bodies are not loaded wholesale. This preserves the archive without making completed-history growth part of restoration memory. No speculative rewrite or history deletion was introduced. All existing soak handles continue unchanged.

## 07:32 UTC explicit private-channel progress filtering

A synthetic regression fixture proved that explicit analysis-marked assistant text could replace public progress in the previous parser. No live private-content exposure was observed. New guards reject analysis, reasoning and thinking phase/channel markings case-insensitively, including event envelopes, modern Codex AgentMessage items, legacy response items and Claude message objects. Public progress remains retained, and existing modern/legacy public-message tests still pass.

All 19 library tests and Clippy with warnings denied passed after the fix. Cache schema is now version 6 and uses agents-cache-v6.json, isolating new summaries from older frozen CLIs that still write agents-cache.json. Old files and history remain untouched; current parser re-reads raw sessions into its new private cache instead of accepting older summaries. Optimized build and controlled installed checks follow. Native source/binary and all four frozen soaks remain unchanged; their CLI scopes predate this parser refinement.

## 07:36 UTC private-channel parser installed and live

Optimized release built successfully. Fresh local scan took 10.229 seconds and returned 20 agents, 16 tasks, 16 Git contexts and no warnings; cached scan took 1.437 seconds with the same agent count and no warnings. Mac CLI installed atomically with backup out/hey-boss-cli.before-private-channel. Devbox installer built/installed the updated CLI and skill successfully; existing Mac connector stayed connected, failures zero, retry_at zero.

Installed native overview returned 39 fresh rows (20 local, 19 remote), no warnings or stale rows. Its v6 cache was modified 3.1 seconds earlier and mode 0600, verifying production uses the new versioned cache rather than accepting legacy summaries. Aggregate evidence is in ignored out/private-channel-installed-evidence.json; raw candidate snapshot is mode 0600. Native binary and all four frozen soaks remained unchanged and failure-free. This Rust parser update is covered by library regressions and real local/server delivery, not by the older frozen CLI manifests.

## 07:40 UTC connector reader setup review

The automatic connector's status-reader still used infallible thread spawning. It now uses a named fallible Builder::spawn and returns an ordinary setup error if thread creation fails. Session ownership is established before fallible reader setup, so its existing process-group cleanup runs on failure rather than orphaning an unattended SSH connection. Missing output pipes and config-display serialization also return errors. No actual resource-exhaustion event was observed or induced.

All 19 library, 20 binary and six integration tests passed after this change; Clippy with warnings denied, release compilation, diff checks and release Swift type-check passed. Mac CLI was installed atomically with backup out/hey-boss-cli.before-connector-reader; SHA-256 is b206de48a059e31d2fa50763fd07040490b95eebec840bfb8685376ffb849b04. Existing connector remains connected with zero failures and retry_at zero; it was deliberately not restarted and therefore has not executed the new reader-setup code yet. This is Mac-only automatic-connection behavior; installed remote broker/parser behavior is unchanged. Native source/binary and all four frozen soaks remain untouched. Full packaged-build verification and terminal soak results remain pending.

The 27-file Cargo package subsequently compiled successfully. At 07:42 UTC the installed read-only overview returned in 14.4 ms; production PID 10354 and connector PID 81602 remained live. Canonical and Mac global skill hashes match, and the connection config hash remains unchanged. Private snapshot evidence is mode 0600 under out/installed-observation-0742.json. The screenshot approval request remains pending; no additional captures were attempted.

## 07:45 UTC evidence provenance review

All eight frozen soak binaries still match their respective manifest hashes; all three later harness hashes also match tools/soak_daemon.py. The original-core legacy manifest records binary and historical source hashes but no harness hash, which is now explicitly documented in the final verification checklist. Manifests were not rewritten. Aggregate review is in out/soak-manifest-review-0745.json.

Screenshot manifest verification exposed a prior dimension-extraction mistake: every image was listed as 72×72. Reading existing JPEG frame headers corrected all 41 entries, with three independently verified using macOS pixel metadata. Actual saved image sizes range from 580×380 to 1254×768; native window sizes and saved image sizes are separate evidence. The original manifest is preserved, image bytes/hashes are unchanged, and no screenshot was captured or emitted during this metadata-only correction. docs/screenshots.md records the correction.

## 07:46 UTC isolated connector reader lifecycle

An isolated copy of the latest CLI used a temporary HOME and fake SSH executable, crossing the real local DNS two-sample gate without making a server request. The new reader recognized the synthetic connection as connected. SIGTERM then produced exit 0, final status stopped, zero failures, and no surviving fake forwarding process. Temporary state was removed; production connector and all frozen soaks were unaffected. Aggregate evidence is out/connector-reader-lifecycle.json. This verifies normal reader startup and shutdown cleanup, not simulated OS thread-creation failure.

## 07:54 UTC remote installed-service verification

The source-defined hey-boss-companion.service is active/running with MainPID 2441178 and NRestarts 0 after the private-channel installer. Devbox's global skill matches canonical/Mac SHA-256. A read-only overview invoked on devbox traversed the existing bridge and returned in 365.7 ms, 38 combined rows (20 local, 18 remote), no scanner warnings. Aggregate evidence is out/server-health-0754.json. An initial query also invoked companion status, which reads Mac automatic-connector config and appropriately returned not-configured on the server; that error is not a broker failure. Strict trusted SSH validation remained enabled despite the existing SFT RSA proof warning.

## 08:01 UTC original-core four-hour checkpoint

The original frozen core reported 31,702 requests over 14,433 seconds, zero failures and unchanged peak RSS 107,264 KiB. This is four-hour evidence for that early core build, not coverage of later scanner/socket/UI or Rust refinements. Independently, the exact installed native run reported 14,927 requests over 6,797 seconds, zero failures, peak RSS unchanged at 98,128 KiB. All four original handles remain live; deadline/terminal success and clean shutdown are still pending.

## 08:08 UTC exact-installed-native two-hour checkpoint

The exact installed native soak reported 15,851 requests over 7,218 seconds, zero failures, current RSS 69,728 KiB and unchanged peak RSS 98,128 KiB. Its frozen native SHA-256 remains 6565b1bb63e1e196b07b89ff8f27b54e62cf3e0119614d96dce7153d91b19032. The frozen CLI predates later record-cap, private-channel and connector-reader changes; those later Rust changes are covered by their separate regressions/live or isolated checks, not this two-hour claim. Production and connector were also confirmed live with their original PIDs at 08:01. All four original handles remain active and failure-free, with terminal verification still pending.

## 08:11 UTC preview launch-job verification

Launchd inventory contains the four isolated reliability services, production, companion and one application preview job. The existing preview PID 98962 has one launch, never exited, elapsed 2:26:36 and RSS 42,064 KiB. Its app-bundle executable matches out/hey-boss-preview.find-candidate SHA-256 5c331ed7aceacf9d7d2f5cb5a385510a58e104e9950a580fab52f704c15dab28; no additional obsolete preview job was present. Production PID 10354 and companion PID 81602 remain unchanged. This is current process/job evidence, not proof of universal future crash immunity; no app was opened or restarted for this check.

## 08:18 UTC snapshot-build three-hour checkpoint

The frozen snapshot build reported 23,782 requests over 10,830 seconds, zero failures, current RSS 41,888 KiB and unchanged peak RSS 100,336 KiB. Its original handle 38006 and daemon PID 65164 remain live. This run predates later deadline/lexical-path/scanner/UI/native and Rust refinements and does not cover them. The independent exact-native run reported 17,171 requests over 7,819 seconds with zero failures. All four original sessions remain active; terminal success is still pending.

## 08:21 UTC healthy-connection quiet observation

Across 135.486 seconds, connector PID 81602 remained live, state connected, failures 0 and retry_at 0. Its log size and nanosecond mtime were unchanged, proving no logged retry/state activity during this interval, not zero packet traffic. Start/result evidence is out/quiet-observation-0818-start.json and out/quiet-observation-0818-result.json. No forced reconnect or extra SSH probe was performed. All four frozen stress runs continued with zero failures and unchanged memory peaks.

## 08:32 UTC deadline-candidate three-hour checkpoint

The frozen deadline candidate reported 23,771 requests over 10,825 seconds, zero failures, current RSS 42,000 KiB and unchanged peak RSS 99,760 KiB. Its original handle 1395 and PID 89447 remain live. This build includes the bounded socket deadlines but predates later stale-state wording, scanner cap and Find changes; it does not cover every current native or Rust refinement. The independent exact-native run remains failure-free, and all four original sessions continue toward 11:18:45 UTC. Terminal results remain unproven.

## 08:32 UTC installed discovery observation

A read-only installed overview returned in 10 ms with 35 combined agents (19 local, 16 remote), 23 task summaries, 28 Git contexts, 33 Codex and two Claude rows. Scanner warnings were zero; local/server snapshot ages measured immediately at receipt were 6.62/4.71 seconds. Aggregate-only evidence is out/installed-overview-0832.json. No raw task text was printed, forced scan requested, window opened, or connection probed. All four frozen runs remained active and failure-free.

## 08:46 UTC production and preview three-hour runtime

Launchd directly reports one launch and never exited for both production PID 10354 and audited preview PID 98962. Their elapsed times were 3:00:35/3:01:58 and RSS 56,528/40,688 KiB. Companion PID 81602 was also confirmed live at 08:44, connected with failures 0 and retry_at 0. All four isolated stress runs remain live and failure-free; these production/preview observations are independent from synthetic test-request coverage. Screenshot approval is still pending and no capture was attempted.

## 09:01 UTC original-core five-hour checkpoint

The original frozen core reported 39,622 requests over 18,042 seconds, zero failures and unchanged peak RSS 107,264 KiB. The independent exact-native run reported 22,847 requests over 10,405 seconds, zero failures and unchanged peak RSS 98,128 KiB. All four original sessions remain live. This five-hour claim covers only the early frozen core; later native/Rust refinements retain their separate verification scopes. The requested deadline and clean terminal results remain pending.

## 09:07 UTC exact-installed-native three-hour checkpoint

The exact installed native run reported 23,771 requests over 10,826 seconds, zero failures, current RSS 69,232 KiB and unchanged peak RSS 98,128 KiB. Its original handle 93708 and daemon PID 87450 remain live. The native binary includes current scanner/socket/UI changes, while its frozen CLI predates the later record-cap, private-channel and connector-reader changes. Those later Rust refinements retain their separate test/live or isolated evidence. All four original sessions continue without failures; final deadline and clean terminal verification remain pending.

## 09:18 UTC snapshot-build four-hour checkpoint

The frozen snapshot build reported 31,702 requests over 14,438 seconds, zero failures, current RSS 42,224 KiB and unchanged peak RSS 100,336 KiB. Its original session 38006 and PID 65164 remain live. This four-hour claim covers that earlier snapshot binary, not later native or Rust changes. Independently, the exact-native run reported 25,091 requests over 11,427 seconds with zero failures. All four original sessions continue without restarts; final deadline and clean terminal verification are still pending.

## 09:32 UTC deadline-candidate four-hour checkpoint

The frozen deadline candidate reported 31,691 requests over 14,433 seconds, zero failures, current RSS 41,744 KiB and unchanged peak RSS 99,760 KiB. Original session 1395 and PID 89447 remain live. This four-hour evidence covers its recorded socket-deadline build, not subsequent native wording/scanner/Find or Rust changes. All four sessions remain active and failure-free; final deadline and clean terminal outcomes remain pending.

## Completion audit, September 15, 14:14 UTC

The requested eight-hour deadline has passed. Three original harness handles exited 0 and recorded passed plus service_stopped returncode 0: original core 57,794 requests / 26,320 seconds / 107,264 KiB peak; snapshot 47,608 / 21,683 / 100,336 KiB; exact installed native 41,030 / 18,691 / 98,128 KiB. All recorded zero failures. These frozen runs do not cover later Rust changes.

The deadline candidate handle exited 1, with no terminal event. Its last sample at 09:36:02 records 32,219 requests, zero failures, and 99,760 KiB peak. This is inconclusive, not a pass. Connector logs separately show ENOSPC exits/restarts; disk pressure is a possible harness interruption cause but is unproven. Current disk has 135 GiB available. No user data was removed in this audit.

Connector status writes previously propagated errors out of run(), dropping SSH and causing launchd reconnects. Status writes now retain the connection and in-memory cooldown, log the first failure only, retry every 60 seconds, and report recovery. Failure to save stopped status no longer changes a clean signal shutdown into exit 1. A deterministic integration test blocks the atomic temporary path, verifies the process stays alive, emits one warning, and exits 0 on SIGTERM; it does not simulate OS-wide disk exhaustion or a connected SSH session.

Final checks: 46 Rust tests passed, Clippy warnings denied, release build, format/diff checks, package listing and Swift release typecheck passed. Native optimized audit compiled and passed from /tmp, including damaged/read-only storage and monotonic socket deadlines. Latest CLI was installed atomically with backup out/hey-boss-cli.before-status-storage, SHA-256 403d9d879d128d3600230af31e087a3dee96b6a7cd6d5479cd4afd0141b08e7d. The existing live connector remains on its older executable: a bounded unattended devbox SSH check requires renewed SFT authentication, so deliberately restarting it would discard the currently working connection. The storage fix takes effect on its next launch; this is an outstanding deployment verification.

Production native PID 10354 remained running for 8h25m, launchd last exit 0; controlled preview PID 98962 remained running for 8h26m, last exit 0. Native installed hash matches the frozen exact-installed binary. Canonical and Mac global skill hashes match. Latest socket overview took 17 ms and included 19 local agents, 14 with task text and 14 with Git context. The existing forwarded connection remains live; fresh remote systemd/skill checks could not be completed because unattended SFT authentication expired. Previously verified remote evidence remains historical.

Final CUA screenshot was rejected by automatic approval review for potential private task/repository disclosure despite the prior stored approval result. No bypass or new capture was performed. The 41 retained screenshot artifacts and corrected dimension manifest remain the visual evidence. The final screenshot, remote refresh, and current connector deployment checks remain pending; this audit does not declare the overall goal complete or claim crashes are impossible.

## Communication policy and notification sources

Canonical skill and Mac global copy now reserve hey-boss for substantial outcomes or essential blocking decisions from long-running background work that need attention. Synchronous replies remain in chat; QA, builds, publishing, deployments, minor completions and per-agent progress do not notify. A coordinating agent consolidates qualifying outcomes into one short message, with at most two brief sentences of optional details. The skill validator passed using isolated uv PyYAML. SHA-256: 15c6f2fb446be84a3dc7e0832f732c82f1aa5284d12318fd9f4e780f5eadf3cf. Updated policy is embedded in the installed CLI for subsequent automatic sync. Registered devbox sync failed because SFT authentication was unavailable; its installed copy is not yet confirmed updated. No notification was sent for this synchronous skill/UI work.

Notifications now carry a durable source hostname. The Mac CLI stamps This Mac; the server broker replaces this at forwarding/replay time with its hostname. Native cards display a muted laptop/server icon and source footer, collapsed groups list their sources, and launch details include source. Historical records and older clients without metadata say Source unavailable instead of being guessed local. The server broker must be upgraded to label server messages; the remote upgrade remains blocked by authentication.

Verification: 53 Rust tests passed (20 library, 22 binary, 2 autoconnect, 9 companion), including replay source metadata. Clippy warnings denied, format/diff checks and release build passed. Optimized native audit passed from /tmp, with synthetic local/server/legacy source labels and persistence round-trip added. Native production binary installed atomically with backup out/hey-boss-daemon.before-source-label, SHA-256 46bc7e00232c7b44d74bf1de9f093942c2f4f30d8977f73618c735179110826f. Controlled launchd restart produced running PID 82359; protocol response verified directly before restoring missing installation protocol metadata, then installed overview command succeeded. Latest CLI SHA-256 7cf8c6eab62257ea5c24089367e30d9a695963f55ba8ea557a33f110eeea9bf1. No final screenshot was captured because the private-overview capture remains rejected; visual source footer is supported by native audit, not a new screenshot review.

## Severity visibility

Notification Surface now draws a severity wash above glass and below content: 12% color in light appearance and 20% in dark, a colored border, and an 85% opacity 3-point leading accent. Info is blue, success green, warning orange, error red; neutral remains untinted. Icons and severity text remain so color is not the only signal. The wash ignores hit testing and refreshes on appearance changes. Expanded project stacks retain per-card colors; collapsed stacks use the highest severity. Native optimized audit and release builds compiled successfully; final native runtime audit and installation recorded below.

Optimized native audit completed from /tmp with exit 0. Installed severity-tint native hash matches e13ffba48b87854aaf80490a37bd0f16ba6aa77a2f6cb31942c8b7a0f49200e8; backup out/hey-boss-daemon.before-severity-tint retained. Controlled launchd restart yielded running PID 47944 and the installed overview socket command succeeded. No notification was sent for this synchronous UI request. No screenshot was captured in this change.

## Controlled Markdown reader and server file input

Added update --file/--markdown-file, preserving inline Markdown. Regular UTF-8 files up to 1 MiB are snapshotted before posting. Markdown rendering now uses the proven CommonMark/GFM parser pulldown-cmark with an app-owned HTML/CSS renderer in native WebKit; raw HTML stays literal and unsafe destinations are removed. The renderer is bounded and off the main thread, with basic native Markdown fallback. The Read update button now uses a regular control's intrinsic height. Actual view hit testing through both the card and complete notification container passed, and clicking its action opened the reader and persisted dismissal; the reported physical click failure has not been reproduced. See docs/markdown-reader.md for precise limits and the screenshot tooling blocker.

Adapted 21 poe-code renderer/parser example groups with preserved MIT notice. Three initially failing groups (frontmatter, alert callouts, unsafe URLs) now pass. Full Rust suite passed 78 tests, Clippy warnings denied, release build and package compilation passed. Native optimized audit passed including actual WebKit DOM and compact/wide layout checks. Mac and devbox CLI/native installs are current; canonical/Mac/devbox global skills match. Real devbox isolated queue testing proved offline --file input, original-file deletion, broker restart and exact replay/rendering, without production QA notifications. Fresh SSH authentication is working and earlier remote distribution blockers have been resolved for these changes. No new reader screenshots could be obtained because CUA returns failedToCreateImageDestination even for a synthetic fixture app.
