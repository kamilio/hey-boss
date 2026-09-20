# Issue interface verification

Verified on September 18, 2026, on macOS using the optimized Rust binary, headed
Chromium, and Playwright WebKit. Synthetic data lives in
`out/issues-web-review/issues.db`; the user's normal database was not seeded.

## Global profile and measured stability work

Boss naming moved to global SQLite profile settings, opened from the profile
badge dropdown. `hey-boss settings show/set --boss-name` provides the CLI surface.
Schema 7 migrates the most recently active project's custom name. Stable
assignments and issue revisions do not change; no project is registered by a
global settings read or write. Compare-and-swap saves and identical request IDs
protect concurrent edits and uncertain retries.

Initial checks passed 213 Rust tests (one optional test ignored), strict Clippy,
19 profile browser assertions and six clean profile accessibility states per
browser. Broader checks passed the 33-step workflow including all 5,000 rows,
30 queue-order assertions, 23 instruction/PR assertions and 27 Markdown checks
in Chrome. The shared Inbox passed 33 assertions and six pending-response checks;
11 Inbox and 10 issue accessibility states were clean. A real Codex `/goal`
completed a reordered issue in an isolated checkout and committed verified bytes.

Polish includes distinct Instructions/Profile Settings entry points, neutral
disabled-button cursors, and selecting a notice's unambiguous matching project
when attaching an issue. The native animation audit now waits for completion with a bounded
three-second deadline instead of assuming completion inside 650ms. WebKit's native selects require generated style
attributes; CSP permits attributes while blocking inline stylesheets and scripts.

Timed test durations and terminal results are tracked separately under
`out/profile-phase1/goal-progress.json`. The first measured pass completed
**03:29:47–05:29:47 UTC on September 19**, with **66,934 integration requests**
and **2,996 native requests**, zero failures, and 7,200 seconds in each run.
The second measured pass completed **06:09:49–08:09:49 UTC**, with **78,837 integration requests** and **2,994 native requests**, zero failures and 7,200 seconds in each run.

Further phase-one testing reproduced list focus moving out of view when a row
above it grew, and related native notice links failing to refresh on an open
issue. Viewport-relative focus restoration and periodic notice snapshots fix
those cases while preserving drafts. Notice failures now recover in place.

A delayed profile reply could also revert a newly saved Boss name. API profile
metadata now includes its global revision; the web app ignores older revisions
and replies from the previous host. The deterministic stale-reply browser check
failed before the fix and passed in Chrome and WebKit afterward.

Fleet regressions reproduced hidden-project allocation, stale tag-filter
buffers, and independent workers sharing one allocation buffer. Allocation now
respects hidden projects, counts matching work, excludes inactive Boss work, and
supplies distinct issues for independent slots with overlapping filters. All
27 fleet tests passed before beginning a separate repeated regression run.

## Boss assignments and list filters

The web identity is always `human:boss`. Global profile Settings and CLI configure
its display name across projects without changing assignments. Exact assignee filters compose with
labels and search. Labels and assignees in rows use native filter links that
receive pointer and keyboard input above the stretched issue navigation link.
Closed rows show the stored closure time with a reachable exact-time tooltip.
The web can unassign any open issue; another session's claim requires confirmation.

**199 Rust tests passed**, with one pre-existing optional live-control test
ignored. New coverage includes schema 5-to-6 migration preserving settings,
priority order, claims and comments; CLI assignment and display names; rename
persistence; exact filters; web identity; reservations; and worker exclusion until
Boss releases an assignment. Repeating assignment to Boss is a successful no-op
without incrementing the issue revision. Strict Clippy passed.

**34 browser checks passed in each of Chromium and WebKit**, covering actual
pointer hit testing, keyboard filtering, filter combinations, encoded labels,
reloads, Boss assignment, unassignment and cancellation, content preservation,
renaming and HTML escaping, `/goal` previews, closure timestamps, mobile bounds,
and automatic dark appearance. Seven accessibility states per browser had **zero
axe violations**. The existing 23 instruction/PR browser checks also passed, along with 30
drag-order checks per browser. Four additional checks per browser verified
confirmed Boss takeover, cancellation, audit attribution and release.
WebKit exposed mobile filter overflow from longer assignee options; flexible
select controls now remain within the viewport.

Evidence: `out/boss-*-tests.log`, `out/boss-final-clippy.log`, and
`output/playwright/boss-*-checks.log` / `boss-*-axe.log`.
Reusable browser flow: `tools/issues_assignee_browser_checks.js`.

