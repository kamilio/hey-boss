# Private published pages

```sh
hey-boss pages publish ./report.html --name weekly-report
hey-boss pages publish ./dist --name dashboard --json
hey-boss pages list
hey-boss pages download dashboard --output ./dashboard
hey-boss pages delete dashboard
```

This is private hosting on your machines, not a public upload. Open **Published
Pages** in the top-right web menu. The same named URL works on each machine's
existing hey-boss web origin. Publishing from a worker does not need a running
supervisor; its files reach the other machines through normal fleet replication.

Reusing a name replaces the page. A directory needs `index.html`; a single HTML
file becomes `index.html`. Use relative URLs for site assets (for example Vite's
`base: './'`). Dotfiles are excluded, symlinks are rejected, and a site may contain
up to 10,000 files / 256 MiB. HTML runs in an opaque-origin iframe: scripts work,
but cannot access hey-boss's session, APIs, or browser storage. No build server,
server-side functions, or public DNS are involved.

Downloads restore the entire original file tree into a new directory and never
overwrite an existing destination. Deletion removes the named page and its old
versions from web access. Offline copies apply deletion on their next sync.
Immutable history and content remain retained for offline replay; deletion is
not secure erasure of replicated storage.

## Storage and synchronization

Files split into SHA-256-addressed 1 MiB chunks. A sorted manifest maps each path
to its length and chunk hashes; the manifest's hash identifies the deployment.
`published_chunks`, `published_deployments`, and `published_pages` join the existing
CRDT capture/replication tables. File bytes use the existing hashed-resource
transfer path, with bounded records. They are durably stored before their rows
are exposed. There is no second sync service or cloud dependency.

Unchanged chunks produce no new rows or transfer records. An identical republish
is a no-op. Every enrolled machine receives the complete content, including
through another peer. Concurrent named updates use the existing deterministic
logical-clock ordering and merge as a whole row, so a name cannot point to a
mixture of two deployments. Tombstones stop stale replicas resurrecting deleted
pages; deliberately publishing the name again creates a new live update.

A local, unreplicated `published_live` pointer retains the last complete version
while a new manifest or chunks are pending. Viewer URLs pin a deployment so
assets do not change halfway through loading. Content streams one chunk at a
time and supports ETag revalidation. Responses remain private and revalidate
before use so deletion is checked before returning a cached response.

The viewer gets a content-only capability from the same-origin API. Opaque-origin
iframe requests need it; they cannot use it to access hey-boss APIs. The server
still enforces its existing loopback, private LAN, or configured Tailscale origin.

The content-hash design follows [Cloudflare Wrangler's open-source Pages uploader](https://github.com/cloudflare/workers-sdk/blob/main/packages/wrangler/src/pages/upload.ts).
Replication reuses hey-boss's existing [cr-sqlite engine](../packages/hey-boss-sync/src/lib.rs), not a new distributed filesystem.
