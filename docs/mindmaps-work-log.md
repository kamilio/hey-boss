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