The optimized binary was installed on the Mac with a matching SHA-256 and the
issue server restarted through the desktop daemon. A private SQLite backup
passed integrity checks; all existing issue rows, including claims and queue
order, remained unchanged. Six read-only checks passed against the live Poe2 UI,
including cancelling an unsaved Boss-name draft. Devbox installation passed its
Linux checks and eight isolated assignment/filter/rename/idempotence/lifecycle
checks. Canonical Codex, Agents and Claude skills match the repository skill.

## Shared issue queue order

Cross-project instruction variables: `{{create_issue_command poe-code}}` expands
to a create command with an explicit target, title, and Markdown placeholders.
The bare variable uses the current project. Six worker prompt unit tests and
23 HTTP/worker integration tests passed, including full IDs, quoting, native
goals, single-pass expansion, preview/claim equality, and executing the command
against another project while `HEY_BOSS_ISSUE_PROJECT` pins the worker project.
Six browser checks passed in Chromium and WebKit for live previews, goal prefix,
full IDs, persistence, claim equality, and mobile bounds. Strict Clippy passed.
Evidence: `out/create-command-*.log`; browser script:
`tools/issues_create_command_checks.js`.
Installed Mac verification checked the live Poe2 preview and cancellation without
saving user settings. An isolated devbox smoke confirmed preview/claim equality,
native goal detection, and executing the expanded cross-project command.

Automatic appearance follow-up: the theme switcher was removed. CSS follows the
system light/dark preference from first paint and responds to live changes,
ignoring legacy saved overrides. **10 checks passed in Chromium and WebKit**,
covering appearance changes while editing, retained drafts, Markdown colors,
reloads, mobile bounds, and dark appearance with JavaScript disabled. Six
accessibility states in each browser had zero violations. The nine existing
HTTP integration tests passed. Existing browser/theme audits now emulate the
system preference. Evidence: `out/auto-theme-*.log`; scripts:
`tools/issues_auto_theme_checks.js`, `tools/issues_auto_theme_accessibility.js`.
The optimized update was installed on Mac and devbox. The live Poe2 UI passed
read-only checks for both system appearances and removal of the switcher.

Follow-up: UI-created issues now prepend atomically; CLI-created issues still
append. The web UI displays all matching issues on one page and has no pagination
controls. Legacy URLs with an offset still show the complete list. **45 issue,
HTTP, and worker tests passed**, including requests beyond the former 100-row
limit, filters, unique ranks, and creation retry deduplication. Defaulted API
fields preserve legacy request payloads. Strict Clippy and formatting passed.
**11 browser checks passed in Chromium and WebKit**, using 125 issues and the
actual desktop/mobile create forms. These checks also cover filtering, totals,
bottom-row reordering, and persistence. Evidence: `out/ui-create-top-*.log`;
reproduce with `tools/issues_ui_create_order_checks.js`.
Installed read-only verification passed on Poe2, and an isolated devbox fixture
confirmed UI prepend, CLI append, and a complete 106-row response. Both machines
have the optimized update; the companion remains active. Live SQLite integrity
and unique queue positions passed; user issues were not used as write fixtures.

September 18 ordering follow-up: **191 Rust tests passed**, with the existing
optional agent-control test ignored. Strict Clippy, formatting, and whitespace
checks passed. The final issue CLI suite also passed after correcting PR output
to print beneath its own issue. Schema 4-to-5 migration retains issue content,
claims, Markdown comments, and PRs, initializing queue positions by issue number.
Earlier schema migrations also pass. A deterministic 90-move regression compares
every result with a reference order and checks unique positions.

**30 browser checks passed in both Chromium and WebKit**. They cover before/after
dragging, keyboard moves and focus, reload persistence, insertion indicators,
Escape and pointer cancellation, mobile bounds, filtered ordering, retained
Markdown/PR links, conflicting concurrent reorders, and failed-save retries with
the same request ID. A deliberately delayed refresh arrives during a drag and
proves that both rendered rows and their order revision remain unchanged.
WebKit exposed a mouse-focus/Escape bug, which was fixed and retested. Actual
Chromium touch input also reordered an issue successfully. Axe reported zero
violations across seven states in each browser: light/dark desktop and mobile,
focused keyboard grip, active drag, and 320px viewport.

