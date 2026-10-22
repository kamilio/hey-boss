# Documents and attachments

## Artifacts

Artifacts are persistent project Markdown documents in the issue store. Use them
for plans and other documents that need stable links and discussion.

```sh
hey-boss artifact create --title 'Release plan' --file plan.md --issue NUMBER
hey-boss artifact list --query 'Release'
hey-boss artifact view ID --json
hey-boss artifact edit ID --file updated.md
hey-boss artifact export ID
```

`--file -` reads stdin; `--body` accepts inline Markdown. `export` writes Markdown
to stdout.

`--issue NUMBER` on create/link attaches the same document;
`unlink ID --issue NUMBER` preserves it. `links` reads attachments
to a resource. `comment ID --body TEXT --quote SELECTED_TEXT` or
`--parent COMMENT_ID` adds discussion. `resolve ID COMMENT_ID` and `--reopen`
retain thread history. `archive/restore ID` retain stable
references.

## File attachments

```sh
hey-boss attachment upload PATH --issue NUMBER
hey-boss attachment list --issue NUMBER --json
hey-boss attachment download FILE_ID --output DIRECTORY
```

Uploads store regular files up to 10 MiB on the authoritative host. Use
`--artifact ID` instead of `--issue` for other resources.
`list` returns IDs, filenames, sizes, and SHA-256. Without `--output`, downloading
materializes a private temporary copy on the caller's machine; with it, choose a
filename or existing directory.
