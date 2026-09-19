# Mindmap implementation work log

The active user goal is the complete `hey-boss mm` feature, including upfront tool
research, ergonomic CLI authoring, a read-only nested-list web viewer, text/Markdown,
issues, automatic issue/PR relationships, PR dependencies, cross-project references,
pending-only notifications and optional link descriptions. The user requested eight
hours of work; this first implementation milestone does not complete that duration
or the full verification audit.

## Authoritative workspace

- Original checkout: `/Users/kjopek/Workspace/hey-boss` (preserved).
- Feature worktree: `/Users/kjopek/Workspace/hey-boss-mindmaps`.
- Branch: `feature/mindmaps`.
- Upstream fetched at start: `origin/main` / `bdecd9b`.
- Local dependency baseline: `ced1b16`, a snapshot of the existing uncommitted source
  needed by this feature. It is not published. The upstream commit lacks the issue
  store, Inbox and web app required by mindmaps. A feature-only PR must account for
  those unpublished dependencies; do not silently publish the whole local baseline.
- Inspect current Git and remote state before resuming or publishing.

## Completed first milestone

- Live primary-source research into Freeplane connector labels, XMind SDK relationship
  titles/endpoints and Markmap's CLI. Ran real Markmap help and offline HTML generation.
- SQLite schema 10: project maps, typed resource nodes, aliases, hierarchy, ordering,
  directed links, optional descriptions, map revisions and resource uniqueness.
- Full `mm` CLI with default show, projects, export, add, issue, pr, notice, edit,
  move, remove, link, unlink, links and web; JSON, host, author, retry and version options.
- Qualified project selectors and atomic typed shorthand endpoint creation.
- Live issue projection and automatic issue→PR relationships from existing attachments.
- Pending-only Inbox projection; unavailable Inbox is explicit and unverified notices
  are hidden without deleting saved references. Child topics remain visible.
- Read-only `/mm` viewer with nested lists, Markdown, search, collapse/expand, refresh,
  project switching, incoming/outgoing relationship labels and cross-project navigation.
- Both `/api/mm` and the general `/api/action` reject map writes.
- Documentation and repository skill updated. Installed CLI/skill not upgraded yet.

## Verification evidence

Commands executed in the feature worktree:

- `cargo check --quiet`: passed.
- `cargo test --test mindmaps --quiet`: 7 passed.
- `cargo test --test issues --test issues_web --quiet`: 36 issue + 14 HTTP tests passed.
- `cargo fmt --check`, `git diff --check`, `node --check src/issues/web/mindmap.js`: passed.
- `target/debug/hey-boss mm --help`: inspected ergonomic command/selector help.
- Playwright session `hey-boss-mm` used an isolated SQLite/demo Inbox fixture.
  Viewed real map rendering; followed a cross-project dependency and verified focused
  target plus backlink; searched a link description; confirmed completed notice omitted
  and pending review shown; console had zero errors or warnings.
- Screenshots: `output/playwright/mindmap-desktop.png` and `mindmap-mobile.png` (390×844).
- Markmap offline artifact: `out/mindmap-markmap-reproduction.html`.

These checks prove a working first implementation. They do not yet prove every
requirement at final delivery scope.

## Next work

1. Exercise actual CLI and HTTP notification projection with a durable reusable synthetic
   Inbox fixture, including completion transitions, unavailability, recovery and no reads.
2. Benchmark substantial maps/links and inspect rendering and response-size limits; remove
   expensive repeated scans and avoid unreadably expanded resource bodies.
3. Cover aliases/names with ambiguity, worktree defaults, cross-project revision/deletion
   behavior, reference normalization, depth limits and latest-state refresh failures.
4. Review whether explicit PRs should reveal their attached issues even when those issue
   nodes have not been manually added to any map; keep automatic relationships useful.
5. Review CLI usability end to end with realistic multi-project planning; improve import/
   batch authoring only if it serves the original big-picture workflow.
