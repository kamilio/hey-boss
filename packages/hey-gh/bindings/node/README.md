# @hey-gh/node

Node.js bindings built with napi-rs for the Rust `hey-gh::ApiClient`. PR/CI reports, required checks, repository commits, watches, bootstrap, and cursor updates all use the shared daemon’s cache and rate-limit queue. The daemon uses your existing `gh auth login`; Node callers load its private local credential automatically.

## Build and use

Requires a current stable Rust toolchain and Node 20.17+, 22.13+, or a newer supported Node release. Build from the hey-gh checkout:

```sh
# From the project root, in one terminal:
cargo run -- serve

# In another terminal:
cd bindings/node
npm ci
npm run build
```

The package is local and has not been published to npm. From this directory, CommonJS uses `require('.')`; ESM uses `import { ApiClient } from './index.mjs'`.

```js
import { ApiClient, HeyGhError } from './index.mjs'

const api = new ApiClient() // http://127.0.0.1:8787/
await api.watch('OWNER/REPO', { pullNumber: 123, intervalSeconds: 30 })
await api.watchRepository('OWNER/REPO', { branches: ['release'], intervalSeconds: 30 })

const ci = await api.ciForPr('OWNER/REPO', 123, { refresh: true })
console.log(ci.data.summary.state, ci.complete)

let snapshot = await api.bootstrap()
let cursor = snapshot.cursor
try {
  const page = await api.changes({ cursor, limit: 100, waitSeconds: 30 })
  // Apply complete replacement snapshots from page.changes, then persist
  // page.next_cursor atomically with your updated state. Drain has_more pages.
  cursor = page.next_cursor
} catch (error) {
  if (error instanceof HeyGhError && error.code === 'cursor_expired') {
    snapshot = await api.bootstrap()
    // Replace local state with snapshot.snapshots before adopting its cursor.
    cursor = snapshot.cursor
  } else {
    throw error
  }
}
```

For another loopback port, pass its origin to `new ApiClient('http://127.0.0.1:9000/')`. The same client reloads credentials after a daemon restart. Remote servers and credential-bearing URLs are rejected.

## Account-wide PR activity

```js
const initial = await api.prStatus()
// Initial metadata/head rollups are batched; detailed sources hydrate in the daemon.
const page = await api.prStatus({ cursor: initial.cursor, waitSeconds: 30 })
for (const change of page.changes) {
  console.log(change.kind, change.pullRequest.url, change.activity)
}
// Apply full replacements, remove rows marked removed, and checkpoint page.cursor
// atomically. Drain hasMore pages, including empty pages with hasMore=true.
```

These PR pages and rows use gh-style camelCase fields (rich nested source data
retains raw names). Their cursor is scoped to the repository selection and is
different from `bootstrap`/`changes` source cursors. Repeat a read to replay the
same historical events. On `cursor_expired`, call `prStatus()` without a cursor
and replace local state with `pullRequests` before adopting its cursor. `kind`
includes closed, merged, and reopened, with terminal timestamps and `removed`.
`activity` identifies comments, CI, heads, conflicts, and review changes.
`complete=false` can mean initial hydration is pending; inspect `sourceErrors`
and `headCiState`/`ci` separately. Initial head context lists are bounded and
`statusCheckRollupComplete` indicates truncation. `read: { refresh: true }`
attempts a bounded detail cycle; `read: { cachedOnly: true }` makes no GitHub
requests and requires a prior successful discovery. `watchAccount(30)` changes
the durable account watch interval.

## Methods and options

Every method returns a Promise. Constructor URL errors throw synchronously. Invalid method arguments reject their Promise before making a request. PR numbers and numeric options must be finite integers within their documented bounds; unsafe JavaScript integers are rejected.

| Method | Purpose |
| --- | --- |
| `prStatus(options?)` | All your open PRs or changes since a scoped PR cursor |
| `watchAccount(intervalSeconds?)` | Durable account watch (default interval 60) |
| `prReport(repository, number, read?)` | Metadata, conflicts, comments, reviews, and CI |
| `ciForPr(repository, number, read?)` | Detailed CI without GraphQL |
| `requiredChecksForPr(repository, number, read?)` | Required-status-check policy and results |
| `repositoryReport(repository, options?)` | Branch reconciliation and discovered commits |
| `myPullRequests(repository, read?)` | Your open PRs |
| `listPullRequests(repository, state?, read?)` | All authors’ PRs; state `open`, `closed`, or `all` |
| `watch(repository, options?)` | Durable PR watch; omit `pullNumber` or use zero for discovery |
| `watchRepository(repository, options?)` | Durable repository/branch watch |
| `watches()` / `unwatch(id)` | Watch diagnostics and removal |
| `status()` | Shared queue, quota, and cache metrics |
| `bootstrap()` | Atomic source snapshots and starting cursor |
| `changes(options?)` | Replay or long-poll the durable feed |

