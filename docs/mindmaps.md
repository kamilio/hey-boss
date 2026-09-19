# Project mindmaps

`hey-boss mm` shows one nested outline per project, with live issues, automatically linked PRs, pending notifications and directed cross-project relationships. The web interface is a read-only viewer. Create and change maps from the CLI.

## Start an outline

```sh
hey-boss mm add 'Autumn release' --id release
hey-boss mm add 'Design decisions' --id design --under release --body '## Scope

Describe the plan.'
hey-boss mm issue 12 --id implementation --under release
hey-boss mm show
hey-boss mm web
```

Use `--body -` to read Markdown from stdin, or `--file design.md` to copy a file. Bodies are stored in SQLite; later file changes do not update text nodes. Issue bodies and titles resolve from the issue store each time the map is loaded. `mm edit design --title 'Design' --file design.md` replaces supplied text fields.

Terminal outlines show titles and relationships by default. `mm view design` reads
one node's full current text; `mm show --bodies preview` includes up to 512 Unicode
characters per body, and `--bodies full` includes complete bodies. JSON reads default
to full bodies for compatibility; use `--bodies preview` or `none` for smaller map
responses. `mm export` always includes complete bodies.

`mm view NODE --bodies preview|none|full` uses the same body modes for a focused
node read; full remains its default. The viewer fetches a current preview when
you choose Show less, retaining live issue titles, assignments and state.
Issue assignments appear in its metadata and `mm view`; search also matches
assignment names/IDs and resource states. Hidden projects are omitted from the
normal viewer picker, but direct references can open them with a hidden label.

The viewer opens a branching map. Drag or scroll to pan, pinch or Ctrl/⌘+scroll
to zoom, and use Fit for an overview. The small overview also jumps to a location.
Select a card to read its Markdown, native assignment/state and incoming/outgoing
relationships; visible dependencies show directional arrows. Branch buttons and
Expand/Collapse all control the hierarchy. Search reveals matching branches and
keeps the search field focused. Outline switches to the complete nested reading
view. Keyboard users can Tab to cards/controls, use arrow keys to pan the map,
`+`/`-` to zoom, `0`/Home to fit, and Escape to close topic details.

Large maps initially collapse their root branches. The map mounts cards near the
viewport and omits offscreen branch edges; complete relationship descriptions
remain in topic details. All viewer interactions remain read-only.

Map reads use a 32 MiB response budget, including rendered Markdown. A full map or
export that exceeds the budget returns an error before printing content. Use
previews or omitted bodies for the outline and `view NODE` for individual text.
When relationship descriptions make the whole map too large, `links NODE` reads
only that node and its relationship endpoints, with bodies omitted. Mutations
remain available and never remove content to meet a read budget.

The default project is the current Git repository, shared across worktrees. `--project Atlas` selects an unambiguous project name or a full project ID. `HEY_BOSS_ISSUE_PROJECT` is honored in worker sessions. `--host devbox` / `HEY_BOSS_ISSUE_HOST` use the authoritative SSH issue store, with no local fallback. Both machines need a CLI version with mindmap support.

Mindmaps are not included in fleet's offline issue replicas. From a fleet agent,
target its controller explicitly with `--host CONTROLLER` or
`HEY_BOSS_ISSUE_HOST`; local map reads and edits report this requirement. The web
viewer can use the same remote host. Standalone stores and controller stores
support local maps normally.

## References and links

`--id` assigns a readable alias, unique within a project. Commands also accept the generated `n-…` ID. `PROJECT::alias` resolves a node in another project's map; full project IDs disambiguate names. Aliases cannot contain colons or start with `n-`.

```sh
hey-boss mm issue 3 --issue-project Platform --id api --under release
hey-boss mm pr https://github.com/org/repo/pull/42 --title 'Implementation' --id implementation-pr
hey-boss mm notice TASK_ID --under release
hey-boss mm link design implementation --description 'Implements the design'
hey-boss mm link implementation Platform::api --kind depends-on --why 'API must land first'
hey-boss mm link pr:https://github.com/org/repo/pull/43 pr:https://github.com/org/repo/pull/42 --kind depends-on
hey-boss mm links implementation
hey-boss mm unlink implementation Platform::api --kind depends-on
```

Typed selectors `issue:12`, `pr:HTTP_URL` and `notice:TASK_ID` add missing reference nodes as roots when used in `mm link`. If either endpoint cannot resolve, the entire operation rolls back. Other commands require existing nodes. An issue reference is checked against the authoritative issue store. PR references use HTTP(S) URLs, with trailing slashes normalized. Notification IDs resolve through the local Mac Inbox bridge when viewing.

A link is directed: **A depends-on B** means A waits for B. `related` is the default kind; custom kinds are allowed. Descriptions are optional, and `--why` is an alias for `--description`. Repeating `link` updates the description for the same source, target and kind; an omitted or empty description clears it. `unlink` removes only that direction and kind. Cross-links can cycle; outline parenthood cannot.