6. Revalidate original/upstream dependency state, prepare a reviewable change and appropriate
   PR/CI workflow without including unrelated unpublished user development by accident.
7. Install/upgrade the CLI and sync the repository skill when ready, test actual installed
   behavior, complete requirement-by-requirement audit, then notify once with hey-boss.

Do not mark the goal complete based on this milestone. Continue the requested eight-hour
work period and verify the full goal against current source/runtime/external state.

## Second milestone: automatic context and live Inbox verification

Current changes add automatic PR→issue context even when the issue has not been
manually placed in a map. The projected endpoint opens the real issue; it is not
persisted as a map node. Multiple map mirrors resolve to one preferred endpoint,
and normalized URL variants are deduplicated. Saved nodes and manual links remain
independent of attachment lifecycle.

Reusable synthetic desktop bridge: `tests/support/mindmap_inbox.rs`. Actual CLI and
HTTP tests now drive completion/cancellation, failure and recovery through the Unix
socket protocol; captured requests contain only `inbox_list`, proving that map reads
do not mark notices read or complete reviews. Projection preserves saved references
and restores nested child placement when a pending notice becomes visible again.

Measured an isolated 2,500-node, 5,000-link map before optimization: 1,372,623 bytes,
2.595 seconds for a debug CLI show. Replaced repeated endpoint membership scans with
a set and PR lookups with a map. Browser render/search now use incident-link lists
instead of scanning every link for each node. A later three-sample debug run measured
0.288, 2.204 and 0.434 seconds (median 0.434); other repository tests were running,
so this is evidence of improvement rather than a clean performance guarantee.
`tools/benchmark_mindmaps.py` reproduces an isolated fixture and reports sample times.
Lookup indexes are additive, including earlier schema-10 feature databases.

Issue bodies now use expandable native details controls to keep the big picture
visible. The skip-to-outline link preserves project navigation rather than replacing
the application route. Browser reload/project focus verified after the changes.

Remaining next priorities after the changes below include larger browser measurements,
worktree/default/ambiguity/concurrency coverage, CLI batch authoring review and the
unpublished dependency/delivery issue described above. The eight-hour goal remains
active; these milestones are progress, not a completion audit.

Compact mutation responses are now implemented and verified: an isolated map with
200 topics containing several megabytes of body text returns/caches a receipt under
4 KiB for adding one small topic. Identical request-ID retries return the same compact
result without duplicating the node. Link acknowledgments include resolved endpoint
IDs and affected project revisions. Authoring notice references no longer waits for
an Inbox snapshot; reads remain pending-only. Qualified `links PROJECT::node` now
reads that node's map so it includes relationships beyond the invoking project.
Notification selection metadata in read responses also follows live pending visibility.

A quieter three-sample 2,500-node/5,000-link benchmark measured 0.545, 0.323 and 0.239
seconds (median 0.323), still on a debug binary. No release performance claim yet.
Selectors now preserve double colons inside typed references (e.g. IPv6 PR URLs and
notice IDs) instead of interpreting them as project qualification.

Attempted the official legacy XMind SDK at its current cloned commit. The Python 3
runtime cannot load its Python-2-string implementation; recorded the exact failure
and API source in the research document. The SDK was not patched or globally installed.
The actual Markmap CLI reproduction remains the successful executable example.

The maximum supported saved-node fixture was also exercised: 10,000 nodes and 20,000
links, a 5,517,123-byte reply, and a single debug CLI sample of 0.642 seconds. This
proves the projection can handle the saved-node limit with small text bodies, not
that arbitrarily large Markdown maps have been verified. Large-body response limits
and browser behavior remain audit items.

Existing typed issue selectors now remain usable for link inspection, unlinking and
removal after the underlying issue has been deleted; creating a new reference still
requires an existing issue. The regression test confirms that graph removal preserves
the issue's deletion/history record. Database errors in version lookup are propagated
as operational errors rather than disguised as optimistic conflicts.

