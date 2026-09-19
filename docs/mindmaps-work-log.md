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

Checkpoint continuation: added native assignment labels/search (including the
current Boss profile), excluded hidden projects from the normal picker while
preserving direct cross-map navigation, and added focused `view --bodies`
preview/none/full reads. Show less now fetches current preview metadata rather
than restoring an old initial projection. Per-node map/resource revisions reject
rollback. Failed previews retain full text and offer a focused retry; a renamed
resource remains visible when the original search stops matching, while newer
search input/focus survives an in-flight read.

Verification: 31 mindmap and 16 web tests passed. Actual mobile browser checks
covered assignment search, hidden-project direct links, native metadata updates,
preview 503/retry, and delayed preview with newer search input. The text-node
revision test fetched revision 18 after initial revision 17, then injected an
older preview with a stale sentinel: fresh title/full text remained, the sentinel
was absent, the revision error appeared, and retry received focus. Evidence:
`out/mm-text-revision-result.txt`. Cleared browser routes and restored the isolated
demo's issue/title/body/assignment, Boss name, and long planning text afterward.

The restarted endurance run did fail at round 836 / 1910.36 seconds. Its terminal
server log is `error: unrecognized subcommand 'mm'`; return code 2. The server
detects executable replacement and execs that path with its original arguments,
so a replacement without mm interrupted the measurement. The currently installed
binary supports mm again. This run is not a passed hour or a memory-failure
diagnosis. The harness now copies its input CLI into its owned output directory
and uses that fixed build for both reads and authoring, isolating endurance from
concurrent installation changes. Upgrade resilience remains a separate concern.

Also reproduced the unmodified XMind SDK relationship read API on existing XML
DOM nodes using Python 3.14.7: endpoint IDs and labeled/unlabeled titles worked.
The research guide records exact scope; legacy workbook load/create limitations
remain documented.

## Visualization pass after the checkpoint

The native goal now reads “Add me nice visualization for the mindmap, work on it
for 4 hours, it must be super fast slick” (created 2026-09-19 15:47:01 UTC).
Earlier eight-hour entries are historical implementation checkpoints. This
visualization pass remains active and its requested duration is incomplete.

Added a dependency-free branching map with HTML topic cards, curved SVG hierarchy,
directional selected-node links, pan/zoom, touch dragging/pinching, branch collapse,
Fit and clickable canvas overview. Details retain native issue context, rendered
Markdown, live focused body reads and cross-project relationships. Outline remains
available. Search reveals/highlights matches at a readable scale, preserves input
focus and restores the earlier camera when cleared. Selection centers cards in
the space beside the desktop inspector; direct links focus visible details.

TDD: the layout test first failed with the missing renderer, then passed hierarchy
ordering, spacing, branch colors, collapse, search ancestor paths, orphan roots,
empty maps and viewport culling. A 10,000-node collapsed fixture computes 25 cards
in roughly 2–4 ms locally. Added the JavaScript checks to CI and the renderer to
same-origin HTTP asset coverage. No external drawing/runtime library is loaded.

Actual browser checks on a 2,500-node/5,000-link fixture: Expand all mounted 7 cards
initially and 12 after the measured pan; outline DOM stayed empty. Twenty automated
wheel + two-animation-frame waits took 39–45 ms each including automation/frame
waiting, not isolated draw timing. Evidence `out/mm-map-scale-pan-result.txt`.
Mobile search kept input focus and showed a 168-CSS-pixel matching card. Selected
long Markdown loaded 53,047 rendered characters, returned to preview, retained
body keyboard focus, and caused no horizontal overflow. Native browser touch input
starting on a card panned without selecting; a pinch changed zoom from 70% to 140%.
Evidence `out/mm-map-touch-result.txt`. Cross-project native selection and backlinks
worked; desktop selected issue showed assignment and a directional relationship.

