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
