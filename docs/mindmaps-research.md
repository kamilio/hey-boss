# Mindmapping research and CLI design

Researched 2026-09-19 before implementation. Primary sources were fetched live.

| Tool | Hierarchy and references | API / CLI pattern | Adopted behavior |
| --- | --- | --- | --- |
| Freeplane | Nodes in a tree; graphical connectors independent of parenthood. Connectors have source, middle and target labels. | [Java scripting API](https://docs.freeplane.org/api/org/freeplane/api/Connector.html): `setMiddleLabel(String)`, `setSourceLabel(String)`, `setTargetLabel(String)`; `node.connectorsIn / connectorsOut`. | Keep parenthood separate from directed links. Allow optional explanatory text on each link. |
| XMind | Topic hierarchy; relationships connect topic IDs. | [Official Python SDK](https://github.com/xmindltd/xmind-sdk-python): load/save workbooks; [RelationshipElement](https://github.com/xmindltd/xmind-sdk-python/blob/master/xmind/core/relationship.py) has `setEnd1ID`, `setEnd2ID`, `setTitle`, `getTitle`. This is the legacy file SDK, not evidence of a current hosted REST API. | Stable endpoint identities and optional descriptions, independent of display title. |
| Markmap | Markdown headings and lists become a navigable hierarchy. | [CLI docs](https://markmap.js.org/docs/packages--markmap-cli): `markmap <input>`, `--no-open`, `--output`, `--offline`, `--watch`. The browser renders authored data. | Terminal authorship and read-only web view. Simple nested lists, Markdown bodies and an outline export. |

Descriptions are a conventional feature of relationship links; they are not mandatory. `hey-boss mm link A B --description 'B must land before A'` records why A depends on B. `--kind depends-on` gives that direction a stable machine-readable meaning. Plain links default to `related` and may omit the description.

## Scope and ergonomics

One map belongs to each existing issue project. The current Git repository supplies the default project, shared across worktrees. `--project` and `--host` follow existing issue command conventions. The issue database remains authoritative; no separate graph server or duplicated issue lifecycle.

Nodes have stable generated IDs and optional project-local aliases (`--id rollout`). Aliases make commands readable. `PROJECT::ALIAS` selects nodes across projects; full project IDs disambiguate names. `issue:123`, `pr:https://github.com/org/repo/pull/4`, and `notice:TASK_ID` are shorthand references. Linking those references adds a node if necessary, so a dependency takes one command. An alias can never silently resolve to multiple nodes.

Nesting controls the outline. Directed cross-links control relationships and can connect any node kinds, including PR-to-PR dependencies. Cross-links may cycle; nesting may not. Deleting a node requires explicit recursive intent when it has children, removes its graph links, and never deletes the underlying issue, PR or notification.

Issue titles/state and attached PRs are read live. Automatic issue→PR relationships are projected from `issue pr add` records, with no copied edge to become stale. Notifications are projected from the Inbox, only while pending. Unavailable Inbox data is reported as unavailable; unverified notices are hidden without deleting saved references or claiming that tasks were resolved.

The web map is a viewer: project switching, collapse/expand, search, refresh and following cross-links. Map mutation requests are rejected by the web API; CLI remains the authoring surface. The existing issue and Inbox applications retain their own functionality.

## Validation plan

Exercise the actual CLI against an isolated SQLite database: project defaults, alias selection, Markdown stdin, issue/PR live projection, cross-project linking, optional descriptions, dependency direction, idempotent retries, optimistic conflicts, cycle rejection, moves/reordering and deletion. Exercise HTTP read-only boundaries and pending-only projection with a synthetic Inbox socket. Inspect the rendered nested-list viewer in a real browser, including narrow layout, keyboard navigation and stale-request handling.

## Reproduction evidence

Executed the actual Markmap CLI using `npx --yes --package markmap-cli markmap --help`.
Its live help confirmed the documented input/output, offline, no-open and watch
controls, plus a `--port` option. Executed an offline HTML conversion of this document:

```sh
npx --yes --package markmap-cli markmap --no-open --offline -o out/mindmap-markmap-reproduction.html docs/mindmaps-research.md
```

The command exited successfully and produced a standalone HTML artifact. Freeplane
and XMind findings above are from their published API/source; their desktop applications
have not been installed or executed as part of this research.

Attempted the official legacy XMind Python SDK at commit
`58b2c7f1971abd941cd0f28e88388ec93ed2c53d` in an isolated checkout. Its
`createRelationship(end1, end2, title=None)` / `addRelationship(rel)` source confirms
that description/title is optional and relationship creation is separate from
hierarchy. Running `xmind.load` under the available Python 3 runtime failed in
`core/__init__.py` with `AttributeError: 'str' object has no attribute 'decode'`.
The SDK assumes Python 2 strings. No XMind artifact is claimed from this attempt;
no old runtime or third-party SDK patches were installed. Markmap's real CLI
reproduction above did produce its verified artifact.