The focused-read checkpoint is committed on current main as `98ad110`; installed
build `f527bf288bc6916e` and all three installed skill copies were verified. An
installed focused preview has 512 Unicode characters. The visualization increment
has not yet been delivered/installed. The fixed-build one-hour endurance run is
active under `out/mm-soak-pinned-hour-20260919`, tool session 76982, port 63448,
build `1a5650ac270a9abc`. Its outcome remains pending.

Later pan instrumentation measured the actual draw method at 0.5–0.8 ms across
twenty frames, with 12 cards, 13 SVG paths (including the arrow definition), and
zero outline nodes. Instrumentation was restored afterward. Evidence:
`out/mm-map-scale-draw-result.txt`. Tightened mobile chrome; at 390×844, the map
navigation controls end at y=800.56 and stay visible without horizontal overflow.
Opening cards now uses at least 80% scale. Topic details include child navigation
(first 50, with complete Outline navigation for larger child lists), avoiding long
pans on phones. Native child selection retained heading focus and assignment.

Applied the visualization increment to current main without staging concurrent
worker/claim edits; main commit `0266a46`, feature commit `1a3ce52`. Merged current
main into the worktree; normalized delivery anchor is `188d480`. Current-main
verification passed 31 map + 18 web tests. Installed build `f72f6387d8a53c86`
serves the exact current renderer/CSS/HTML bytes and an isolated map read;
`out/mindmap-installed-smoke/visualization-result.json` records the asset hashes.

Reproduced a global Expand all regression on the 2,500-node fixture: the visible
root disappeared after layout changed. Global expansion/collapse now preserve
the nearest root's screen position. The real browser retains that root with
0.000122 CSS-pixel expansion shift and zero collapse shift. Evidence:
`out/mm-map-global-expand-before.txt`, `out/mm-map-global-expand-after.txt`.
Closing details whose card is hidden now returns focus to a visible ancestor
or to the map surface.

Native omitted-body reads now project only metadata and body presence, using
SQLite octet_length so NUL-only bodies are still counted. Regression tests were
added before the change; the current suite passed 32 map + 18 web tests, with the
expanded Boss-assignment/closed-state case also passing separately. The synthetic
fixture explicitly closes its own Boss-assigned issue with --force. Full/preview
text behavior is preserved. A 500-issue/72,501-byte-body benchmark had local
omitted-body medians of 0.143 seconds before and 0.133 after; no large speed claim.
The research guide records the official SQLite function behavior and a real
3.53.4 NUL reproduction. Feature commit `91fc2af` is applied to original main;
installation of this increment is pending.

Reproduced an asynchronous details-focus regression with a completed, delayed
full note read: selecting a different native issue before completion retained
selection/search but lost the newer heading focus. The inspector now preserves
unchanged DOM, retains focus when updating the same topic, and starts at the top
when selecting another topic. Before/after completed-read evidence:
`out/mm-map-focus-race-before-completed.txt` and
`out/mm-map-focus-race-after-completed.txt` (53,051 raw Markdown characters;
focus now remains on the newer native topic). Routes were cleared afterward.
Complete child-topic Outline navigation now also focuses its destination.

Restored native issue context on map cards: foreign references include their
source project, assignments show the current display name, and a hover title
exposes complete text when the card truncates it. The real browser verified
`Platform · ISSUE #1 · open` and local `ISSUE #1 · open · Morgan`, preserving
the existing accessible assignment name. JavaScript syntax/layout checks and
the debug build passed.

Delayed focused reads also used to steal focus after switching from Map to
Outline. Completion/retry focus now follows the original reading focus only
when the user has kept it. Actual delayed success and synthetic 503 reproductions
preserve `view-outline`; an ordinary successful read still focuses the loaded
body (53,046 rendered characters in this run). Before/after evidence:
`out/mm-map-view-focus-before.txt`, `out/mm-map-view-focus-after.txt`, and
`out/mm-map-view-focus-success-error.txt`. Mock routes were removed afterward.

