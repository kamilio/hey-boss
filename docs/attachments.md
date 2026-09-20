# File attachments

Issues, mindmap nodes and artifacts share disk-backed attachments. Open the resource
in the web UI, drop files on **Drop files here**, or use **Choose files**. Every row
shows the original filename and size. Click the filename or download action to
retrieve it. Removal asks for confirmation and deletes the stored file. Files are
served as downloads; arbitrary HTML and other active content never execute in the
interface. Any file type is accepted, up to 10 MiB per file and 1,000 files per
resource.

```sh
hey-boss attachment upload ./design.png --issue 57
hey-boss attachment upload ./trace.zip --node release
hey-boss attachment upload ./reference.pdf --artifact ARTIFACT_ID
hey-boss attachment list --issue 57 --json
hey-boss attachment download FILE_ID
hey-boss attachment download FILE_ID --output ./reference.pdf
hey-boss attachment remove FILE_ID
```

Select exactly one of `--issue`, `--node`, or `--artifact` for upload/list. Node
selectors accept a node ID, alias or `issue:NUMBER` in the selected project.
`--project` selects another project. `--host SUPERVISOR` sends reads and writes
through the existing SSH issue RPC. Downloads always materialize on the caller's
machine, even when the attachment is stored remotely. Omit `--output` to create a
private temporary folder, or pass an existing directory to retain the original
filename. Existing files are never overwritten. The CLI prints the local path;
`--json` includes path and metadata without base64 contents. Temporary downloads
belong to the caller and should be removed when no longer needed.

Attachment commands support `--agent`, `--request-id` and the usual issue project
and host environment variables. Reuse the same request ID and identical contents
when retrying an upload or removal whose response was lost. Browser retries retain
the request ID. A pending upload stays in memory on its current page; keep that
page open to retry it after reconnecting.

The authoritative issue database stores only metadata and upload digests. Contents
live in the sibling directory named by replacing the database extension with
`attachments` (normally `issues.attachments/`). Files use opaque IDs, never supplied
filenames, and private permissions. Uploads are synced before metadata commits;
failed commits clean up newly written files. Downloads verify the recorded size
and SHA-256. Back up the database and this directory together. Attachment mutations
are not replicated as issue rows; fleet companions must use `--host SUPERVISOR`.
Moving an issue to another project moves its attachment access atomically.
Removed resources do not delete attachments implicitly: files remain accessible by
ID until explicitly removed. Paired Fly devices use the authenticated supervisor
bridge; the existing persistent transport journal carries pending uploads and
download results while the authoritative disk remains on the supervisor.

Verification: `cargo test --locked --test attachments --test issues_web --test artifacts`,
`npm --prefix mobile test`, and `tools/attachments_browser_checks.js` against an
isolated Attachment Studio fixture cover storage, remote materialization, authority,
project/device isolation, upload retries, downloads, deletion and responsive themes.
