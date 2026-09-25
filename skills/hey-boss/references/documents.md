# Documents and attachments

## Artifacts

Artifacts are persistent project Markdown documents in the issue store. Use them
for plans and other documents that need stable links and discussion.

```sh
hey-boss artifact create --title 'Release plan' --file plan.md --issue NUMBER
hey-boss artifact list --query 'Release'
hey-boss artifact view ID --json
hey-boss artifact edit ID --file updated.md --if-version VERSION
hey-boss artifact export ID
```

`--file -` reads stdin; `--body` accepts inline Markdown. `export` writes Markdown
to stdout. Use the current version from `view` when editing.

`--issue NUMBER` or `--node ALIAS_OR_ID` on create/link attaches the same document;
`unlink ID --issue NUMBER` or `--node NODE` preserves it. `links` reads attachments
to a resource. `comment ID --body TEXT --quote SELECTED_TEXT` or
`--parent COMMENT_ID` adds discussion. `resolve ID COMMENT_ID` and `--reopen`
retain thread history. `archive/restore ID --if-version VERSION` retain stable
references.

## Mindmaps

Use `hey-boss mm` for the project's nested outline and `mm web` to open it.

```sh
hey-boss mm add 'Release' --id release
hey-boss mm issue NUMBER --under release
hey-boss mm link issue:12 Platform::api --kind depends-on --why 'API must land first'
hey-boss mm view release
hey-boss mm show --bodies preview --json
```

`mm issue --title` supplies a map-only label; it does not rename the issue.
Use `--if-version VERSION --request-id ID` to guard and deduplicate map edits. PR URLs also work
as link selectors: `pr:https://github.com/org/repo/pull/2`.

## File attachments

```sh
hey-boss attachment upload PATH --issue NUMBER
hey-boss attachment list --issue NUMBER --json
hey-boss attachment download FILE_ID --output DIRECTORY
```

Uploads store regular files up to 10 MiB on the authoritative host. Use
`--node SELECTOR` or `--artifact ID` instead of `--issue` for other resources.
`list` returns IDs, filenames, sizes, and SHA-256. Without `--output`, downloading
materializes a private temporary copy on the caller's machine; with it, choose a
filename or existing directory.