Browser verification showed an automatic unplaced issue link correctly opens
`/#project=named%3APlatform&issue=2`, and issue details are collapsed in the outline.
A CLI/server reload renewed the CSRF token and exposed an interrupted navigation case.
The viewer now reconnects once for read requests rejected with a stale token; persistent
failure remains explicit. The live reload/recovery browser check passed: after the token renewal, following
the cross-project link produced `status: Connected`, `errorHidden: true` and focus
on the requested Platform node without pressing Refresh. The browser recorded the
expected initial HTTP 403 used to exercise the reconnect path; no script error was
claimed absent from that request log.

Latest focused test evidence: 14 mindmap tests pass, including actual CLI Inbox
projection, response/receipt bounds, selector edge cases, deleted resource management
and qualified link inspection. The actual HTTP projection and read-only boundary
checks also pass; the 36 issue regressions passed after additive index changes.
The entire requested duration and delivery audit are still incomplete.

## Third milestone: bounded bodies and large-map rendering

Map reads support `full`, `preview` and `none` body modes. SQLite selects bounded
prefixes for saved topic bodies, previews preserve Unicode boundaries, and complete
source stays unchanged. `mm view NODE` reads one full live node, including qualified
cross-project selectors and pending-only notices. Terminal outlines default to titles
and relationships; JSON retains its full-body default and Markdown export stays full.
The viewer uses previews and loads individual bodies on demand, with explicit failure
and retry controls and generation/map/resource-version guards against stale results.

Focused verification: 17 mindmap and 16 web tests pass. The new cases exercise
40 large Unicode Markdown bodies, full single-node reads, terminal/JSON defaults,
live issue text, pending notice views and both HTTP read-only boundaries. Formatting,
JavaScript syntax and diff whitespace checks pass.

Playwright verified a 52 KB note reaches its final sentence when loaded and returns
to a short preview with “Show less”; keyboard focus remains on the body. A simulated
503 leaves the preview visible and enables retry; retry loads complete text and clears
the error. Expected HTTP 503/403 entries are recorded in the browser request log.
A successful full-body response delayed by 750 ms while switching from Atlas to
Platform did not insert Atlas content into Platform: zero full-body controls, zero
body errors, correct project title and Connected status after the delayed reply.

For a 2,500-node/5,000-link browser fixture, initial collapsed rendering had 25 rows
and 672 DOM elements; expanded rendering had 2,500 rows and 60,097 elements. Collapsed
branches now omit descendant DOM. Root sections of maps above 200 nodes start collapsed
on first visit. Search covers titles, previews, loaded bodies and link descriptions;
it does not fetch all complete bodies. The viewer had no horizontal overflow in the
measured fixture. These are fixture observations, not a general performance guarantee.

The original development checkout remains unchanged. A fresh upstream fetch still
leaves the unpublished dependency baseline unresolved for publication. Installation,
final requirement audit and the requested eight-hour duration remain incomplete.

## Fourth milestone: authoring identity and transactional coverage

`mm alias NODE NAME` and `mm alias NODE --clear` change readable selectors while
preserving generated identity, hierarchy and links. They work for saved resource
nodes as well as topics. Collision/validation errors roll back; unchanged aliases
do not increment revisions. Request receipts permit retrying the old selector after
it has been renamed. Documentation and repository skill describe the behavior.

22 mindmap and 16 HTTP tests pass. New end-to-end cases verify Git worktrees share
their default map, worker project environment overrides work, explicit projects take
precedence, ambiguous names require full IDs, simultaneous versioned edits commit
one change, simultaneous identical retries create one node, and recursive deletion
increments each affected neighboring project exactly once. Alias lifecycle coverage
checks cross-project projection and preserved links, resources, retries and conflicts.

On the collapsed 2,500-node fixture, searching for the last topic revealed that node
and its ancestor with seven visible rows and no horizontal overflow. Focus navigation
now clears a search that would otherwise hide an existing requested target.