Expanded the actual browser fixture to 10,000 topics and 20,000 dependencies.
Expanded panning mounted at most 12 cards/15 SVG paths. Thirty instrumented draw
calls took 1–1.5 ms each, excluding browser paint; the expanded update took
5.2 ms. Search reveals the result and keeps input focus. Seven full input-handler
measurements ranged from 4.8 to 21.9 ms. Evidence:
`out/mm-map-10k-draw-result.txt` and `out/mm-map-10k-input-result.txt`.
Heap readings before/after this short sequence are not a memory-leak assessment.

Cached the graph's incident relationships and case-insensitive search text until
a map/focused content read changes it. This avoids rebuilding relationship text
on each input. TDD first failed for the missing index; regression coverage checks
Unicode/body text, aliases, state, native project/issue context, assignment display
names/IDs, dependency descriptions, neighboring live titles and unavailable
resources. The same seven real-browser input measurements on 10,000 topics now
range from 1.2 to 7.3 ms (previously 4.8–21.9 ms); local fixture observations.
Actual full→preview reads add/remove a unique tail-text search match. Renaming
the isolated Boss to Avery and reading another topic updates the assignee match;
the fixture's Morgan name was restored. Evidence:
`out/mm-map-10k-index-after.txt`, `out/mm-map-index-live-result.txt`, and
`out/mm-map-index-boss-result.txt`.

One hundred repeated large-map queries kept mounted cards at 12 maximum. Heap
used after explicit garbage collection changed from 12,832,460 to 12,958,352
bytes; this short controlled probe does not establish indefinite memory behavior.
Evidence `out/mm-map-10k-heap-result.txt`. Dark-mode screenshot was inspected;
reduced motion disables card transitions, Escape returns focus to the selected
card, and arrow/plus keys pan/zoom the map. At 320×740, controls stay visible and
the Zoom in button remains the top clickable element, with no page overflow.

Added compact previous/next search controls and Enter/Shift+Enter navigation,
with a current-match indicator in Map and Outline. The Outline search bar stays
visible when navigating distant results. A real resize reproduction previously
restored the desktop camera unchanged on a phone, mounting zero cards after
clearing search. Restoration now preserves the world center relative to the new
viewport; the same reproduction mounts four cards. Evidence:
`out/mm-map-search-navigation-before.txt`, `out/mm-map-search-navigation-after.txt`,
`out/mm-outline-search-position-before.txt`, `out/mm-outline-search-position-after.txt`,
`out/mm-map-resized-search-before.txt`, `out/mm-map-resized-search-after.txt`.
The 320×740 search screenshot was inspected. Narrow-phone overview dimensions
now leave a gap beside the complete navigation controls, including when the
browser reserves scrollbar width.

Added `tools/mindmap_scale_browser_checks.js`: 30 actual browser assertions cover
search counts/cycling/wrapping, IME composition, selection/Escape focus, camera
restoration with resizing, empty/single matches, Outline navigation visibility,
phone width/control spacing and keyboard pan/zoom. The first harness run hit a
JavaScript-expression semicolon wrapper issue; the next checks were corrected
to account for scrollbar width and wait for the scheduled clearing draw before
measuring keyboard movement. Final assertions passed. Evidence:
`out/mm-map-browser-checks-result.txt`. A pinned-build repeated UI run is active
until 19:40 UTC under `out/mm-map-ui-soak-20260919`; outcome pending.

The fixed-build native/server endurance run passed its full 3,600 seconds and
1,522 CLI/HTTP edit-read cycles. It exercised full/preview text, live native titles,
cross-project descriptions, automatic PR links, pending/unavailable Inbox states,
one stale-token recovery and repeated read-only mutation rejection. Server RSS
samples ranged from 14,304 to 18,416 KiB across 59 samples; clean server exit 0.
Evidence: `out/mm-soak-pinned-hour-20260919/samples.jsonl`. The earlier unpinned
run remains a failure caused by concurrent executable replacement; it is not
counted as a passing endurance measurement.