Scheduler integration tests prove startup order **3, 1, 2** and a live reorder
while a slot is occupied, with tag filtering, producing **1, 3, 2**. Selection
and reservation occur in a fresh immediate SQLite transaction on every pickup.
Concurrent moves using the same revision yield one success and one conflict;
neither replaces the other's queue or content. CLI JSON and printed output were
also checked against a browser-created queue in another process.

A real Codex `/goal` worker in an isolated checkout picked issue **2** after it
was moved ahead of issue 1. It used the inferred working directory and project,
manually claimed the issue, verified exact file bytes, committed and pushed to
an isolated bare remote, completed its native goal, and closed issue 2. The
lower priority control remained open. This fixture used its own SQLite DB.

Evidence: `out/issue-order-*.log`, `out/issue-order-cli-browser.*`, and
`out/issue-order-native-smoke-20260918/`. Screenshots:
`output/playwright/issue-order-desktop.png` and
`output/playwright/issue-order-mobile.png`. Standalone checks are
`tools/issues_order_browser_checks.js` and
`tools/issues_order_accessibility.js`; the native smoke accepts
`HEY_BOSS_SMOKE_ORDER=1`. Installed verification uses the real Poe2 UI read-only.
The optimized CLI and skill were installed on this Mac and devbox. The Mac's
SQLite backup passed integrity checks before migration; all existing issue
content, claims, comments, and PRs were preserved. The companion service is
active on devbox, its 35 installation tests passed, and an isolated Linux CLI
fixture confirmed reordered output and PR grouping. Synthetic browsers and the
test server were closed; no real worker was started.

## Goal prefix, modal actions, list PRs, and offline workers

September 18 correction: **184 Rust tests passed**, one existing optional
real-agent control test ignored. Clippy with warnings denied, formatting, and
whitespace checks passed. **23 browser checks passed in Chromium and WebKit**,
including bare and multiline `/goal`, retaining the preview while typing, Save
and Cancel behavior, Escape/focus, mobile bounds, and opening multiple PR links
directly from the list. The click test found and fixed the issue title’s stretched
link overlay covering PR links. Seven accessibility states in each browser,
including the PR list, had zero violations.

A real Codex smoke test used an isolated checkout, a local bare Git remote, and a
separate SQLite DB. The CLI worker inferred its project and checkout with no
flags. The prompt’s `/goal` prefix activated a native goal; the agent retrieved
and manually claimed the issue using bare commands, verified exact file bytes,
committed, pushed main, completed the native goal, and closed the issue. The
project Instructions preview and cancel behavior were then checked on the
installed Poe2 UI without modifying user settings or starting real workers.

Offline integration tests run a real broker and three sequential scheduler jobs
without a Mac bridge. All three issues complete and close, the worker stops
without draining the outbox, and all updates replay after broker restart. A
second regression checks FIFO delivery, receiver rejection, stable IDs after
restart, lost acknowledgments, retained retries, and chronological ordering across
legacy and current queue IDs. Existing tests also prove
that malformed rejected legacy items remain recorded and do not block healthy
notifications. Replay now backs off exponentially with jitter, caps at 30
seconds, and resets when the bridge generation changes. Delivery remains at
least once; receiver duplicates after acknowledgment loss are acceptable.

Project CLI overview includes open, claimed, unassigned, closed, and deleted
counts; tests cover both structured counts and printed columns. Logs and native
smoke artifacts use `out/goal-prefix-*`; screenshot:
`output/playwright/goal-prefix-instructions.png`.

## CLI-only worker correction

The web app now contains **no worker setup, launch buttons, worker list, or
session/run pages**. Concurrency and tag filtering remain worker CLI flags;
directory comes from the CLI's current directory. Reservation timeout stays an
internal CLI default. The web server never launches or supervises workers.
Only project instructions, their exact dynamic preview, and PR behavior remain
in the UI. Storage implementation text was removed.

The 17 issue tests, 10 worker lifecycle tests, and 5 HTTP tests passed. The added
HTTP regression inserts an enabled legacy managed worker and confirms that the
web app does not reserve an issue or launch Codex. Browser checks verify the
absence of worker UI, prompt/PR preview and persistence, mobile bounds, and
closing/focus: 11 checks passed in each browser. Five accessibility states had
zero violations. Logs are `out/cli-only-worker-*.log`. Earlier worker UI results
below describe the replaced design.

## Independent workers, reservations, and PRs