A comparison against the original development checkout found 19 files changed since
the initial dependency snapshot, including worker/store/web code. The original checkout
has not been altered. Integrating these newer local dependencies into the isolated
feature branch is the next delivery step; none of these snapshots should be silently
published as a feature-only change.

## Local dependency integration

Created unpublished snapshot `4721445` on `dependency/mindmap-refresh-20260919` from
the current original development tree, using a temporary Git index. The snapshot
also includes newer local mobile and GitHub drain work; it remains local. Neither
the original checkout nor its index/branch was changed.

Merged the snapshot into the feature worktree. The store-open overlap required keeping
the newer WAL/deferred-read behavior and adapting its schema guard/migration threshold
to version 10. Additive mindmap indexes are installed only when absent so repeated
reads do not contend with existing writers. New regression coverage upgrades an
actual schema-9 fixture, preserves its issue, and reads both its map and a single
node during an uncommitted writer transaction without seeing uncommitted text.

After integration: 23 mindmap, 16 HTTP and 37 issue tests pass. Formatting, whitespace
and JavaScript syntax checks pass. Browser focus navigation from a search with no
matches revealed the requested last topic, cleared the obstructing search and focused
the target while Connected. Installation and the final duration/audit remain pending.

## Bounded projection responses and focused relationships

Reads now have a 32 MiB serialized-response budget, with space reserved for envelopes
and metadata. Values are charged incrementally while projecting nodes and links,
including automatic relationships, before collecting arbitrarily large responses.
The counter writes to a byte-counting sink instead of allocating another serialized
copy. Oversized full-body reads return an explicit error directing the caller to
previews, omitted bodies, single-node text or focused relationship reads. No content
is removed or changed to satisfy the budget.

`links NODE` now fetches only the selected node and incident relationships/endpoints,
with bodies omitted. Automatic issue/PR edges retain the same authoritative endpoints;
focused issue reads still reuse explicit saved PRs. General `links` omits bodies too.
Notification enrichment follows the budget and propagates size errors through both
CLI and HTTP. It still reads only pending Inbox state.

25 mindmap and 16 HTTP tests pass. New evidence covers a full-body Unicode map above
the budget, a 2,100-link fixture with maximum-size descriptions, and 30 pending notices
with large live summaries. Previews and individual text reads remain available, focused
relationships remain available when a complete description-heavy map cannot fit, and
mutations continue preserving all saved nodes and descriptions. Focused automatic PR
relationships are compared against full projection. The large Inbox case exposed and
fixed macOS nonblocking flag inheritance in the synthetic bridge; the fixture now uses
blocking accepted streams and avoids a second destructor panic during test failure.

Post-change isolated debug benchmark, 2,500 nodes/5,000 links: 1,472,642 response bytes,
three samples 0.242/0.250/0.282 seconds (median 0.250). These are local observations,
not a release performance guarantee. Formatting and whitespace checks pass; the
existing-project read-under-writer-lock issue regression passes after the changes.

Next audit items include project-revision behavior when both link endpoints belong
to projects other than the invoking project, installation/delivery reconciliation,
and final requirement checks. The requested eight-hour goal remains active.

## Foreign link revisions

Linking two nodes in other projects no longer increments the unrelated invoking
project's revision. Link/unlink already records endpoint projects and any implicit
reference-node creations; the generic revision update now applies only to other
node mutations. Regression coverage verifies both endpoint maps advance exactly once,
the unrelated outline remains unchanged, identical links are no-ops, unlinking behaves
the same way, and implicit foreign PR creation advances its owning map once.
26 mindmap tests pass; formatting and diff whitespace checks pass. Documentation
clarifies that optimistic guards check the selected map, so link authors should select
an endpoint project when using `--if-version`.

Further useful audit work: measure terminal outline rendering at 10,000 nodes (it still
rescans the node array at each hierarchy level), review optional readable PR titles
for big-picture authoring, and exercise installed CLI/skill delivery against the latest
original development state. The original checkout remains untouched; no readiness
notification has been sent and the eight-hour goal is not complete.

## Terminal scalability and readable PR labels