Delivered search navigation to original main as `1245d12`, then merged current
main into the worktree; normalized delivery anchor is `469b53a`. The four focused
web mindmap checks and JavaScript layout/index tests pass. Installation is now
build `dff79018d18d7777`: all four served map assets match current-main source
bytes, and the Codex/Agents/Claude skill copies match the repository skill.
Installed native reads preserve a NUL-leading Unicode fixture: omitted body
returns zero characters with body presence/assignment, preview returns 512,
and full returns all 514. The isolated smoke server stopped cleanly.
Evidence `out/mindmap-installed-current-20260919/result.json`.

Dense hubs exposed a separate bottleneck: a 2,500-topic/4,999-link fixture with
2,501 relationships incident to one topic drew all 2,501 dependency paths and
mounted 10,010 inspector elements. Measured JavaScript draw calls took
12.8–14.6 ms. Dependencies now draw only with both endpoints near the viewport;
same-column connections curve outside the cards. Topic details page dense lists
in groups of 50, retaining all targets/descriptions and usable pager focus.
The inspected after screenshot has 10 dependency paths, 211 inspector elements,
and draw calls of 1.1–1.2 ms, excluding paint. Evidence:
`out/mm-map-star-before.txt`, `out/mm-map-star-after.txt`, and screenshots
`output/playwright/mm-map-star-before.png`, `mm-map-star-after.png`.

`tools/mindmap_relationship_browser_checks.js` passed 60 actual-browser assertions.
It visits all 51 pages and compares every target ID and full description against
the saved graph: 2,501 entries, no skips/duplicates, at most 50 entries and 211
elements mounted. Pager remains inside the inspector and the last page leaves
enabled keyboard focus. Evidence `out/mm-map-relationship-browser-result.txt`.
The independent older-build UI endurance run continues; its native RSS samples
vary rather than grow monotonically in early large-map samples. No indefinite
memory conclusion yet.

Dense-hub changes are applied to original main as `e1e56bd` and installed as
build `6f91add4c986dc52`. Both the 30 general and 60 dense browser checks pass
on the latest debug code; four focused web boundary/asset checks pass on main.
Installed served assets and skill copies match current-main source, and the
NUL/Unicode focused-read smoke remains correct. Evidence:
`out/mindmap-installed-dense-20260919/result.json`.

Wrapped card titles were shrinking below two full line boxes: a 19.5-pixel line
height had only 35.17 pixels for two lines, and 12-pixel metadata shrank to 10.83.
Adjusted internal spacing/padding while retaining the 84-pixel card geometry.
The real browser now gives 39 pixels to the two title lines and 12 to metadata;
before/after dark screenshots were inspected. Evidence:
`out/mm-map-wrapped-title-before.txt`, `out/mm-map-wrapped-title-after.txt`.

Wrapped-label spacing is delivered as main `2be96fd` and installed build
`a5abc690832f1982`. The four served assets match current-main bytes; evidence
`out/mm-installed-label-assets-result.json`. The 320×740 wrapped-label screenshot
was inspected too: full unscaled 39/12-pixel line heights, no horizontal overflow.
This stylesheet change retains all card/edge layout dimensions.

Delayed text reads exposed relationship focus defects: two relationships sharing
a destination could restore focus to the wrong description in Map, and Outline
lost the focused link entirely. Relationships now carry their stable directional
identity; shared focus capture/recovery preserves the specific topic control,
relationship, body, heading or native issue summary in both views. Focus intent
uses that identity across DOM replacement, so completing an earlier Outline read
does not prevent a newer read from focusing its own loaded body.
Before/after evidence: `out/mm-map-link-focus-before.txt`,
`out/mm-map-link-focus-after.txt`, `out/mm-map-concurrent-focus-before.txt`,
`out/mm-map-concurrent-focus-after.txt`.

`tools/mindmap_focus_browser_checks.js` passes 19 assertions covering delayed
success and synthetic 503 failures in both views, same-destination relationships,
overlapping reads, and ordinary full/preview body focus. Mock routes are cleared
after every case. Evidence `out/mm-map-focus-browser-result.txt`.

Focus recovery is installed as build `4bf651fcf86415b6`; all four served
assets match main `59c4d08` (evidence
`out/mm-installed-focus-assets-result.json`).

