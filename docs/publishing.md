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
The library shows compressed file sizes (including retained versions) and the
current original size. Shared files count toward each page that uses them.

Reusing a name replaces the page. A directory needs `index.html`; a single HTML
file becomes `index.html`. Use relative URLs for site assets (for example Vite's
`base: './'`). Dotfiles are excluded, symlinks are rejected, and a site may contain
up to 10,000 files / 256 MiB. HTML runs in an opaque-origin iframe: scripts work,
but cannot access hey-boss's session, APIs, or browser storage. No build server,
server-side functions, or public DNS are involved.

Downloads restore the entire original file tree into a new directory and never
overwrite an existing destination. **Delete everywhere** removes the named page,
its old versions, unshared files, and their journal payloads. Offline machines
clean up after reconnecting. Small deletion identities remain to reject stale
replays. Freed SQLite storage is reusable; the database file need not shrink.

## Storage and synchronization

Files split into SHA-256-addressed 1 MiB chunks. A sorted manifest maps each path
to its length and chunk hashes; the manifest's hash identifies the deployment.
`published_pages`, `published_publications`, and `published_page_chunks` join the
existing CRDT capture/replication tables. File bytes use the existing hashed-resource
transfer path, with bounded records. They are durably stored before their rows
are exposed. There is no second sync service or cloud dependency.

Page metadata travels before content, so new pages appear as syncing while
their files arrive. Protocol frames and new page resources are gzip-compressed;
chunk files are compressed on disk too. Legacy disk chunks convert when read,
without changing their hashes. Retained journal payloads also compress in bounded
background transactions; logical facts, delivery positions, and receipts stay
unchanged. Decoded sizes are bounded and content hashes verify original bytes.
Protocol version 5 requires upgrading every peer before synchronization resumes.

Updates within a page's lifetime reuse unchanged chunk facts. A new page or a
republish after deletion gets independent resource facts, so deleting another
page cannot strand its content on an offline peer. Disk chunks remain shared.
An identical republish is a no-op. Every enrolled machine receives the complete content, including
through another peer. Concurrent named updates use the existing deterministic
logical-clock ordering and merge as a whole row, so a name cannot point to a
mixture of two deployments. Tombstones stop stale replicas resurrecting deleted
pages, including edits made offline to their old lifetime. Deliberately publishing
the deleted name again creates a new lifetime.

Cleanup waits for local capture, indexing, and projection to finish. It preserves
unknown incoming owners and shared content, then prunes payloads without changing
delivery positions or receipts. Empty journal payloads are permanent tombstones;
replayed bytes cannot refill them. Existing publications acquire scoped resource
facts automatically, and their historical ownership is indexed in bounded scans.

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