September 18 follow-up: **174 Rust tests passed**, one existing optional real-agent
control test ignored. Formatting, whitespace checks, and Clippy with warnings
denied passed. Worker integration tests exercise real scheduler/child-process
lifecycles with a synthetic Codex protocol peer: independent capacities and tag
filters, atomic reservations, manual claiming, claim deadlines, takeover,
approval blocking, cancellation, process reaping, orphan recovery, and native
goal protocol ordering. A status regression includes 24 active sessions plus a
bounded completed history. Schema 1/2/3 migrations, project prompts, PR defaults,
multiple durable PR links, idempotence, and unsafe URL rejection are covered.

| Browser verification | Chromium | WebKit |
| --- | --- | --- |
| Worker settings, preview, slots/activity, reservations, PRs, tags, focus/mobile | 24 passed | 24 passed |
| Existing issue workflows | 32 passed | 32 passed |
| Markdown rendering and safety | 27 passed | 27 passed |
| Project activity/hide/restore | 9 passed | 9 passed |
| axe: workers, runs, tags, settings, PR sidebar, light/dark/mobile | 11 states; zero violations | 11 states; zero violations |

Two **real Codex** smoke tests ran in isolated Git repositories with separate
SQLite databases. Both manually claimed their issue, verified exact file bytes,
committed the requested file, completed the persisted native goal, and closed
the issue. The remote case also pushed main to an isolated local bare repository.
The live terminal captured slot counts, pipeline, claim countdown, session ID,
elapsed time, and latest activity. No real issue was used as a worker fixture.

Logs: `out/worker-independent-*.log`. Real Codex artifacts:
`out/worker-independent-native-smoke/` and
`out/worker-independent-native-remote-smoke/`. Browser fixture:
`out/worker-independent-browser/issues.db`. Final installed UI was reviewed
without enabling pickup on real projects. Two synthetic project discoveries
were softly hidden after the native tests. Backups were integrity checked before
installation; all user issue data remained in the normal SQLite database.

## Results

Project management follow-up: **147 Rust tests passed**, including schema-1 to
schema-2 migration, automatic registration, worktree discovery, activity order,
and hide/restore persistence. **Nine project browser checks** and the existing
**32 workflow checks** passed in Chromium. Hiding retains Markdown, comments,
claims, and numbering; subsequent discovery and activity do not restore hidden
projects. The project browser checks also cover keyboard focus during refresh
and mobile layout bounds. These follow-up logs are in `out/projects-*.log`.

| Check | Result |
| --- | --- |
| Full `cargo test --locked` | 143 passed; 1 existing live-Codex test ignored |
| Final issue CLI and HTTP integration suites | 15 passed |
| Formatting and Clippy with warnings denied | Passed |
| Optimized release build and whitespace checks | Passed |
| Complete browser workflows | 32 passed in Chromium and 32 in WebKit |
| Timing, recovery, and responsive checks | 17 passed in Chromium and 17 in WebKit |
| axe-core 4.10.3 | Zero violations across 10 desktop/mobile/light/dark states |
| Server restart with an open draft | Submission renewed CSRF and saved successfully |
| Real CLI write while browser remained open | Polling discovered the comment and loaded it |

The workflow suite covers project creation/switching, list filters and pagination,
Markdown previews and unsafe input, create/edit, claims/unassignment, comments,
close/reopen, delete/restore, activity, keyboard shortcuts, drafts, conflicting
edits, and a committed write whose response was lost. Retrying that write after
a full reload produces one comment.

Additional checks deliberately delay old project responses, finish refreshes
while search text is being typed, edit issues from another session while a draft
is open, and interrupt network access during submission. Layout and editor bounds
are checked at 320, 390, 768, 1024, and 1440 px with reduced motion enabled.

Accessibility states include desktop light/dark lists and details, desktop editor,
mobile detail, project picker, light/dark lists, and dark editor. Axe reported
some inconclusive checks involving transparency and ARIA relationships; keyboard
flows and screenshots were also reviewed. This is automated and visual coverage,
not a screen-reader certification. SSH routing has integration coverage using an
SSH test double; a live remote authority was not exercised here. The ignored
live-Codex test belongs to agent control, not the issue interface.

## Performance

HTTP timings use 60 warm requests per scenario over loopback. Timing includes
response transfer; JSON parsing happens afterward. The scale fixture contains
5,000 issues with roughly 4 KB Markdown bodies. These are local measurements,
not remote-network guarantees.