Extended the isolated benchmark with a terminal outline mode. A 10,000-node,
20,000-link debug outline took 64.356 seconds before optimizing CLI display. Terminal
rendering repeatedly scanned every node for children and again for each link label.
The renderer now builds child and label indexes once, preserving sibling order.
The same fixture after the change returned 1,673,328 bytes and all 10,000 topic rows
in 0.516/0.530/0.529 seconds (median 0.529). These are local debug measurements.

PR creation accepts `--title LABEL`; saved PR labels can be changed with `edit --title`.
URLs, native issue attachments, identities and dependency links stay intact. PRs accept
title edits only; issue and notice text remains live. Single-node terminal views show
the URL and Markdown export links the label to the PR. Export escapes literal node
title punctuation so labels containing brackets, angle brackets or emphasis characters
retain their text when rendered. Unchanged edits no longer rewrite updated timestamps.
Documentation and the repository skill include the new option.

27 mindmap and 16 HTTP tests pass, including rendered Markdown export, native PR URLs,
automatic relationships, dependencies, label edits and no-op timestamps. Formatting
and diff whitespace checks pass.

The old demo handle 7830 was confirmed terminal: it exited during the earlier schema
guard merge error, rather than merely timing out. Recreated it with the durable
`tools/serve_mindmap_fixture.py` runner, which confines databases to `out/`, serves a
synthetic read-only Inbox, records requests and reloads after binary changes. New
live demo handle: 28591, port 59479, the same preserved fixture DB. Browser verification
shows Navigation polish → Project outline, correct GitHub URL, pending Review release,
hidden read notice, Connected status and no mobile horizontal overflow. Screenshot
`output/playwright/mm-pr-label-mobile.png` was visually inspected. The recorded bridge
request was only `inbox_list`; no notice mutation occurred.

The original checkout and installed CLI/skill remain unchanged. Further audit/delivery
work and the requested eight-hour duration remain incomplete.

Installed-command preflight: `/opt/homebrew/bin/hey-boss` reports version 0.1.0,
build `cd9c1b256d76f225`; `hey-boss mm --help` rejects the unrecognized subcommand.
Thus the worktree binary is proven, while actual installed availability is not yet
delivered. Help/version checks did not access or migrate a real issue database.
Next delivery work must reconcile newer original development before installing a
coherent local build and skill; do not replace it with an older dependency snapshot.

## Installed delivery verification

Fresh local dependency snapshot `22b6bba`, parent `4721445`, has the same tree as
the reconciled dependency snapshot. Its source build ID is `cd9c1b256d76f225`, matching
the installed predecessor exactly. The source delivery therefore preserves the
already-installed user development baseline.

Checked and applied the feature-only patch from that snapshot to `19cac67` in the
original checkout: 25 files, 4,404 insertions and 21 deletions. Existing user development
is preserved, and the original index remains unstaged. The original checkout now
contains the feature; earlier untouched-checkout statements describe prior milestones.
Local ref `delivery/mindmap-applied-20260919` records `19cac67` as the applied anchor.
Use changes after this anchor for later delivery updates, checking current original
state before applying. Nothing has been published.

Original and feature source identities both matched `0d1bb89f8c1de2a6`. The local-only
upgrader's initial check reported the installed predecessor outdated. By the apply
call, the installed build already matched; the upgrader reported `current`. Verified
actual installed `/opt/homebrew/bin/hey-boss --version` and `mm --help`: mindmap commands
are available. All three installed skill copies (.codex/.agents/.claude) match the
repository guide. Upgrade source remains the original development checkout.

Actual installed CLI end-to-end smoke uses `out/mindmap-installed-smoke/issues.db`
and the synthetic Inbox: issue creation and PR attachment, nested outline, readable
PR labels, automatic relationships, cross-project dependency with description,
one-step typed PR dependency creation, aliases, pending/read notice filtering,
single-node text and focused links all pass. Its projected Atlas map has five visible
nodes and three links; pending notification availability is confirmed.