Read options are `{ refresh?, cachedOnly?, maxAgeSeconds? }`. Refresh and cached-only cannot both be true. `maxAgeSeconds` is 0..86,400, default 30. Repository reads use `{ branches?, allBranches?, read? }`. Repository watches use `{ branches?, allBranches?, intervalSeconds? }`; PR watches use `{ pullNumber?, intervalSeconds? }`. Intervals are 10..86,400 seconds, default 60. Changes use `{ cursor?, limit?, waitSeconds? }`, with limit 1..1,000 (default 100) and wait 0..30 seconds (default zero).

Inputs use camelCase; except for account PR pages described above, returned payloads retain the Rust/HTTP API’s snake_case names. Payloads are ordinary JavaScript objects/arrays, with nulls and strings preserved. GitHub JSON numbers use JavaScript number semantics, as with `JSON.parse`; cursors are opaque strings. TypeScript declarations include concrete report, watch, and feed types. An incomplete PR/CI report resolves with `complete=false` and source errors; inspect these before interpreting success. Required-check policy can be unknown even while observed CI succeeds.

Normal `prStatus({ cursor })` calls read observed changes without starting another
upstream scan. The durable account watch owns polling; request `read.refresh`
explicitly for detailed upstream work. Individual authored-PR watches covered by
an equal-or-faster account watch expose `covered_by_account=true`. They check the
account's cached source availability, so their success timestamps are health
checks rather than independent revalidation times.

`HeyGhError.code` identifies errors such as `cursor_expired`, `invalid`, `local_auth`, `rate_limited`, `queue_full`, `deadline`, `cache_miss`, and `transport`. Rate-limit errors include `retryAfterSeconds`; HTTP errors include `httpStatus`. Polling and cursor guarantees match the [Rust SDK documentation](../../README.md).
GraphQL operation errors use `graphql`, or `graphql_access_denied` when GitHub
explicitly denies fields. They retain operation diagnostics without an invented
GitHub `httpStatus`, even if the local gateway response is HTTP 502.
For genuine GitHub HTTP failures, `httpStatus` retains the explicit upstream
status supplied by the daemon, rather than its local gateway status. Older
daemons without this metadata retain the existing fallback behavior.
Daemon authentication, storage, transport, and stopped-scheduler failures retain
their `auth`, `storage`, `transport`, and `stopped` codes without a GitHub HTTP
status. Known legacy diagnostic formats remain compatible.

## Package and validate

```sh
npm run typecheck
npm test
npm pack
```

`npm test` builds a synthetic Rust daemon fixture and uses a local GitHub mock; no real login or GitHub requests are needed. `npm pack` builds a release addon for the current platform and bundles the loader, JavaScript entry points, TypeScript declarations, and native binary. Install that archive in another project:

```sh
npm install /path/to/hey-gh/bindings/node/hey-gh-node-0.1.0.tgz
```

Then use `import { ApiClient } from '@hey-gh/node'` or `require('@hey-gh/node')`. An archive built on one platform contains only that platform’s binary; build it on the destination platform. CI defines native builds/tests for Linux x64, macOS, and Windows x64 on Node 20 and 22. Prebuilt npm releases and browser bindings are not supplied.

PR feed reads support transport selection:

```js
const fields = ['id', 'number', 'state', 'headCiState', 'conflicts', 'removed']
const baseline = await api.prStatus({ fields })
const changes = await api.prStatus({ fields, cursor: baseline.cursor, waitSeconds: 30 })
```

Selected rows always retain `complete` and `sourceErrors`. TypeScript reflects
omitted fields, which are unknown. Apply replacements only to selected local
state and use the same fields for bootstrap/deltas; omit fields for a full
mirror. Cursors, lifecycle kinds, activity, changed fields, and envelope health
remain intact. Activity can include comment bodies. Projected stored reads validate raw JSON spans and skip decoding omitted row
fields. Original stored-byte limits and cursor boundaries remain unchanged;
raw JSON is still read.
