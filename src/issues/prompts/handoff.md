Dependencies unblock at Ready, before merge.

- Read prerequisite PRs; use stacked PRs on unmerged branches, using the dependency branch as your branch start and PR base.
- Rebase and update PR bases when upstream changes or merges.
- Reopen Ready tasks before reworking them.
- Check CI yourself, attach the PR, and run `hey-boss issue ready {{number}} --project {{project_arg}}`. Leave handoff notes.