Use `hey-boss issue pr add 12 URL` to attach a PR to its issue. Mindmaps project automatic `pull-request` links from those records; removing an attachment removes the automatic relationship on the next load. Explicit PR nodes reuse the automatic link endpoint. An explicit PR also reveals
its attached issues even when those issues have not been placed in any map. Those
unplaced issue endpoints are live references and open the issue directly; viewing
them does not add saved nodes. When an issue is placed in multiple maps, the PR
prefers its node in the current map, then in the issue's own project map. Trailing
slash variants of the same PR attachment do not duplicate automatic nodes or links.
Automatic links are not manually editable with `mm link/unlink`.

Mirrored issues show their source project and issue number in the viewer and
`mm view`. Deleted issues keep their saved map references with an unavailable
state; the viewer omits their issue-opening link.

PR labels default to their URLs, including URLs up to 2,048 bytes. Custom labels
and ordinary topic titles support up to 512 bytes. Supply `mm pr URL --title LABEL`, or use
`mm edit implementation-pr --title 'Navigation polish'` to change a saved PR label.
Its URL, issue attachments and dependency links remain intact. `mm view` prints the
URL, and Markdown export makes the label a link to the PR. PRs accept title edits;
issue and notification content stays live.

Only notices confirmed pending in the current Inbox appear. Completed, read, cancelled or absent notices are omitted, while any child topics are promoted to their nearest visible ancestor. If the Inbox cannot be read, unverified notification nodes are hidden and an availability warning is shown. Saved references remain in the map. Opening the map does not mark a notice read, answer a question, or finish a review.

## Organize and remove

```sh
hey-boss mm move design --under release
hey-boss mm move design --before implementation
hey-boss mm move design --after implementation
hey-boss mm move design                  # move to the root/end
hey-boss mm alias design architecture
hey-boss mm alias architecture --clear
hey-boss mm remove design
hey-boss mm remove release --recursive
hey-boss mm projects
hey-boss mm export > roadmap.md
```

An anchor supplies the destination parent unless `--under` is supplied; then the anchor must be its child. Nesting stays within one project, supports 32 levels and rejects cycles. A project supports 10,000 saved nodes. A referenced resource has one saved node per map; use aliases to place and link it. Removal with children requires `--recursive`. Deleting a map node removes its links and never deletes its issue, PR or notice.

`mm alias NODE NAME` changes a topic or resource node's alias; `--clear` removes it.
The generated node ID, nesting and links stay intact. The new alias must be unique
within the selected project. Identical request-ID retries still resolve after the
original alias has changed.

## Automation and concurrency

All commands support `--json`. `show` returns the live projected map, `view` returns
one full node, and `links NODE` returns a node and its relationship endpoints with
bodies omitted. Mutations return
compact saved node metadata, the changed link when applicable, and affected project
versions; use `show` to read the complete current map. Mutations do not read the Inbox,
so adding or linking notice references works while the desktop app is unavailable.
The same compact mutation result is cached for request ID retries, rather than a copy
of every node in the map. Mutations use the same durable transactions, author identity
and request ID machinery as issue commands. Use `--agent human:NAME` in a terminal when session detection is unavailable.

```sh
hey-boss mm add 'Release' --id release --request-id release-topic --if-version 0 --json
hey-boss mm edit release --title 'Release plan' --if-version 1 --json
```

Reuse a request ID only for an identical uncertain retry; a changed operation conflicts.
`--if-version` checks the selected project's map revision. Select an endpoint project
when using this guard for links. Adding/removing/updating a cross-link increments both
endpoint projects' map revisions. If both endpoints belong to other projects, the
invoking project's revision stays unchanged. Live issue changes and Inbox state do
not increment the outline revision; reload for their current state.

Exit codes follow issue commands: 2 invalid input/identity unavailable, 3 not found, 4 conflict, 1 operational failure. `--request-id` and `--if-version` apply only to mutations.

## Viewer

`mm web` serves `/mm` on loopback, default port 4781. `--port 0` chooses an available port; `--json` prints the URL. Existing `issue web` servers also serve `/mm`. Mindmaps appears in the main web navigation alongside Inbox, Issues and Workers, and in the Mac menu bar alongside Inbox and Issues.

The viewer supports project switching, nested collapse/expand, search through topics
and link descriptions, incoming/outgoing dependency labels, Markdown bodies and
cross-project navigation. Issue bodies sit behind an expandable “Issue details”
control so the outline remains readable. Bodies initially show previews; “Read full
text” loads one complete node and “Show less” restores its preview. Search examines
titles, previews, loaded full text and link descriptions; it does not fetch every
body. On the first visit to a map with more than 200 nodes, root sections start
collapsed. Search reveals matching descendants and their ancestors, and following
a relationship reveals its target. Refresh reloads live resources. Maps are not
writable through either `/api/mm` or the general `/api/action` web route. The ordinary
issue and Inbox interfaces keep their existing editing behavior.

See [research and design rationale](mindmaps-research.md).