Started a fresh server from the installed command on an ephemeral port against that
fixture. `/mm` serves successfully and the preview API returns the map. Alias writes
through both `/api/mm` and `/api/action` return HTTP 403/forbidden; direct SQLite checks
confirm the saved alias remains unchanged. The temporary smoke server was terminated
after checks. Results are saved under `out/mindmap-installed-smoke/`; production data
was not used for these checks.

Delivery is now proven for the installed local command. The full goal remains active:
continue the requested eight-hour work period, inspect cross-project resource labels
and unavailable-resource navigation, check long PR URL authorship consistency, and
perform the final requirement audit before readiness notification/completion.

## Resource context and long PR references

Mirrored issue nodes now project `reference_project_name` alongside their stable
resource project ID. CLI `view` prints the project, issue number and full project
ID. The viewer labels foreign issue-opening links with the source project and
adds issue context to relationship endpoints; search also matches resource project
names/IDs. Deleted resources retain their map node and unavailable state without
an issue-opening link.

Explicit `mm pr` and typed PR creation now accept the same 2,048-byte resource
URLs. Custom labels and ordinary topic titles remain limited to 512 bytes. PR
title edits can restore the full reference URL, including equivalent trailing
slash forms. Failed over-limit typed creation remains atomic.

Focused verification: 29 mindmap tests and 16 web tests passed. New CLI fixtures
cover both URL creation paths, restoring full URL labels, rejected oversized
custom/topic labels and atomic rejection of oversized references. Mirrored issue
views retain source context and node identity after resource deletion. Actual
390×844 browser snapshot and screenshot verify the foreign-project label and
unavailable issue presentation using isolated demo data. Browser session still
contained earlier failure-test routes; cleared those routes and disabled cache
before checking the current assets. Screenshot:
`output/playwright/mm-reference-mobile.png`.

Applied resource-context/URL fixes from commit `d1f6255` to the original checkout
after `git apply --check`, preserving its unstaged development. Advanced the
delivery anchor to that commit. Local-only source upgrade reported `updated`,
and the installed command now reports build `cae9b1248d409db3`. Actual installed
`mm view mirrored-api` prints `Platform · issue #1 · named:Platform`. Isolated
installed CLI checks also create a long PR URL, give it a readable label, restore
its URL label and read the same reference successfully; results are saved in
`out/mindmap-installed-smoke/long-url-result.json`.

Export follow-up: plain relationship labels, descriptions and project names now
escape literal Markdown punctuation consistently with node titles. A rendered
export fixture verifies title emphasis/brackets/code and explanatory text stay
literal. The existing Markdown renderer still autolinks a bare URL; the test
checks that `[ship](URL)` remains visible as literal syntax around that URL,
rather than becoming a Markdown link labelled “ship”.

## Authoritative maps on fleet machines

Confirmed `tools/fleet_hey_boss.py` journals a fixed `TABLES` list that excludes
mindmap tables. This feature does not introduce offline graph replication.
Local map operations on a fleet agent now return an explicit requirement to use
`--host CONTROLLER` / `HEY_BOSS_ISSUE_HOST`, instead of exposing an unsynchronized
map as though it were authoritative. Existing local map data is preserved;
standalone and controller stores retain ordinary local behavior. Documented the
same host selection for CLI reads, edits and the web viewer in the guide/skill.

31 mindmap tests pass after the guard. A focused follow-up fixture runs actual
CLI/RPC with an SSH shim targeting a separate synthetic controller database:
remote reads see its map, remote authoring saves there, and a failed transport
returns an explicit no-local-fallback error. Reverting the fixture role confirms
the caller's original map/version are unchanged. This exercises transport
routing without claiming a real network/SSH fleet deployment.

The export patch was delivered through checked incremental application to the
original checkout; delivery anchor is `c47a1c1`. Its local source upgrade reported
`updated` and installed build `b2ee3a6c43891a4d` was verified. Fleet guard/skill
delivery is the next incremental update.