Dependency descriptions already existed in SVG titles, but the edge layer's
inherited `pointer-events:none` made them unreachable by hover. Dependency
paths now accept pointer hits on their stroke. Actual hit testing reaches the
path and its full description; dragging from that path still pans the map and
preserves topic selection. Evidence `out/mm-map-edge-hit-before.txt` and
`out/mm-map-edge-hit-after.txt`. Hierarchy edges keep their existing behavior.

Dependency hover is installed build `158c0c15c7760114`, with all four served
assets matching main `055737a` (`out/mm-installed-hover-assets-result.json`).
A separate pinned installed-build focus endurance run is active until 19:40 UTC
under `out/mm-focus-ui-soak-20260919`; it covers the current focus recovery,
while the existing older-build large-map endurance remains independent.

Fit previously placed the rightmost cards behind an open desktop inspector.
It now fits within the space beside details, uses the whole map after closing
details, and retains the phone overlay camera behavior. A stale inspector from
a preceding project does not reserve space when selection is cleared. Regression
assertions failed before both fixes and pass afterward in the CI layout script.
Actual-browser checks passed eight assertions at 1280, 900 and 390 pixels wide;
the phone overlay test dispatches Fit programmatically because the open details
intentionally covers the map controls. Evidence:
`out/mm-map-fit-inspector-before.txt`, `out/mm-map-fit-inspector-final.txt`,
`out/mm-map-fit-test-before.txt`, `out/mm-map-fit-stale-test-before.txt`.
The 30 general browser assertions also passed with the Fit adjustment on the
10,000-node/20,000-link fixture (`out/mm-map-scale-fit-checks.txt`).

Fit is installed as build `d0f12d7313009daa` with all four served assets
matching main `27814be` (`out/mm-installed-fit-assets-result.json`). A real
Atlas→Platform project switch retains the whole-width camera: initial transform
equals manual Fit (`out/mm-map-fit-project-switch.txt`, two assertions).

The overview was mapping clicks across the entire padded button instead of its
canvas. It now maps the drawn canvas coordinates and clamps padding clicks to
the graph boundary. Added three phone/keyboard browser assertions, bringing the
general suite to 33. The corrected regression check fails against the preceding
HEAD renderer served through an isolated mock route, and passes current assets;
the mock route is removed in finally. Click tests compare the browser's actual
event coordinates because Chrome quantizes mouse coordinates. Earlier probes
that assumed fractional click precision are retained as unsuccessful evidence.
Authoritative evidence: `out/mm-map-overview-baseline-regression.txt`,
`out/mm-map-overview-checks-final.txt`. Six additional desktop checks on the
expanded 1,117,172-unit-high world pass at 1280 and 900 pixels, including padding
boundaries (`out/mm-map-overview-large-final.txt`). Layout/index assertions,
JavaScript syntax, own whitespace and debug compilation pass.

A concurrent shared-UI update in the original checkout replaces `.skip` with
`.skip-link`. Its mindmap script still bound `.skip`, causing a null listener
exception before load. Preserve all concurrent shell, CSS and picker edits;
the skip handler now accepts either class. An isolated browser reproduction
serves the current shared shell/assets with unchanged read APIs: before throws
and leaves count blank, after loads `2 nodes · 1 link` without page errors and
opens/focuses Outline via the renamed link. Mock routes are removed in finally.
Evidence `out/mm-map-shared-shell-before.txt`,
`out/mm-map-shared-shell-after.txt` (two assertions). Added the keyboard skip
assertion to the general suite; all 34 pass on the original shell too
(`out/mm-map-scale-shell-checks.txt`). The first mock attempt intercepted the
read endpoint too and remains unsuccessful evidence; final exact asset routes
leave native reads intact. Debug compilation, syntax and own whitespace pass.
The overview install attempt met an active concurrent upgrader and did not
change the installed binary. Wait for it; never remove locks or kill it.

