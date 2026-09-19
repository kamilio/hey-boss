# Mac performance round — 2026-09-17

Installed native daemon: `/opt/homebrew/opt/hey-boss/libexec/hey-boss-daemon`.
Previous executable retained as `out/hey-boss-daemon.before-performance`.

## Measurements

Optimized Swift builds; 1,000 synthetic agents across 50 repositories; 30 iterations
alternating an empty search and `feature-1`. The loaded runs each add eight
low-priority CPU workers, removed when the harness exits. The Mac also had
substantial independent activity (load averages approximately 52–65 during these
runs), so scheduling conditions were not identical.

| Measurement | Before | After |
|---|---:|---:|
| Warm overview rebuild, p95 | 16.99 ms | 1.09 ms |
| Search, p95 | 16.28 ms | 6.61 ms |
| Local store queue delay during slow periodic mobile sync | 1912.52 ms | 0.021 ms |

The local HTTP fixture delays each response by one second. Store queue delay is
one probe submitted while that sync is active. These measure model rebuilding
and local queue responsiveness, not screen FPS. The first rebuild populates the
cache; cold work remains necessary when a new snapshot arrives.

## Changes

- Periodic mobile HTTP runs on a serial utility queue, outside the local SQLite,
  notification, and comment queue. Database snapshots and application of server
  outcomes remain serialized. Explicit foreground answers retain server
  acknowledgement semantics.
- The outbox carries revision numbers, including a migration for installed
  databases. Completion of an older upload cannot delete a newer enqueue.
- Each sync batch publishes presence once instead of repeating it for every
  outgoing task. The five-second timer allows 500 ms scheduling tolerance.
- Overview sorting and lowercased search data are cached until snapshots change
  or their stale classification changes. Search and expansion reuse that work.
- Repository comparison metadata is computed once per group. Interactive
  rebuilds use cached connection state; connection and machine configuration
  files are read on a utility queue during explicit refresh.
- AppKit schedules layout on its normal frame instead of forcing layout inside
  overview rebuilding. Notification bursts coalesce arrivals into one layout.
- Remote scans use a utility operation queue with at most two concurrent
  processes. Agent refresh remains on opening/manual refresh; no polling added.

## Verification and reproduction

The full optimized native audit passed, including 20 arrivals producing one
layout, legacy outbox migration, stale completion preserving a newer enqueue,
agent grouping/search, steering UI, Markdown/WebKit, comment persistence and
selection, and overlapping dismissal animations. Light and dark screenshots
are preserved in `output/performance/`.

`tools/measure_mac_performance.py BINARY --load-workers 8 --output FILE`
runs the isolated benchmark with a slow local HTTP service. Build the audit
binary from `hey_boss_daemon.swift` and `test_hey_boss.swift` with `-O`,
`-whole-module-optimization`, `-parse-as-library`, and `-D HEY_BOSS_AUDIT`.

`tools/measure_daemon_latency.py --pid PID --cli CLI --output FILE`
samples ten seconds of idle CPU/RSS and 15 overview RPCs. RPC wall time includes
CLI startup and OS scheduling; it is not a pure UI-thread timing.

Raw measurements: `out/mac-performance-{before,after}{,-loaded}.json` and
`out/mac-runtime-{before,after}.json`. Audit: `out/mac-performance-audit.log`.

## Installed app check

The restarted daemon responded to every overview RPC. Wall-time p95 was
30.3 ms before and 17.1 ms after;
median remained about 12.1 ms. System load differed,
so this is a health check rather than an isolated optimization comparison.
The ten-second idle sample measured 0.0014% CPU and
59.6 MiB resident memory after restart (the older, long-running
process had 47.6 MiB RSS). The cache trades retained metadata
for less repeated UI work; startup and memory compression also affect RSS.

## Click-path follow-up

The original model-only benchmark omitted the AppKit work after a click. The
follow-up exercises 30 repository expansion/collapse handlers, followed by
content layout and window display work, with 1,000 agents and eight competing
CPU workers. Windows are offscreen; these are synthetic AppKit timings, not
physical pointer-to-photon measurements or FPS. Cold first-layout outliers are
retained in the raw measurements.

| Stage, p95 | Before | After |
|---|---:|---:|
| Click handler | 3.41 ms | 7.60 ms |
| Following layout/display work | 36.21 ms | 3.74 ms |

Incremental updates do some AppKit work inside the handler, so handler time
increased while subsequent layout work fell substantially. The medians are
2.88 ms handler plus 31.49 ms layout before,
and 6.51 ms plus 2.30 ms after.
These per-stage medians are not an exact combined-latency percentile.

Repository expansion inserts/removes its affected rows, preserves unaffected
cells, and refreshes the changed header. Agent expansion updates only that
row's content and height. Visible action tags are rebound after row shifts.
Expansion preferences write asynchronously in order on a utility queue.
Disclosure controls accept the first click in an inactive window. Normal
mouse-up/cancel behavior is preserved, with no artificial activation delay.

A second bottleneck affected notification dismissal: resolving an already
published item used presence, republish and resolve requests. It now makes
one acknowledged resolve request. A 404 publishes an unknown item safely and
then resolves it; errors and phone/Mac conflicts retain existing semantics.
The native mobile audit verifies the one-request conflict path, unknown-item
fallback, exact winning answer, pending offline requests and cross-device read
receipts. The full native UI audit also passed.

The overview JSON now exposes `p95_mouse_event_age_ms` (event timestamp to
window dispatch), `p95_click_handler_ms`, `p95_table_update_ms`, and full versus
incremental update counters. Samples are bounded to 60 and collected only
when events/actions occur; no sampling timer or background scan was added.
A zero event-age value in a synthetic run means no hardware mouse samples.

Raw data: `out/mac-click-before.json`, `out/mac-click-after.json`.
Audits: `out/mac-click-final-audit.log`, `out/mac-click-mobile-audit.log`.