| Operation | Median | p95 |
| --- | ---: | ---: |
| Project list and counts | 7.27 ms | 13.69 ms |
| Small project issue list | 0.51 ms | 1.11 ms |
| 50-row page from 5,000 issues | 0.67 ms | 0.97 ms |
| Substring search over 5,000 titles/bodies | 35.20 ms | 56.70 ms |
| Issue detail with Markdown/comments | 0.53 ms | 0.73 ms |

Ten Chromium reloads measured a median first contentful paint of **36 ms**
(maximum 72 ms), and the list visible by **56 ms** (maximum 105 ms). List readiness
includes Playwright's observation and two animation frames. The four embedded
assets total **95,669 bytes** before HTTP headers. There is no frontend framework,
external asset request, or frontend build step. List responses omit Markdown
bodies, and the browser renders only one page of rows.

## Reproduce

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
git diff --check

# Creates a fresh, isolated database; refuses to overwrite an existing one.
python3 tools/seed_issues_web.py
HEY_BOSS_ISSUE_DB="$PWD/out/issues-web-review/issues.db" \
  target/release/hey-boss issue web --port 4782 --no-discovery \
  --project github.com/example/hey-boss --agent human:alex
```

In another terminal, with `playwright-cli` installed:

```sh
playwright-cli -s=issues open http://127.0.0.1:4782/ --headed
playwright-cli -s=issues --raw run-code --filename tools/issues_web_browser_checks.js
playwright-cli -s=issues --raw run-code --filename tools/issues_web_resilience_checks.js
playwright-cli -s=issues --raw run-code --filename tools/issues_web_project_checks.js
playwright-cli -s=issues --raw run-code --filename tools/issues_web_load_timings.js
python3 tools/benchmark_issues_web.py

playwright-cli -s=issues-webkit open http://127.0.0.1:4782/ --browser webkit
playwright-cli -s=issues-webkit --raw run-code --filename tools/issues_web_browser_checks.js
playwright-cli -s=issues-webkit --raw run-code --filename tools/issues_web_resilience_checks.js

npm install --prefix out/issues-web-tools --no-save axe-core@4.10.3
python3 tools/prepare_issues_web_accessibility.py \
  --axe out/issues-web-tools/node_modules/axe-core/axe.min.js \
  --output out/issues-web-review/axe-matrix.js
playwright-cli -s=issues --raw run-code --filename out/issues-web-review/axe-matrix.js
playwright-cli -s=issues close
playwright-cli -s=issues-webkit close
```

The scripts are standalone Playwright functions for `run-code`, not Playwright
Test specs. They require the synthetic server on port 4782 and create additional
QA issues only in that store. Accessibility output lists each state's violations
and inconclusive checks. Local run logs and timing JSON are in
`out/issues-web-review/`.

### Markdown regression fixture

Markdown follow-up: **27 checks passed in Chromium and WebKit**, plus the
existing **32 workflow checks** in Chromium. Axe found zero violations in the
reference issue across desktop/mobile and light/dark themes after making
overflowing code and tables keyboard-focusable. The Rust suite passed, with an
unrelated secret-CLI temporary-directory collision passing on isolated retry;
all four HTTP tests, renderer tests, formatting, Clippy, and the release build
passed. Evidence is in `out/markdown-*.log` and
`out/markdown-accessibility.json`. Screenshots use Chromium; Playwright's WebKit
screenshot helper injects an inline style that the app's CSP rejects, so the
WebKit check runs the same assertions without screenshots.

Against a separate test database and the server on port 4782, create the reference
issue once, then run the Markdown checks (the script adds test comments):

```sh
HEY_BOSS_ISSUE_DB="$PWD/out/issues-markdown-review/issues.db" \
  target/debug/hey-boss issue create --project 'named:Markdown QA' \
  --agent human:alex --title 'Markdown rendering reference' \
  --file tests/fixtures/issues-markdown.md
