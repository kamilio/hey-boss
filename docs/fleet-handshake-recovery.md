# Fleet handshake recovery

Opening the companion database, registering its identity, preparing capture,
bootstrapping a standalone store, and collecting workers all precede `hello`.
The SSH command also ensures the companion service first. A fixed wait for only
`hello` classified any delayed startup as a missing or incompatible executable.
Database writer contention reproduces delayed startup without any protocol change.
The historical incident also included relay ownership conflicts and SSH failures;
the old generic timeout cannot establish which stage caused each occurrence.

The companion reports a versioned `starting` frame every five seconds during
service setup and snapshot preparation, beginning before opening the database.
These frames carry a phase and elapsed time, never worker definitions, cursors,
or configuration acknowledgments. The reporter stops before `hello`. Normal fast
startup still begins with `hello`, and the supervisor accepts legacy companions.
Upgrade the supervisor first: older supervisors do not recognize startup progress.

The supervisor retains a 15-second silence deadline and a 120-second total
startup deadline. Progress cannot keep an unfinished startup alive indefinitely.
Missing executables, EOF/SSH errors, malformed frames, incompatible versions, and
timeouts have distinct diagnostics. Timeout and EOF diagnostics include the last
reported phase; bounded fleet events retain phase timings for live investigation:

```sh
hey-boss fleet status --records events --limit 100
```

The companion's input reader refreshes local connection status upon valid
supervisor frames, including coalesced pings while a pull waits for the writer.
Writes are throttled to once per second. Only a completed pull advances
`last_sync`; EOF clears connection freshness without erasing the last sync time.
No worker restart, pause, capacity adjustment, or assignment change is needed.

Authority commands allow up to 15 seconds for a missing/refused local relay or
an unfinished supervisor handshake to recover. The same deadline covers the
response, including partial frames; cancellation stops waiting. An explicit
configure with missing capabilities still reports unsupported promptly. A
connection gap alone never recommends an upgrade.

Retries occur only before connection or after the relay explicitly confirms
that the handshake is pending and nothing was forwarded. Once forwarding is
possible, a lost response has an unknown outcome: reuse the same request ID and
content to inspect/recover its receipt. Revision guards are never refreshed
automatically, and no failure falls back to writing the local replica.

Verification uses private databases and transport processes:

```sh
cargo test --locked -p hey-boss --lib fleet::native::
cargo test --locked -p hey-boss --test fleet_native --test fleet_tunnel
node tools/test_fleet_view.js
```

`tools/fleet_handshake_browser_checks.js` runs with `playwright-cli run-code`
against a native issue web page. It substitutes synthetic API responses and
checks desktop/phone layouts, light/dark appearance, seven preserved jobs,
connected/disconnected/recovered state, keyboard focus, and absence of worker
control requests. Screenshots under `output/playwright/issue682` are disposable.