The combined shared-shell/10,000-node UI check exposed narrow-phone overflow:
305 pixels available with a reserved scrollbar, but body minimum width 320 and
navigation reached 336.2. A mindmap-only <=380px rule removes that minimum and
uses compact horizontal main/navigation spacing. Restoring 12px main side
padding also keeps map navigation clear of the overview; the first partial fix
passed overflow but caught that overlap and is retained as unsuccessful evidence.
The combined 34 browser assertions now pass using the same shared asset snapshot
and unchanged native read API; returned asset hashes include the final stylesheet.
Evidence `out/mm-map-shared-scale-integration.txt` (before),
`out/mm-map-shared-scale-integration-after.txt` (partial),
`out/mm-map-shared-scale-integration-final.txt` (pass). The adjustment is scoped
to the mindmap stylesheet; concurrent shared-theme files remain intact.

Imported committed main's shared UI into the worktree. The CSS merge conflict
was resolved with that main snapshot, which already contained our spacing/hover
changes; the entire merged index was verified equal to MERGE_HEAD before commit.
All 34 general browser checks pass against the actual native shared-UI server,
without asset mocks (`out/mm-map-shared-native-scale-checks.txt`).

Project-picker Escape bubbled to the viewer's global handler, also closing topic
details and taking focus away from the restored picker trigger. The viewer now
respects handled keyboard events (`defaultPrevented`). The baseline renderer fails the
inspector-preservation assertion; current
code passes six checks: picker search focus, picker closes, topic remains open,
trigger focus restored, a second unhandled Escape closes details, card focus
restored. Evidence `out/mm-map-project-picker-escape-baseline.txt` and
`out/mm-map-project-picker-escape-after.txt`; reusable
`tools/mindmap_shell_browser_checks.js`. The first harness attempt retained
Outline across same-document navigation and is not counted as bug evidence.
The 19 focused-read checks also pass current shared UI
(`out/mm-map-shared-native-focus-retry.txt`). Its first reload timed out with
a pending local icon request during fixture replacement; that run remains
unsuccessful evidence, not a pass.

The new shared header moved phone navigation below the initial viewport. At
320×740 the controls began at y=749.08 and were 41px tall; at 390×844 and
700×900 they also extended below the screen. The mobile map now reserves space
for the shared header and keeps at least 300px for the map. The selector applies
only beneath that shell; desktop geometry is unchanged. General browser coverage
now includes initial control visibility at all three sizes and passes 37 checks.
Evidence `out/mm-map-shared-phone-height-before.txt`,
`out/mm-map-phone-height-widths-first.txt` (320-only partial fix), and
`out/mm-map-shared-phone-height-final.txt` (complete pass).

Expanded pan frame probe on the current native shared UI: 10,000 topics and
20,000 links, 1,117,172-unit-high expanded world, 239 sampled animation frames
after warmup. Median interval 10ms, p95 10.9ms, maximum 11ms; no intervals over
34ms or recorded long tasks. Ten cards and 13 SVG paths remained mounted.
These are local test-environment observations, not an all-device FPS guarantee.
Evidence `out/mm-map-expanded-frame-cadence.txt`.

Native continuous touch exposed a gesture bug missed by the earlier one-move
probe: a card drag moved 12px once, then all later movements stopped. Moving
capture from the card's span to the map emitted a bubbling lostpointercapture
from the span; the map incorrectly deleted its gesture point. Only the map's
own lost capture now ends its tracked gesture. Pointer up/cancel still clear
points from any target. Evidence `out/mm-map-touch-continuous-before.txt`.

New `tools/mindmap_touch_browser_checks.js` fails the second pan-step assertion
on the baseline and passes 15 checks on current native code: six 12×4px drag
steps, no unintended selection, six pinch steps with expected zoom, cancellation
cleanup, and a fresh selecting tap. It sends native Chrome touch input through
CDP, with gesture cancellation in finally. The first after-run attempted an
empty cancel before a gesture and hit a protocol error; corrected cleanup safely
ignores that no-gesture error. Final evidence `out/mm-map-touch-checks-before.txt`
and `out/mm-map-touch-checks-after.txt`. Layout/index, syntax, own whitespace and
debug compilation pass.