playwright-cli -s=markdown open http://127.0.0.1:4782/
playwright-cli -s=markdown --raw run-code --filename tools/issues_web_markdown_checks.js
playwright-cli -s=markdown close
```

The fixture covers the reported bare PR URLs, punctuation, query strings,
emails, code literals, aligned tables under CSP, all five GitHub callouts, nested
lists, tasks, syntax colors, independent comment footnotes, previews, saved
comments, unsafe input, and 320/390/768/1440 px layouts. HTTP integration tests
also verify that preview, saved issue, and saved comment HTML agree and preserve
the original Markdown text. Native renderer tests retain source-line mapping
and the existing 21 Markdown example groups.

## Visual review

The interface follows the Hey Boss app's system typography, indigo accents,
slate palette, translucent surfaces, and rounded controls. Reviewed captures:

- [Desktop light list](../output/playwright/issues-desktop-light.png)
- [Desktop dark list](../output/playwright/issues-desktop-dark.png)
- [Issue detail, dark](../output/playwright/issues-detail-dark.png)
- [Issue detail, light](../output/playwright/issues-detail-light.png)
- [Mobile list](../output/playwright/issues-mobile-light.png)
- [Mobile detail](../output/playwright/issues-mobile-detail.png)
- [Mobile project switcher](../output/playwright/issues-mobile-projects.png)
- [Mobile editor](../output/playwright/issues-mobile-editor.png)

## Subtasks and second measured stability pass

Subtasks add normalized SQLite relationships, transactionally guarded cycles,
depth and direct-child capacity, version checks, histories on both endpoints,
atomic child creation, progress, breadcrumbs, link/unlink, ordering and shared
worker/fleet readiness. The web card shows directly clickable PRs, label/assignee
filters and nested progress; deleted relationships remain available to unlink.
Global Boss naming lives in profile Settings and retains `human:boss` ownership.

Preflight: **225 Rust tests passed**, one optional companion test ignored; strict
Clippy and formatting passed. Chrome and WebKit each passed **41 subtask workflow
checks**, **11 recovery checks**, **7 queue-host isolation checks**, **8 subtask
accessibility states**, **33 Inbox flow checks**, and **11 Inbox accessibility
states** with no violations. The original assignment UI passed **34 checks**.
The native optimized audit also passed.

The seeded graph-scale fixture contains **5,000 issues and 4,950 edges**. Both
browsers passed **14 checks** for full-list parent/progress links, queue order,
filtered context, parent navigation, all 99 direct children, narrow screens,
and a searchable 5,000-issue picker. HTTP graph list p95 was **49.13 ms**;
parent/child detail p95 was **13.21 / 14.26 ms** in 60 samples under test load.
These are local measurements, not cross-machine performance guarantees.

A seeded three-replica randomized run passed **1,000 edits, 294 sync cycles and
1,588 integrity checks**. All replicas converged and **126 conflicts** retained
attempted payloads. Durable canonical graph replay now handles temporary cycles,
pending edits, lost acknowledgments and snapshots that remove never-applied
links. Fresh replica number ranges initialize from their own reserved range;
legacy bootstrap includes subtask and PR rows before relationship history.

The second actual 7,200-second pass began at **2026-09-19 06:09:49 UTC**.
It completed at **08:09:49 UTC**, with **78,837 integration requests** and
**2,994 native requests**, zero failures and 7,200 seconds in each run. Proof and
terminal results are under `out/subtasks-phase2/endurance/final-report.json`. Earlier browser candidates
failed due to fixture assumptions, literal Boss naming, asynchronous initialization
or ambiguous short project names; fixes are recorded rather than counting those
candidates as clean endurance runs.

Second-pass navigation testing reproduced shortcuts using the old detail route
immediately after returning to the list. Navigation now saves the old comment
and updates history and route synchronously, with one render. The deterministic
regression fails on the original build and passes **50 checks per browser**,
including an actually held list response, rapid keyboard cycles, drafts and
Back/Forward. Another delayed-host regression reproduced local comment drafts
being copied into remote storage when cancelling a host transition; drafts now
use the displayed issue's host. **Six checks per browser** pass for cancellation
and late responses. The compiled candidate passed the **33-check full workflow**
in both browsers. WebKit's occasional fetch diagnostic during an immediate redundant reload was
classified with 30 workflow traces: all functional flows completed, zero window
errors or unhandled rejections, and positive controls caught deliberately uncaught
exceptions. Engine diagnostics remain recorded; they are not suppressed.

Page requests now abort on page exit and resume when restoring a cached page.
Both browsers pass six lifecycle checks. Hidden saved comment drafts no longer
trigger an exit warning from the issue list; eight checks per browser cover actual
reload, visible-draft protection and restored drafts. Both bugs were reproduced
against the older build.

The sustained Rust run exposed a timing-dependent worker upgrade failure: an older
updater could write its legacy `upgrading` marker after a long-lived store opened,
causing strict configuration parsing to stop active sessions. Worker status and
reservation now migrate that marker in their write transaction, preserve active
sessions and block new pickup while draining. A deterministic late-write test and
process-level upgrade repetitions pass. The final candidate `cdf0b00ed64633f1`
passes 226 Rust tests, one optional test ignored, strict Clippy and formatting.

One older browser loop stopped when its Markdown save assertion expected the
recent-comment DOM count to exceed 20. Captured page state showed a saved new
comment and cleared compose field. The harness now verifies the new comment ID;
full history remains stored and available through activity. The failed runs and
the timestamped harness correction are retained in the endurance evidence.

Latest installed candidate checks passed 66 fleet regressions, three global-profile
sync tests, seven upgrade tests, an isolated 13-check Linux worker/goal smoke, and
11 read-only installed web checks. Mac/devbox build IDs, live web assets and all
three installed skill copies matched `cdf0b00ed64633f1`. Synthetic desktop/mobile
subtask review images are under `output/playwright/phase2-subtask-demo-*`.

The long seeded three-replica edit run was deliberately interrupted after 6,847
acknowledged operations (the requested 10,000 was not counted as completed).
Native SQLite backups preserved its state. Recovery from those backups drained
189 pending journal entries, verified stable duplicate-ack receipts, converged
all three graphs, left no deferred relationships and retained 687 conflicts.
All database integrity/foreign-key checks passed. The first recovery attempt
finished assertions but hit a reporting-script name error; the corrected replay
and original failed report are retained separately.

A fresh seeded run against the final installed candidate passed 1,000 actual CLI
edits, 329 sync cycles and 1,658 integrity checks; all replicas converged and
66 conflicting payloads remained retained.

Final sustained candidate validation completed 16 full Chrome/WebKit rounds
(**10,400 checks**), 756 Markdown save/render repetitions (**20,412 checks**),
237 graph/upgrade/worker regression rounds (**11,139 checks**) and 137 successful
process-level upgrade-race repetitions. The browser round already in progress at
the measured deadline finished cleanly at 08:11:24 UTC. All owned synthetic
servers, native fixtures and browsers were stopped; the installed app and unrelated
browser sessions were preserved. The verified source and both installs remained
`cdf0b00ed64633f1`. No unresolved failures remain in the final runs; one optional
Rust companion test was ignored, with explicit Linux worker/goal smoke coverage
recorded separately.

## Project workflow settings · issue 39 · 2026-09-19

The settings dialog now separates shared instructions from workspace and delivery
choices. Both conditional branches remain editable; included branches are marked,
and each prompt distinguishes a built-in default from a project override. The
assembled preview sits beside the editor on desktop and below it on narrow
screens. Save failures retain edits and remain visible while the preview updates.

Verification used a separate synthetic `Workflow QA` database and Inbox socket:

- Chromium and WebKit each passed **53 interaction/layout checks**, including all
  four worktree/PR combinations, inactive overrides, default reset, goal display,
  persistence, save failure/retry, cancel, Escape, and focus restoration.
- Desktop at 1440 × 1000 and widths of 768, 390, and 320 pixels were checked for
  overflow, preview placement, and footer visibility. Light/dark screenshots were
  visually reviewed, including the 320-pixel WebKit preview.
- Axe found **zero violations in eight audits**: desktop/mobile, light/dark, in
  both browsers.
- **110 issue/web/worker integration tests**, six prompt unit tests, three worker
  CLI unit tests, and two fleet replication/legacy replay tests passed. Integration
  tests clear `HEY_BOSS_ISSUE_PROJECT` inherited from worker sessions.
- Rustfmt, clippy with warnings denied, JavaScript syntax, and diff checks passed.

The broader local health inspection tests timed out or failed their process/state
assertions, and one unrelated secret CLI test returned BrokenPipe. Those failures
were outside the changed feature; the feature suites above completed successfully.

The repeatable browser suite is `tools/issues_project_workflow_browser_checks.js`.
Review artifacts are under `output/playwright/issue39-*`, including
[desktop light](../output/playwright/issue39-desktop-light.png),
[desktop dark](../output/playwright/issue39-desktop-dark.png),
[mobile light](../output/playwright/issue39-mobile-light.png), and
[WebKit narrow preview](../output/playwright/issue39-webkit-mobile-preview-320.png).
