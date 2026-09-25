# Native notification mouse dispatch

Issue 251 exposed a timing error in `auditNotificationClicks`, rather than a
production notification hit-testing failure. AppKit can spend more than 300 ms
inside the first mouse-down dispatch. The old fixed-duration event pump then
stopped before dequeuing mouse-up, so the click recognizer never completed.
The full suite also leaves window and activation events queued because it drives
AppKit without `NSApplication.run()`. A diagnostic run spent nearly five seconds
dispatching those events before reaching its first synthetic mouse-down.

Before creating each gesture, the fixture drains pending setup events with a
bounded wait. This keeps old window/activation events out of the gesture's timing.
It then pumps events until mouse-up is dispatched to its window, or an
action confirms that a control consumed mouse-up in its own tracking loop. A
five-second deadline still fails an unfinished sequence explicitly. The separate
behavior assertions remain unchanged: body and title clicks open the notice,
dragging does not open it, CTA and dismiss actions do not double-fire, collapsed
groups expand, and nested card clicks do not collapse their group.

Do not fix this failure by changing production recognizer delays, removing mouse
events, replacing them with direct action calls, or relaxing the behavior checks.

## Reproduce and verify

Compile with the same assertion-enabled flags as the macOS Check job:

```sh
xcrun swiftc -g -Onone -parse-as-library -D HEY_BOSS_AUDIT \
  hey_boss_daemon.swift test_hey_boss.swift -o /tmp/hey-boss-test
HEY_BOSS_AUDIT_NOTIFICATION_CLICKS=1 /tmp/hey-boss-test
HEY_BOSS_CLI_PATH="$PWD/target/debug/hey-boss" /tmp/hey-boss-test
```

Run from a logged-in desktop session. The focused audit must exit normally and
print `Passed: native mouse events open body/title`. The full audit must also
reach its final `Passed: grouping threshold, CTA dismissal` marker. A partial
log, interrupted process, or missing final marker is not a successful audit.
For visual inspection, set `HEY_BOSS_AUDIT_SNAPSHOT_DIR` to a disposable directory
on the focused click audit. It exports AppKit layouts in light and dark appearance
for individual, collapsed, and expanded notifications. These duplicate views use
solid backing because bitmap export cannot capture compositor-owned glass. The
behavior assertions still exercise the original glass-backed views with real
AppKit mouse dispatch. Neither CI nor these exports require Screen Recording.

After the full audit, verify the optimized setup and daemon builds, app packaging,
strict code signature, plist validation, Rust release build, and workspace
package checks in `.github/workflows/check.yml`. Skipped downstream steps do not
count as a complete Check run. Remove temporary executables and snapshots after
review.
