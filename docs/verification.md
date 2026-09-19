# Eight-hour verification checklist

The work period began at 03:18:45 UTC on September 15 and ends at 11:18:45 UTC. Completion is unproven until that deadline and the final checks below pass. Historical notes in reliability.md describe individual builds; frozen soaks do not automatically cover later edits.

| Requirement | Evidence already collected | Remaining verification |
| --- | --- | --- |
| Preview launch crash fixed | Preview startup no longer invokes forced audit/export; full native audit runs from /tmp; many CUA launches | Final installed-source audit and launch |
| Recoverable native failures | Storage/schema/damaged-record audits, bounded requests/images/timers, transactional dismissal and cancellation, monotonic read deadline and unread-response cleanup audit | Latest candidate audit and final runtime health |
| Actual Codex/Claude activity | Same-user process/open-session matching, modern/legacy parser tests, live snapshots and screenshots | Final actual local/server counts and scanner warnings |
| Directory/repository/worktree grouping | Git tests and normalized origins; screenshot 38 combines six actual sessions; screenshot 39 splits them 1/1/4 with selection retained | Final installed build plus grouping/search/selection invariants |
| Native visual quality and screenshots | 41 numbered screenshots across light/dark, sizes, selection, many/empty/settings; documented resize/ellipsis corrections | Render any final visual edits and inspect evidence |
| Installed Mac and devbox | Controlled binary replacements with backups/history snapshots; launchd and systemd running; actual combined overview queried from both ends | Final binary/skill hashes, single-instance and no-exit checks |
| Durable queue and two-way answers | Replay/restart/cancellation mapping integration tests; real remote queue/bridge checks and stored terminal outcomes | Final integration suite and durable replay state |
| Canonical global skill distribution | Mac/devbox/canonical matching SHA-256; embedded canonical skill and registered-host syncing | Final hash comparison after all edits |
| Quiet VPN-aware automatic connection | Absent-VPN/cooldown fake-SSH integration; exact DNS gate, two positive samples, persisted bounded retry, unattended SFT; observed timeout recovered naturally | Final connector state and retry/request logs |
| Transport extensibility | Existing notification/status/wait/hide plus immediate overview controls; read-only combined snapshot API | Final socket latency and forwarded snapshot check |
| Reliability over the requested period | Independent original, snapshot, deadline and exact-installed frozen soaks, all currently active | Require each original handle to exit 0, a passed event and service_stopped returncode 0; check frozen manifests and report each build/duration/request/failure scope |
| Packaging/documentation | Cargo package verified; source/README-linked docs and Python harness included, no bytecode | Final package listing, format/diff checks and release source type-check |

All screenshots and private runtime samples remain under ignored out/. Private task text and raw production snapshots should not be published in shared reports. Final claims must distinguish installed-build audits, real live checks, and each frozen runtime soak.

The original-core manifest uses a legacy `sha256` map and includes historical source hashes; only its frozen binary hashes should match the current files. Its harness hash was not recorded. The three later manifests use `binaries` and `harness_sha256`; verify both without rewriting the manifests. Missing historical provenance must be reported rather than inferred from the current harness.

## Final audit status

See reliability.md completion audit for terminal results and limits. Three soaks passed; the fourth exited 1 without a terminal record and cannot satisfy the original all-four gate. Latest code checks pass (46 Rust tests and optimized native audit). The latest CLI is installed, but the existing connected process deliberately remains on its older image while fresh unattended SFT authentication is unavailable. Final remote checks and the automatically rejected screenshot remain pending. The overall goal is not marked complete. No unconditional no-crash guarantee is supported by this evidence.