Fleet guard/skill changes from `6657bb7` were checked and applied to the original
checkout; the delivery anchor advanced to that commit. Local-only installation
reported `updated`, and actual installed build is `a9d3929d4d0a8ab0`. Isolated
installed replica reads and writes return exit 2 with the authoritative-host
instruction, preserving the one existing note. All three installed skill copies
match the current repository guide. Results:
`out/mindmap-installed-smoke/replica-guard-result.json`.

Added `tools/soak_mindmaps.py` for a fresh database under this checkout's `out/`.
It exercises a real persistent viewer server during repeated CLI issue/text/link
edits, 512-character Unicode previews, full body reads, focused body-free link
reads, pending/read/unavailable synthetic Inbox snapshots and forbidden writes
through both web routes. Samples include owned-server RSS and SQLite integrity;
Inbox requests are recorded. It terminates its own server/socket in cleanup.

The first preflight failed before any cycle because the harness appended API
paths to the announced `/mm` URL. Corrected it to use the URL origin. Installed
30-second preflight passed 13 complete cycles and clean service shutdown;
initial server RSS was 12,384 KiB. Started the one-hour installed run under
`out/mm-soak-hour-20260919`; its outcome is pending and must not be reported as
passed until its final result is observed.

Delivered the endurance tool/log increment from `cf08bf8` to the original checkout
after checking the patch; that commit is the current applied anchor. This tool is
outside runtime source identity, so it requires no CLI replacement by itself.

Mobile metadata boundary: a valid 128-byte alias occupied 921.75 CSS pixels and
was clipped by the outline panel at a 390-pixel viewport. Allowed metadata flex
items to shrink/wrap, including foreign project labels, and let project headings
wrap long names. Actual browser measurement after rebuilding/reloading shows the
same alias at 275 pixels with no metadata element extending beyond the viewport.
Screenshot: `output/playwright/mm-long-alias-mobile.png`.

Checked/applied the wrapping increment `81c746f` to the original checkout and
advanced the delivery anchor. Local-only upgrade reported `updated`; actual
installed build is `a9e607b2412ce7d6`. A fresh installed ephemeral server serves
the verified wrapping stylesheet; it was terminated after the asset check.
Result: `out/mindmap-installed-smoke/wrapping-result.json`.

The one-hour endurance server remains the process started with build
`a9d3929d4d0a8ab0`; subsequent CLI cycles use the replaced installed binary.
The intervening runtime change is CSS only. At the 187.79-second sample, 81
complete cycles had no failure and owned-server RSS was 12,192 KiB. The run is
still pending; do not restart it merely because a tool wait yields or times out.

Later observed actual termination of that run at round 99 / 231.09 seconds:
HTTP 403 “Reload the page to reconnect to this server” after CLI replacement.
The source upgrade renews web tokens even in existing server processes, and
the viewer already recovers by fetching bootstrap/retrying the read. The harness
did not yet model this recovery. Added a single bootstrap/token retry for its
read commands only; intentional forbidden authoring requests are never retried.
The failed run was stopped cleanly. A fresh run is required; its earlier samples
do not establish a completed one-hour result.

The corrected 30-second preflight passed 13 complete cycles and clean shutdown.
Started the new one-hour installed run under
`out/mm-soak-hour-recovery-20260919` on build `a9e607b2412ce7d6`, port 59210.
At round 8 / 17.23 seconds, an intentional stale-token probe successfully fetched
bootstrap and retried the read; subsequent cycles continue. Its first RSS sample
was 12,496 KiB. The one-hour outcome remains pending. Active tool session: 55313;
earlier hour session 61260 is terminal (failed), and preflight session 3273 is
terminal (passed). Do not restart the active run on tool yield/timeout alone.

Applied the harness recovery/log increment `2f91197` to the original checkout
after checking the patch; no runtime rebuild is needed for this tool-only change.
Native goal report remains active at 6,951 seconds used (~116 minutes): the user's
eight-hour duration remains incomplete. No readiness notification or publication.
