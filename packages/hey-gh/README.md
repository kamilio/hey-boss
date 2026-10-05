# hey-gh

Maintained as `packages/hey-gh` in the hey-boss workspace. Imported from sibling
hey-gh revision `e47688b37f90be76afb0eafa2d45e78077e90f0b`, including the existing
local report-read reliability changes. The `hey-gh` executable and CLI remain
unchanged. Run the commands below from this package directory.

A Rust GitHub SDK, CLI, and local HTTP API with persistent caching, a shared request queue, and a durable incremental feed. Authentication uses your existing `gh` login.

## Optional GitHub App installation

CI checks, commit statuses, workflow runs, workflow jobs, CI-only REST PR metadata,
and the fixed CI-selector GraphQL query can use a selected repository installation's
separate primary quota.
Register/install a GitHub App with read access to Actions, Checks, Commit statuses,
Contents, and Pull requests (Metadata is automatic).
Installation tokens request only these permissions, even if the App has broader
grants. Keep the existing `gh` login: all other activity uses the user identity,
including generic PR metadata, discovery, generic GraphQL, comments, reviews, repository metadata,
and required-check policy. Uncovered repositories also keep user authentication.
Installation errors remain explicit;
they do not silently retry with user credentials. Secondary backoff and socket
limits remain shared. All machines using one installation share its allowance.

Configure each machine through secure secret entry, with its RSA private key in
`HEY_GH_APP_PRIVATE_KEY`, then restart the daemon:

```sh
hey-boss notif secret --field HEY_GH_APP_PRIVATE_KEY -- hey-gh app --client-id APP_CLIENT_ID --installation-id INSTALLATION_ID --repository OWNER/REPO
hey-gh service restart
```

Repeat `--repository` for selected repositories in that installation. The key
is stored in a mode-0600 file under the platform's local data directory,
`hey-gh/apps/github.com.json`; it is never printed or put in the response cache.
The scheduler renews tokens before expiry. Cached-only reads never mint tokens.
App identity and repository selection partition the cache; enabling/changing an
installation requires a fresh bootstrap and watch registration. Hourly renewal
and private-key rotation preserve that scope. `status.rate_limits` distinguishes
`installation/core` and `installation/graphql` from the user's allowances. GitHub registration may
require interactive passkey confirmation and organization permission.

## Start

Requires Rust, Cargo, and an authenticated GitHub CLI (`gh auth login`).

```sh
cargo install --path . --locked
hey-gh install
hey-gh service start
```

`hey-gh install` installs or updates a compact global command-card skill in
`~/.codex/skills/hey-gh` (or `$CODEX_HOME/skills/hey-gh`),
`~/.agents/skills/hey-gh`, and `~/.claude/skills/hey-gh`. It runs without the daemon or GitHub authentication.
Reload skills or start a new agent session to discover it. For a custom skill
root, use `hey-gh install --skills-dir /path/to/skills`; repeat the option for
multiple roots. The installer only replaces its own `SKILL.md`.

The fleet PR monitor starts this same user service when the local API is absent,
so supervisor upgrades cannot leave an old private daemon holding the port.
An existing listener is reused. Automatic startup requires launchd or a systemd
user manager; startup failures are logged and retried on the next monitor cycle.
For environments without a user service manager, run `hey-gh serve` explicitly.

In another terminal:

```sh
hey-gh ci poe-internal/poe2 15064
hey-gh pr poe-internal/poe2 15064
hey-gh mine poe-internal/poe2
hey-gh prs poe-internal/poe2 --state all
hey-gh required-checks poe-internal/poe2 15064

# Monitor the default branch, selected refs, and refs involved in watched PRs.
hey-gh watch-repo poe-internal/poe2 --branch main
hey-gh repo poe-internal/poe2

# Discover and monitor all your open PRs in this repository.
hey-gh watch poe-internal/poe2

# Or monitor a specific PR.
hey-gh watch poe-internal/poe2 15064 --interval 30
hey-gh watches
hey-gh status
```

PR numbers above are examples. `watch` registrations and discovery state survive restarts. A repository watch discovers new PRs and refreshes a previously tracked PR once after it closes or merges; unsuccessful refreshes remain tracked for retry. CI polling runs independently of the full comment/review scan. Background account hydration allows five seconds without source progress, excluding deliberate scheduler queue waits; each cycle remains bounded and unfinished work stays queued. `/v1/watches` reports each loop's last successful refresh and errors. Covered individual watches report stale or pending evidence when their lane's conservative validation clock is unknown or older than two watch intervals (at least 60 seconds), even if cached data is complete.

Each background account lane hydrates at most two PRs at once, so a cached PR
can finish while another waits for metadata or quota. Each PR keeps its own
stall budget; admission advances the durable rotation before work starts.
Small embedded queues remain sequential. Request concurrency, queue capacity,
quota pacing, freshness, and foreground reservations still use the shared scheduler.

`pr` includes PR metadata, merge conflicts, conversation comments, inline comments, reviews, current review requests and decisions, review thread resolution and replies, paginated review-request/removal history, timeline events, and CI. `prs --state all` lists every author’s open, closed, and merged PRs without GitHub Search’s 1,000-result ceiling. `ci` fetches CI and merge metadata without comment requests. `--refresh` revalidates all its sources; `--cached-only` makes no GitHub requests. Default reads allow cached responses up to 30 seconds old. Incomplete reports include source errors and make the CLI exit unsuccessfully after printing JSON.

CI reads can reuse recent account-discovery selectors when the PR identity, head,
base, and validated test-merge commit match cached REST metadata. Freshness uses
that PR's discovery page time; unrelated REST metadata keeps its original clock.
These validations appear as `my-open-prs://HOST/OWNER/REPO/NUMBER#ci-selectors`.
Completed scans retain each PR's page cursor as a cache lookup hint. CI can use
a newer validated page before the next scan finishes, without making a GraphQL
request or replacing the authoritative account roster. The hinted page must
still contain exactly one matching PR; moved or ambiguous entries fall back to
REST. The original page validation time remains the freshness boundary.
Missing, stale, or inconsistent selectors fall back to REST. Explicit refresh and
cached-only behavior are unchanged, and CI still confirms selectors after
collecting checks.

When discovery cannot validate stale REST metadata, a fixed GraphQL query can
confirm matching CI selectors. Configured installations own that query and its
cache/in-flight identity; identical queries through the generic interface still
use personal authentication. Installation failures remain explicit under the
normal caller deadline. Personal selector queries retain a two-second optional
shortcut before REST fallback. Cache-only status evidence follows the same
provider route, without minting or requesting additional data. Changed selectors
still require full REST metadata. CI uses the configured installation for cold,
changed, ambiguous, and explicitly refreshed metadata; generic metadata reads keep
the personal account. These routes have separate cache and in-flight identities.
CI can reuse the newest full cached payload from either provider within the caller's
freshness policy, including closed PRs. Cached reads never turn an installation
failure into a personal network request. CI lifecycle publication retains the full
REST payload and its validation time.
Newer contradictory CI metadata invalidates a personal collection seed or cache
read. A generic reader then revalidates through the personal account; cached-only
reads report a cache miss rather than restoring an older lifecycle or selector.

Fresh CI metadata can also prove an empty check-run list from a null commit
rollup or an explicit zero check-run count. It reuses existing selector/discovery
reads; no query is added solely for this shortcut. Missing or mismatched evidence
retains REST, newer REST evidence wins, and contradictory presence at the same
clock cannot certify an empty list. Explicit refresh still reads every source;
offline evidence keeps its original validation time.

The daemon binds to `127.0.0.1:8787` by default. Use `serve --listen 127.0.0.1:PORT` and `--server http://127.0.0.1:PORT` on client commands for another port. Only loopback addresses are supported. Browser requests and nonlocal Host headers are rejected. The daemon creates a private local API credential automatically; the CLI and `ApiClient` load it without another login or token entry.

For lifecycle-only consumers, Rust `ApiClient::pull_request` and
`GET /v1/prs/OWNER/REPO/NUMBER/metadata` return the cached REST metadata envelope
with its original validation timestamp. They share the daemon queue and cooldowns,
honor `cached_only`/`max_age_seconds`, and never hydrate CI/comments/reviews or
register watches. Metadata refresh work has a 15-second background deadline.

Watcher integrations can use `ApiClient::background()` or `background=true` on
targeted PR, CI, and required-check HTTP reads. These share the background queue
and stop after 20 seconds; new upstream jobs inherit that deadline. Interactive
reads can still promote shared work. Metadata keeps its 15-second deadline.

If the official CLI reports a local API credential failure, use the same OS user
and cache directory as the daemon. Restart the existing `hey-gh serve` process
with its original `--listen` and `--cache` options to recreate a lost credential.
Keep the cache database: it holds watches and resumable cursors. This local
credential is separate from the GitHub login. Harvester preserves the private
`hey-gh/instances` directory during cache expiration, including resumed sweeps.

## Recent diagnostics

The daemon automatically keeps private, rotated logs: `hey-gh.log` plus four
archives, each at most 2 MiB (10 MiB total). Logs survive daemon restarts and
include startup/shutdown, refresh progress, deferred work, source error codes,
transport failures, retries, and rate-limit cooldowns. Tokens, upstream response
bodies, PR content, and comments are not persisted in diagnostics.

```sh
hey-gh logs --tail 200             # Recent events across retained archives
hey-gh logs --summary --since 900  # Safe JSON aggregates for the last 15 minutes
hey-gh logs --path                 # Print the directory; no daemon needed
hey-gh watches                    # Current polling/source errors
hey-gh status                     # Queue and observed rate limits
```

The default location is the platform cache directory under
`hey-gh/logs/8787` (the suffix is the daemon port). Use `--server` with `logs`
for another port. `serve --log-dir PATH` overrides the location; read those logs
with `logs --log-dir PATH`. On Unix, the directory is private (0700) and files
are private (0600). Only one daemon may write to a log directory. Shutdown handles Ctrl-C and Unix SIGTERM, with a five-second grace limit for active HTTP reads. `RUST_LOG`
controls verbosity; persisted output is restricted to hey-gh diagnostics.

`logs --summary` works offline and defaults to a 900-second window; `--since`
accepts 1..86400 seconds. It deduplicates completed requests by request ID across
rotations and separates failed retries from successful ones. Endpoint, HTTP
status, source, and failure-code counts retain GraphQL HTTP 200 operation errors.
Unknown labels are replaced with `unknown`; arbitrary log text is never emitted.
`request_elapsed_ms_by_endpoint` reports timing samples, unavailable timings, and
nearest-rank p50/p95/max milliseconds for completed jobs, including failures.
These timings include queuing and retries; they are not individual HTTP latency.
Missing or invalid timings stay unavailable rather than becoming zero.
The output reports retained time bounds, archive gaps, oversized files, and
partial records. `retention_covers_start=false` means the requested window is
not fully covered. Reads are non-atomic and can race with rotation. Attempt counts
belong to jobs completing in the window, including attempts before it; they are
not window-wide traffic or quota charges. A retry without a retained final is
unresolved evidence, not proof of an active job. Summary counts do not establish
PR freshness or readiness; use `status`, `watches`, and report validation clocks.
`source_refresh_failure_records` groups retained failure warnings by discovery,
CI/detail mode, source collection, and static error code. For example, access
errors under `details.review_threads.graphql_access_denied` explain missing
thread evidence separately from `discovery.open_pull_requests`. Exact copies in
rotated files deduplicate. These are distinct log records, not GitHub request
counts, attempts, unique PRs, current health, or proof that an unlisted source
succeeded. Repository names, PR numbers, comment bodies, and unknown labels are
excluded from these aggregates.

`account_refresh_cycles` separates CI/detail read failures from local refresh budget
interruptions and includes each lane's latest completed cycle with its clocks.
`deferred_across_cycles` sums repeated deferrals, not unique PRs. Seed-only
collection work is excluded; malformed/legacy cycle records are counted in
`uncorrelated_refresh_cycle_lines`. Completed cycles can start before the window,
and cycle progress does not measure GitHub traffic or evidence freshness.

## All your PRs and changes since a cursor

```sh
hey-gh pr                         # All your authored open PRs across repositories
hey-gh pr list                    # Same account list
hey-gh pr status                  # Same status view
hey-gh pr list -R OWNER/REPO      # Restrict output to one repository
hey-gh pr view 123 -R OWNER/REPO  # Full PR report, with gh-style JSON field names
hey-gh pr checks 123 -R OWNER/REPO # Detailed CI without comments
hey-gh pr view https://github.com/OWNER/REPO/pull/123
hey-gh pr view BRANCH -R OWNER/REPO
hey-gh pr view                    # Infer repository and current branch from git

hey-gh pr --json number,title,url,headCiState,conflicts
hey-gh pr view 123 -R OWNER/REPO --json number,title,ci,comments
hey-gh pr --refresh               # Wait for bounded detailed-source hydration
hey-gh pr --cached-only           # Last-known data; zero GitHub requests
```

Bare `hey-gh` also selects the account status view. The first response contains
`pullRequests` and `cursor`. Save that cursor, then request only later PR events:

```sh
hey-gh pr --cursor 'CURSOR_FROM_FIRST_RESPONSE'
hey-gh --cursor 'CURSOR_FROM_FIRST_RESPONSE'          # Shorthand
hey-gh pr changes --cursor 'CURSOR' --wait 30        # Long poll
hey-gh pr changes --cursor 'CURSOR' --limit 100      # Bounded event page
```

Cursor reads return an empty `pullRequests` array and a `changes` array. Each
change has `kind`, `activity`, `changedFields`, `observedAtMs`, its own `cursor`,
and a complete `pullRequest` replacement. The response's `cursor` is the
checkpoint after the entire returned page. Apply the page and persist that
cursor atomically; keep paging while `hasMore` is true. An empty `changes` page
with `hasMore=true` is valid because unrelated source events share the underlying
feed. Updates are coalesced: only the latest value of each resource is retained.
A repeated request can therefore return newer current data after another poll;
intermediate versions are not an audit history. Incremental replacements keep
their latest observation cursor and timestamp. Bootstrap continuation cursors
mark scan positions; their rows retain their original observation timestamps.

`kind` is `baseline`, `opened`, `updated`, `closed`, `merged`, `reopened`, or
`removed`. Closures and merges include terminal state and available
`closedAt`/`mergedAt` timestamps; `removed=true` tells an open-PR consumer to
remove the row. If a disappearance cannot yet be resolved, it is `removed` with
an unknown state and explicit source errors when retrieval fails. The durable
retry queue keeps probing until the terminal state is known. Reopened PRs are
explicit events. Activity identifies new/edited/deleted comments and inline
comments, reviews, thread resolution, CI changes, pushed heads, and conflict
changes. The feed records observed changes; polling cannot recover a close and
reopen, or a comment added and deleted, entirely between observations.

Reopening clears prior close/merge dates, including when REST hydration is
unavailable. A confirmed merge remains terminal for the same GitHub PR node ID,
even if a later listing briefly reports it as open. Status exposes `id` when
available; legacy rows acquire it on refresh. Lists and individual views support
`--json id,number,state,complete,sourceErrors` for a compact diagnostic projection. A different node reusing a repo
and PR number is opened as a new PR and cannot reuse the old node's CI evidence.
Cached-only preparation keeps terminal follow-up probes entirely in the cache.

Account discovery paginates the viewer's authored PR connection rather than
using Search. Each scan records the starting count and last PR in creation
order, then reads through that boundary. PRs appended during the scan are picked
up on the next pass. Missing boundaries, duplicate PRs, and inconsistent counts
fail explicitly and retain the last complete collection. Default reads first
collect batched metadata and head-check rollups, then register a durable account
watch automatically. Its independent
CI and detail loops hydrate the richer REST CI, comments, and reviews in the
background. `headCiState` is GitHub's observed head-commit rollup, **not** a
merge-eligibility verdict; `statusCheckRollupComplete=false` indicates that the
initial bounded context list is incomplete. The full `ci` report includes head
and test-merge results and remains null until available for the selected head.
These sources are polled independently: a fresh `headCiState` can disagree with
older detailed `ci` evidence for the same commit, including after a rerun.
`complete=true` means the stored evidence is available without source errors;
it does not mean every source was recently validated. For a decision about a
specific PR, read `pr checks` and inspect its per-resource validation times.
`complete=false` is expected during initial hydration. A fast read can exit
successfully while hydration is pending; actual source errors still produce an
unsuccessful exit after printing available JSON. Missing sources never imply
successful CI. `requiredChecks` remains a separate policy conclusion: it can be
null or have `state="unknown"` and policy errors even when the row is complete.
Inspect its state and errors before using required-check policy in a readiness
decision; `complete=true` alone does not establish merge readiness.
Account rows attach policy only when its head, base branch, PR-associated base
commit, and test-merge selector match the selected PR metadata, and its evaluated
base tip matches the latest cached branch evidence. Policy reports expose
nullable `pr_base_sha` (PR-associated commit), `base_sha` (resolved branch tip used
for evaluation), and `merge_sha`. A base change with an unchanged head
withholds the old policy until a matching report is collected; legacy snapshots
without these selectors are also withheld. These changes appear in the cursor
feed as `required_checks_changed` activity.

The account watch survives daemon restarts. A bounded cycle persists its next
PR so slow PRs cannot repeatedly starve later ones. Unvisited PRs remain queued;
exhausting a cycle does not add timeout errors to their rows. Normal cursor reads
consume the local feed without starting discovery or detail refreshes; use
`--refresh` to explicitly request upstream work. Discovery failures preserve
known PRs and allow REST CI polling to continue. Default discovery allows cached
responses up to 30 seconds old; background loops run every 60 seconds by default
and their timestamps/errors appear in `hey-gh watches`. Detail sources can be
older when hydration is pending or unsuccessful. A cursor records observation
order, not a guarantee that every source is currently fresh. `--refresh` attempts
a bounded full detail cycle and reports any remaining failures explicitly.

Account cycle logs distinguish failed observations from reads interrupted by the
local refresh budget (`interrupted`) and untouched queued work (`deferred`). A
local interruption keeps prior evidence with `complete=false` and an explicit
PR or cycle budget source error. Per-PR interruptions can occur without exhausting
the whole cycle. They are not GitHub
transport or rate-limit failure. Fair retry position remains durable; genuine
upstream errors keep their original diagnostics.

Discovery uses pages of 25, reads the head CI state without enumerating check
contexts, and shares a durable, fully paginated collection
between polling loops. Its reuse window starts when the scan completes; rows
retain the oldest page's actual validation time. Failed or inconsistent scans
never replace the last successful collection. Explicit refresh bypasses this
collection cache. PR bootstraps use a compact SQLite index of open, nonremoved
status snapshots and their original byte counts, narrowing repository filters
before loading bodies. Closed and removed PR bodies are not scanned on each
bootstrap. The index is built once for existing databases and maintained with
snapshot writes. Malformed PR JSON remains eligible for an explicit corruption
error. Closed PRs and unrelated repositories do not consume the open list's byte budget. Selected rows
still count their original stored bytes before projection. The selected state and
global observation cursor remain transactional; closure and reopening events
remain available through cursor reads.
Background discovery has its own full scan budget and waits its polling interval
after each completed or failed scan. Slow failures cannot trigger immediate
catch-up scans. CI and detail loops consume
the last successful collection without starting or waiting for discovery;
`watches` exposes separate `discovery_last_*` health fields. Successful source
hydration does not clear an independent discovery failure.
Incremental and cached feed envelopes expose a known discovery failure in
`errors` with `complete=false`, including on empty pages, without changing the
observation cursor or making additional GitHub requests.

PR feed envelopes separate selected PR evidence from account-wide discovery:
`coverage.repository` identifies the `-R` filter (null for the whole account),
`coverage.returnedRows` counts the rows on this page, and
`coverage.returnedRowsComplete` reports whether all those rows have complete
stored evidence. This covers bootstrap rows or incremental replacements only,
including fields omitted by `--json`. An empty page has zero returned rows;
it does not prove the repository has no PRs, or that a saved mirror is complete.
`accountDiscovery` reports `complete`, `errors`, `lastPollAtMs`, and
`lastSuccessAtMs` separately. Its `complete` is null when no discovery result
is known, false after a known failure, and true after a successful scan.
These are last-known results, not freshness or merge-readiness guarantees.

For example, an accessible selected repository can report
`coverage.returnedRowsComplete=true` and `accountDiscovery.complete=false`
when another organization's IP allow list prevents account discovery.
The returned PR facts remain usable, with their own `sourceErrors` and validation
times; discovery cannot establish whether other selected PRs are missing.
No error-message parsing or network-restriction bypass is used to infer access.
Actual missing/failed target sources still make returned-row coverage incomplete.

For compatibility, aggregate `complete`, `errors`, and CLI exits are unchanged:
a known account-discovery failure still produces `complete=false` and exit 1,
even with complete selected rows. Parse that JSON and retain its cursor and data;
inspect the separate health fields to determine which evidence is incomplete.
Cached-only and cursor health reads make no extra GitHub requests. A successful
bounded `pr view N -R OWNER/REPO --refresh --timeout 30` can validate that PR
without repairing account discovery or proving a complete repository roster.
Recovery requires a successful account scan and does not itself advance cursors.
Older daemons omit these new fields; upgraded SDKs represent absent coverage
and discovery completeness as null, never as successful evidence.

Discovery health is durable and credential-scoped, including deadlines. Rust
feed pages and HTTP envelopes expose the same known failure after restart or
watch removal. Reusing a last-good collection does not clear that failure; only
a newly validated, successful complete scan does. Recovery health and the
completed collection commit atomically. Attempt generations prevent late SDK
clients from replacing newer collections or undoing newer failures/recoveries.
These diagnostic changes never generate activity or advance observation cursors.
Successful discovery immediately publishes pending rows for new PRs; their
visibility does not depend on reaching them in the detail hydration queue.
Detailed checks come from the independent CI collector. Before that collector
finishes, `statusCheckRollupComplete=false` keeps absent checks inconclusive.
Discovery retains each page's validation time internally, so a later page's
new head cannot be overwritten by REST metadata validated between pages.
Validation clocks are absent from semantic replacements and do not churn cursors.
Matching commit refs allows CI reuse, while newer discovery metadata retains its
title, draft state and conflict result over older REST metadata for those refs.
A later REST validation can supersede discovery again. Valid GitHub `updatedAt`
versions additionally prevent older PR lifecycle state, terminal dates, titles,
draft state, body, labels and assignees from replacing newer observed values,
even if the stale response arrives later. Timestamps are compared as RFC3339 instants, including offsets
and fractional seconds. Equal or unavailable versions fall back to validation
order. PR timestamps do not gate CI, conflict checks, or GraphQL review updates;
those sources can change independently. A disappearance with older terminal
evidence remains removed with unknown state, `complete=false`, and an explicit
state error until retry resolves it. An older listing cannot reopen that unresolved
row. A confirmed merge stays irreversible for its node ID.

Feed metadata precedence survives restarts through private SQLite validation
clocks committed atomically with replacements. Cached individual reads preserve
newer observed metadata; clock-only updates never advance a cursor. Legacy rows
use their last semantic observation as a conservative barrier until refreshed.
Unchanged PR status updates write only validation and ownership metadata. An
atomic content-hash and entity check prevents a delayed reader from validating a
concurrent replacement; large status bodies are not cloned or serialized again.

GraphQL-only review decisions and merge-state status have a separate private
validation clock, committed with the same replacement. Newer REST metadata cannot
suppress these updates for the same head/base refs; an older discovery cannot
restore a review/merge result belonging to different refs. Cached projections and
restarts preserve the clocks without adding heartbeat events.

Background account detail scans collect comments, reviews, threads, and review
history independently of CI. Each successful source publishes before later
sources finish, so a timeout preserves progress and the last successful sources.
Issue comments run first. Remaining independent detail sources use batches of
up to three through the shared rate-limited queue; small embedded queues remain
sequential. A completed source publishes without waiting for its batch siblings.
Combined individual reports can still be incomplete until all required sources
are available; incomplete reports remain inconclusive for readiness.

An individual watch for one of your authored open PRs shares upstream polling
with an existing account watch when the account interval is equal or shorter.
Its registration remains durable, and `watches` exposes `covered_by_account`.
Covered watches report the availability of the account's last-known sources;
their success timestamp is a health check, not independent source revalidation.
Watches for other authors, repository discovery, and shorter intervals continue
to poll independently. Removing the account watch or losing roster membership
restores independent polling on the individual watch's next tick. Explicit PR
reads and refreshes still work independently.

PR cursors are scoped to account/endpoints/database and the `-R` selection.
Keep the same selection on later reads; changing it requires a new bootstrap.
They survive restarts and use the same cursor retention as the lower-level feed below.
An expired cursor returns `cursor_expired` (HTTP 410): run `hey-gh pr` without a
cursor, replace local state, and adopt its new cursor. Source-feed cursors from
`hey-gh snapshot` are a different API and cannot be used as PR cursors.

This is a subset of the `gh pr` command surface, not a full drop-in
replacement. It supports `-R/--repo`, `--json` projections, `-L/--limit`,
`--state open`, and `--author @me` for the account list. JSON retains an extended
cursor envelope instead of gh's bare array. Rich GitHub subcollections retain
raw fields. Single-PR view/checks resolve a numeric ID, URL, or unique open head
branch; repository inference uses `GH_REPO` or the local origin remote without
spending an unrelated gh API budget. Single-PR views are REST reports;
GraphQL-only `reviewDecision`/`mergeStateStatus` and separate policy fields are
null there. `pr checks` returns the full CI report; use `pr view --json ci` for
field selection. The legacy `hey-gh pr OWNER/REPO NUMBER` syntax still works.
Use `hey-gh prs OWNER/REPO --state all` for every author's historical PRs.

Post comments with `hey-gh pr comment 123 -R OWNER/REPO --body-file comment.md`
or `hey-gh issue comment 123 -R OWNER/REPO --body 'Fixed the retry.'`.
`--body-file -` reads stdin. PR comments also accept a URL or branch and default
to the current branch; issue comments require a number or URL. Comments must
contain text and stay within 2 lines and 300 Unicode characters after trimming.
The CLI uses the shared comment guidance and rejects invalid text before running
`gh`. Posting uses the existing `gh` login directly, without a daemon, cache,
automatic retry, or length override. Read options are rejected for comments.
After an uncertain failure, check GitHub before retrying. Cached comment reads
converge through the existing background monitoring.

`pr list --limit N` truncates the initial display with `totalCount` and
`truncated=true`; it is not a complete consumer bootstrap. Omit it when building
local state. Large open rosters page within the bootstrap byte limit (256 MiB
by default), at most 1,000 rows at a time. The first page uses `pullRequests`;
continuations use `changes` with `kind=baseline`, then replay updates from the
scan's starting watermark. Persist each page and its cursor together and drain
`hasMore`, including after the initial response. Restarts preserve scan progress;
an expired cursor requires a new bootstrap. CLI `totalCount` is null while the
initial roster is paginated. On ordinary cursor reads the limit bounds scanned
events. Incremental pages also use a byte budget: normally the configured
collection limit (64 MiB by default), with one indivisible larger observation
allowed up to the bootstrap limit (256 MiB by default). A page can contain fewer
events than requested; preserve its cursor and drain `hasMore`. PR cursor reads
select source and repository before decoding bodies, so unrelated source data
does not consume the page budget. Larger selected observations fail explicitly
without advancing a cursor. HTTP clients use `GET /v1/pr-status` with optional `repository`,
`cursor`, `limit`, `wait_seconds`, and the usual freshness parameters. Rust
clients use `ApiClient::pr_status`; Node clients use `api.prStatus({ cursor,
repository, limit, waitSeconds, read })`. `watch_account` / `watchAccount` can
set a custom polling interval (10..86,400 seconds).

PR list/change `--json` selections also project the daemon HTTP response, reducing
local transfer and caller JSON decoding. HTTP accepts `fields=number,state`;
Rust uses `ApiClient::pr_status_selected(PrStatusSelection { fields: Some(&["number", "state"]), ..Default::default() }, ...)`;
Node uses `api.prStatus({ fields: ['number', 'state'], cursor, read })` with types
that reflect selected fields. HTTP/SDK projections always retain `complete` and
`sourceErrors`; CLI stdout keeps its exact requested fields. Cursor boundaries,
activity, lifecycle kinds, changed fields, and envelope health remain unchanged.
Use the same selection for bootstrap and deltas. Omitted fields are unknown;
projected rows replace only selected local state. Small-field open-list bootstraps
read an atomically maintained compact SQLite index. Selections containing body,
CI, comments, reviews, review status, required checks, or check rollups fall back
to the original payload, as do incremental reads. Those projected reads validate
raw JSON spans and skip decoding omitted row collections. The byte budget still counts original stored bodies, so projection
preserves page/cursor boundaries and oversized-event failures. Scope/lifecycle
fields needed for internal filtering remain available; the HTTP response removes
those fields unless selected. Activity remains intact and may contain comment
bodies. This is not a new total wire-size bound. Unknown/empty fields fail before
GitHub reads or watch registration. Omit selections for a full mirror.

File-backed cache reads use a separate read-only SQLite connection, so background
writes do not hold up reads of committed evidence. Each read operation keeps its
cursor, payload, identity checks and validation clocks in one consistent snapshot;
reading does not advance freshness. The next operation sees later commits.
Bulk feed reads have their own bounded reader, so decoding large full snapshots
does not block compact indexed PR lists or point lookups used by background work.

## Repository commits and branches

```sh
hey-gh watch-repo OWNER/REPO --branch release --interval 30
hey-gh repo OWNER/REPO --branch release --refresh
# Optional: discover every branch, including new and deleted branches.
hey-gh watch-repo OWNER/REPO --all-branches
```

Repository watches persist alongside PR watches. They always include the current default branch and the local head/base refs involved in registered PR watches. Fork head branches belong to another repository; register that repository separately. Explicit branches are literal Git names, including names containing `/`. Switching a repository watch's branch options replaces its configuration; ordinary PR watches continue independently.

Branch SHAs drive reconciliation. An unchanged tip avoids commit/comparison requests and produces no duplicate feed event. A changed tip produces `old_sha`, `new_sha`, normalized commit metadata (message, authors, timestamps, parents, and URL), and a comparison URL. Each branch and its discovered commits publish in one transaction; successful branches publish independently so a later failed ref or report deadline does not discard their observations.

`branch://` snapshots contain `transition.kind`: `baseline`, `created`, `updated`, `deleted`, or `missing`. The first observation of an existing selected branch is a baseline; all-branch discovery reports branches added after a successful roster scan as created. A selected branch that initially does not exist is missing, not a proven deletion. Confirmed deletion has a null current SHA. `transition.ancestry` is `forward`, `rewind`, `rewritten`, or `unknown`; rewind and rewritten comparisons identify observed force pushes. Dropping a branch from selection is not a deletion event. A changed default branch appears in `repository://`.

Comparisons paginate and validate their commit count. Collection rechecks the branch tip to avoid publishing a comparison against a moving ref. When an old commit cannot be compared, the new tip still publishes with errors and `comparison_complete=false`; later polls retry from the original old SHA. The discovered commits are complete replacement snapshots, deduplicated by immutable SHA across refs and restarts. A baseline contains its tip, not the repository's entire history. Force-push comparisons describe newly reachable commits relative to the old tip; use the old/new SHAs and comparison URL to inspect removed history.

A repository monitor triggers a fresh conflict and head/test-merge CI fetch for affected registered PRs. Failed follow-up CI refreshes remain pending for retry across restarts. PR CI/review loops also continue independently. Polling cannot capture a branch created and deleted, or commits pushed and overwritten, entirely between polls. Webhook ingestion remains future work. Large all-branch scans are bounded by the normal report deadline and byte limits; prefer selected refs for prompt updates.

## Reviews and required checks

`pr.review_status` exposes requested users/teams, latest submitted reviews, observed approvals and change requests, dismissed reviews, and resolved/unresolved/outdated thread counts. A later comment-only review does not discard an active opinionated review. These are observed decisions, not an evaluation of required review counts, code owners, stale approvals, or merge eligibility. Full raw reviews and thread comments remain available.

`ci.failures` provides failed checks, statuses, workflows, and jobs with direct result links; job entries include failed step names, numbers, conclusions, and job links. CI still exposes all raw workflow/job/step data.

```sh
hey-gh required-checks OWNER/REPO PR_NUMBER --refresh
```

Required-check policy combines classic required status checks and effective branch rulesets. Verified native GitHub stacks use their registered trunk; chained base branches alone do not establish membership. `base_branch`/`base_sha` retain the direct diff base, while `policy_identity` identifies the effective branch and native stack and `policy_sha` records its resolved tip. Collection and consumers reject changed stack identities even when the head and direct base stay unchanged. Incomplete advertised stack metadata returns an explicit error.

Matching honors pinned GitHub App IDs, latest results, and context-specific test-merge precedence. Success, neutral, and skipped completed checks satisfy a required-check result; pending, missing, failure, and unknown remain explicit. Strict policies verify that the PR head contains the current policy branch tip through the merge base, and collection rechecks head/base/merge refs.

The aggregate state is `satisfied`, `failure`, `pending`, `missing`, `unknown`, or `not_required`. Policy permission errors, malformed responses, and missing CI sources yield unknown, never a claim that nothing is required. Inaccessible policy endpoints (ordinary HTTP 403/404, never throttling errors) back off for five minutes in the credential-scoped cache to avoid one denied request per PR sharing a base. They remain explicit policy errors; `--refresh` probes immediately and clears the backoff after a successful read. Required-check polling runs with the independent REST CI loop, so GraphQL exhaustion cannot suppress it. Rules are returned for inspection, but review/deployment/workflow rules, bypass privileges, merge queues, and overall merge eligibility are not evaluated. A satisfied required-check report is solely a status-check conclusion.

## Incremental API

1. Register watches for the repositories or PRs you want updated.
2. Read `GET /v1/snapshot` (`hey-gh snapshot`). It returns all last-known source snapshots and a cursor from the same database transaction.
3. Read `GET /v1/changes?cursor=CURSOR&limit=100&wait_seconds=30`.
4. Apply each change as a complete replacement for its `resource`, then persist `next_cursor` with your updated state. Continue paging while `has_more` is true.

```sh
hey-gh snapshot
hey-gh changes --cursor 'CURSOR_FROM_SNAPSHOT' --wait 30
```

Every change contains `cursor`, `resource`, `changed_fields`, `observed_at_ms`, and `data`. Cursors are opaque and scoped to the database, GitHub endpoints, API version, and credential fingerprint. They survive daemon restarts. Consumers should apply complete replacements and checkpoint state and cursor atomically. Repeated requests converge to current data; they do not replay superseded versions.

Resources use these names:

| Resource | Snapshot |
| --- | --- |
| `metadata://HOST/OWNER/REPO/NUMBER` | PR metadata and conflicts |
| `ci://HOST/OWNER/REPO/NUMBER` | Checks, commit statuses, workflow runs, jobs, and CI summary |
| `comments://HOST/OWNER/REPO/NUMBER` | Conversation comments |
| `review_comments://HOST/OWNER/REPO/NUMBER` | Inline review comments |
| `reviews://HOST/OWNER/REPO/NUMBER` | Submitted review metadata and state |
| `review_threads://HOST/OWNER/REPO/NUMBER` | Threads, resolution, and all replies |
| `timeline://HOST/OWNER/REPO/NUMBER` | PR timeline events |
| `review_events://HOST/OWNER/REPO/NUMBER` | Review-request/removal history with GraphQL IDs |
| `review_status://HOST/OWNER/REPO/NUMBER` | Current review requests, decisions, and resolution counts |
| `required_checks://HOST/OWNER/REPO/NUMBER` | Required-status-check policy and results |
| `repository://HOST/OWNER/REPO` | Current default branch |
| `branches://HOST/OWNER/REPO` | Last successful all-branch roster |
| `branch://HOST/OWNER/REPO/ENCODED_BRANCH` | Tip and last observed transition |
| `commit://HOST/OWNER/REPO/SHA` | Normalized immutable commit metadata |
| `prs://HOST/OWNER/REPO/STATE` | All authors’ PR roster in open/closed/all state |
| `pr://HOST/OWNER/REPO/NUMBER` | A complete combined PR report |
| `prs://HOST/OWNER/REPO/mine` | Your open PR roster |

Successful sources publish independently. A GraphQL quota error cannot suppress successfully observed REST CI or conversation-comment changes. An incomplete combined report never replaces its last complete `pr://` snapshot and has no resume cursor. Its successfully refreshed sources still produce their own events.
Individual PR/CI failures before or after collection also publish explicit
health for already tracked PRs, retaining evidence and successful-validation
times. The original typed error survives even if health storage fails. Contended
cached reads and internal detail/policy projections remain read-only for this
failure path.
GraphQL operation errors in a successful HTTP response have separate `graphql`
and `graphql_access_denied` diagnostics, including organization IP allow-list
denials. They do not invent an upstream HTTP failure. Partial GraphQL data is
never cached or published; the last-good roster remains with explicit discovery
errors. REST sources continue independently. Genuine HTTP failures and GraphQL
rate limits retain their existing status/retry behavior.
HTTP error envelopes retain `upstream_status` and `upstream_message` separately
from the local API's gateway status. Rust and Node SDKs use these explicit fields
to preserve the original GitHub failure without duplicating its message.
Invalid or absent upstream metadata falls back to the local response status for
compatibility with older daemons. GraphQL operation errors have no invented
upstream HTTP status.
Authentication, local authentication, storage, transport, and stopped-scheduler
errors keep their categories across the daemon/SDK boundary. Error envelopes
retain an explicit cause or GitHub hostname where needed, so SDKs do not wrap
local cache failures as GitHub HTTP failures or duplicate diagnostic prefixes.

Unchanged JSON, including HTTP `304` revalidation, produces no duplicate events. A first observation creates a baseline event. Changed comment bodies, added replies, deleted comments, CI reruns, and conflict changes appear when their next successful refresh observes them. `changed_fields` identifies changed top-level fields; consumers can compare comment IDs/bodies for individual changes.

Only current resource data is retained. A small cursor journal keeps at most 10,000 metadata entries per credential scope and seven days of observations; it contains no historical payloads. Pruning happens during observation writes. Old cursors return HTTP `410` with code `cursor_expired`. Recover by replacing local state with `/v1/snapshot` and its cursor. Current data remains available after cursor metadata is pruned. The legacy `snapshot` API name refers to this current data, not stored historical versions. Invalid, foreign, and future cursors return `400`. Resetting the database or changing the token/account/API version requires a new bootstrap.

These are cursor-count and age limits, not a total database byte limit. Large
current comment or CI collections and cached responses still consume space.
Incremental vacuum reclaims freed pages; page byte limits bound individual reads,
not the combined size of current data and cached responses.

PR HTTP cursor preflight reads only feed identity and retention metadata, avoiding
decoding the page twice. The page read checks retention again in its own transaction.

This feed synchronizes current data. If several updates affect a resource before a consumer reads it, only its latest observation is returned. Activity and changed-field details describe that latest observation, not every change since the consumer cursor. It cannot reconstruct a comment created and deleted between refreshes. Webhook ingestion is a future addition. Snapshot timestamps identify the last observed change, not a freshness guarantee; use watch diagnostics and report validation timestamps to assess freshness.

## CI correctness

CI covers both the immutable PR head and its current test-merge SHA when available. It fetches all pages of check runs, combined commit statuses, workflow runs, and jobs/steps for the latest attempt of each current workflow. Superseded runs do not make a successful rerun look failed. A head/base/test-merge change during collection causes a bounded recollection instead of relabeling old-head checks.

The final CI selector check can reuse REST or matching discovery evidence under
15 seconds old, bounded further by the requested cache age. This includes
evidence validated while CI was loading. Older evidence is revalidated through
REST; explicit `--refresh` still requires revalidation.

The point selector and account discovery queries also record whether head/test-merge
commits have no legacy statuses. CI can reuse explicit null evidence within the requested
cache age, saving the corresponding REST status read without another query.
Missing status fields retain REST reads. Nonempty point-query status rosters and
head-status rosters from existing account-discovery pages can validate a complete
cached REST list when node IDs, update times, context names,
states, descriptions, URLs, and the independent rollup count all agree. This
reuses the original REST payloads and numeric IDs, including every cached page;
it neither synthesizes REST objects nor issues an extra query. The proof must
already be cached when the status read starts; concurrent selector validation
does not delay CI collection to obtain it. Changed versions force REST validation
even when an older list is still within the requested cache age.
Online reuse requires every raw REST page to be less than one day old. Auxiliary
REST fields such as creator profile details retain their last REST observation;
the GraphQL proof validates the status fields listed above. The proof appears as
`graphql://HOST/OWNER/REPO/pulls/NUMBER#commit-status-versions:SHA` or
`my-open-prs://HOST/OWNER/REPO/pulls/NUMBER#commit-status-versions:SHA` with its
original point/page validation time. The newest full roster owns the proof;
conflicting rosters at the same time or newer malformed rosters retain REST.
Legacy discovery presence alone cannot renew a proof. Offline discovery scans
and page hints can still read both preceding query formats after an upgrade.
Malformed or incomplete rosters fall back to REST; missing pages and collection limits retain
their existing behavior. Newer REST evidence takes precedence on every page.
Offline reads retain the original REST page clocks as well as the proof's
validation time, and explicit refresh bypasses this shortcut.
Each CI/policy read shares one roster lookup while checking the current cached
page for newer evidence. A fresh page from an unfinished scan can supply status
evidence without changing roster completeness. Newer nonempty metadata defeats
older empty evidence and requires the full REST status payload; conflicting or
unavailable payloads remain incomplete.

Node-scoped discovery access denials exclude the affected PR nodes, including
nodes with only one denied field. Other nodes are retained separately as partial
evidence, using the original response clock. Discovery continues pagination
within its existing deadline and collection limits but still reports failure;
partial evidence cannot renew or replace the complete roster. CI reads can use
the retained nodes with the same identity, freshness, and generation checks.
Failed network operations still return their access errors; partial pages do not
satisfy generic GraphQL cache reads. Excluded nodes still count toward the scan's
byte budget. Unscoped errors, rate limits, and malformed pagination do not supply
this partial evidence.

Point selectors and existing discovery pages also provide head/test-merge
check-run counts. A zero count or explicit null rollup can satisfy an empty list
using the original query/page clock. Nonempty, missing, or invalid counts retain
REST reads. No extra query obtains these proofs; explicit refresh still reads REST.

Check-run summaries and failure lists use the newest check ID per immutable SHA,
app, and check name. GitHub's `filter=latest` is scoped to a check suite, so old
suite failures can remain in the raw `check_runs` evidence after a successful
rerun. Independent apps and head/test-merge results remain separate.

Required-check policy combines legacy branch protection and effective rulesets.
It collects only check runs and commit statuses for head/test-merge SHAs; optional
workflow/job pagination does not delay or invalidate required-check policy. This
collection never publishes a full CI recovery or clears independent CI errors.
Single-PR HTTP/CLI reads and direct required-policy reads take priority over
queued background work; watch-driven policy refreshes remain background work,
including a coalesced request that was originally queued by a watch. A foreground
waiter also promotes the background PR, CI or policy report holding its lock,
so it can finish without leaving that waiter behind the queue. After at
most three interactive dispatches in a quota lane, an eligible background
request in that lane gets a turn.
Final REST head checks for ready reports take priority over other reads of the
same foreground/background class, then yield to an ordinary read in that class
and quota. Coalesced checks inherit that priority; foreground/background fairness
still applies.
Priority never bypasses quota exhaustion, cooldowns, lane limits or deadlines,
and cannot preempt an already active socket.
Ordinary quota pacing keeps requests queued until their actual deadline, allowing
a later coalescing reader to extend the shared work. Coalescing and report
promotion wake the scheduler immediately. Background account collections also
wait within their existing deadlines for exhausted quota, reserved headroom and
GitHub cooldowns; expiry interrupts that collection and leaves unvisited PRs
queued. Ordinary callers retain prompt rate-limit feedback when the hard wait
exceeds their deadline. Cached evidence remains readable throughout.
An explicit GitHub `404: Branch not protected` establishes absence of legacy
protection even when rulesets mark the branch protected; generic 404 and 403
remain errors. Ancestry is fetched only when the combined policy is strict.
For non-strict policy, `up_to_date` is null and unavailable ancestry cannot
invalidate otherwise satisfied requirements. Head/base/test-merge selectors
and final branch-tip confirmation still bind every policy conclusion.

Summaries are `success`, `failure`, `running`, `skipped`, or `unknown`. Missing sources and missing results never become a success claim; skipped-only results remain skipped. Counts include check, status, workflow, and job results and can overlap. This describes observed CI results; use the separate `required-checks` report for status-check policy. Merge queues and overall branch-protection eligibility are not inferred.

Default polling reuses completed jobs for up to 24 hours only after validating the completed workflow's version and attempt. A changed workflow version or new attempt fetches jobs again. A newly completed workflow validates every job page before establishing that dependency cache. Explicit `--refresh` also revalidates completed jobs.

Full CI collections start workflow lists early, then admit their job reads ahead
of remaining commit sources. This lets interrupted collections fill missing
completed-attempt pages instead of repeatedly spending their budget on the same
mutable sources. Active reads continue within the existing three-read bound;
remaining commit reads wait for those jobs to finish, including their cache
lookups. Output ordering, freshness, byte limits, and quota controls remain
unchanged. Required-check policy does not read workflow jobs.

Each CI collection keeps up to three independent source or workflow-job reads
in flight through the shared queue, starting the next ready read when a slot
opens. Head and test-merge reads do not wait for fixed batches to finish. Jobs
start once both workflow lists are collected and superseded runs are removed;
unrelated check sources can still be pending. Small embedded queues remain
sequential. Source errors, head revalidation, and collection byte limits
still apply. Rotated logs include CI source timing and result counts, including
job fetches, without request credentials or response bodies.
Individual background watches hydrate comments/reviews separately from CI,
using a 30-second source cache window. They project compatible combined `pr://`
snapshots from cached CI without additional CI network requests. A cached
projection cannot clear the independent CI loop's failure health; detail polling
can succeed while CI remains explicitly incomplete.

Account CI and detail loops alternate new or changed PR heads with their ordinary
rotation. Each loop persists its own observed heads, unfinished priority work,
next lane, and ordinary cursor before starting a PR read. Discovery updates and
daemon restarts cannot consume that priority; unsuccessful attempts rotate to the
back of the priority queue, and successful attempts leave it. Each PR is attempted
at most once per cycle. New arrivals and repeatedly failing PRs cannot monopolize
the ordinary rotation. This changes refresh order without adding requests to the
cycle budget or changing source freshness and error reporting.

An incomplete CI refresh that retains newly validated completed-job pages earns
one earlier continuation in the next cycle, using the priority lane. Cached pages
and mutable sources do not renew it. Starting the continuation consumes it, and a
changed head discards it; ordinary rotation still gets its alternating turn.

Conflict status is `clean`, `conflicting`, or `unknown`, based on GitHub mergeability. GitHub can return unknown while calculating it. CI failures, pending reviews, and branch-protection blocks are separate from merge conflicts. Reports expose per-resource validation times and whether data came from cache, network, or conditional revalidation.

## Queue and caching

Charged reads are paced across the remaining quota window, including above 1,000 remaining. Parallel requests reserve separate pacing slots at dispatch. A validator alone does not bypass pacing: only a representation last validated with HTTP 304 may probe while more than 100 requests remain. A subsequent HTTP 200 revokes that exemption. HTTP 304 preserves existing pacing debt; primary exhaustion, shared secondary cooldowns, and minimum request spacing still apply.

The `traffic` section of `logs --summary` counts retained dispatches and response
headers separately from completed jobs, grouping endpoint, priority, conditional
status, and completed-job cache decisions. Quota windows carry opaque auth and
daemon instance identifiers and observed header ranges, never credentials or
response bodies. Header movement includes other machines and clients; it is not
a count of this daemon's charges. Check retention coverage and unmatched responses
before comparing bounded windows. Older logs lack these records and cannot prove
zero traffic. Cache hits remain visible in `status`; they do not dispatch requests.

The daemon owns one bounded queue: active, waiting, and retrying distinct requests share its 256-request capacity. Identical in-flight requests share one operation, even when callers cancel. Eligible ready requests retain FIFO order within each bucket and priority class; an exhausted resource bucket does not block a ready request from another bucket. REST core, search, and GraphQL use separate budgets, refined from GitHub's `x-ratelimit-resource` header.

Core and GraphQL pacing estimate shared allowance consumption from response
headers, including other clients and GraphQL queries that cost multiple points.
This estimate is scoped to each authentication provider and reset window; it is
not an attribution of exact query cost. Work queued before its quota slot stays
active demand during long pacing waits. A genuinely idle client still discards
the old estimate when it resumes. Both resources retain 100 points of headroom;
caller deadlines and server cooldowns remain authoritative.

Three of those slots are reserved for interactive PR/CI/policy reads, so polling
cannot fill every admission slot. Embedded queues reserve `min(3, capacity / 4)`
slots (none below four). New background requests are rejected at that boundary;
interactive requests are rejected only at total capacity. Equivalent requests
coalesce before either check and interactive waiters promote existing work.
The reserve adds neither sockets nor waiting tasks and does not bypass quotas,
deadlines, or the scheduler's background fairness rule. It is admission headroom,
not a guarantee that every concurrent report will finish.

`status` reports `interactive_reserved_slots` and process-lifetime
`queue_full_rejections`; rejected admissions made no network attempt and are not
completed requests in `logs --summary`. `queue_full` means local backpressure,
not invalid authentication or passing CI. Source-level failures remain explicit
in incomplete reports; cached validation times and existing cursor semantics
are preserved.
After one second, retry only the affected `pr checks` or `pr view` with
`--timeout 15`; if still saturated, inspect `status`, `watches`, and
`logs --summary --since 900` before retrying. Avoid account-wide refresh loops.
Retain saved feed cursors on failure; a later complete targeted read can publish
recovery without clearing unrelated source failures. A cached read does not
establish freshness, and even complete CI does not establish merge readiness.

For `api.github.com`, at most eight network attempts are active, with separate
limits for core CI/lifecycle, GraphQL, and REST comment/review work. Other hosts
use at most three attempts. The detail lane shares core quota and minimum spacing
with lifecycle and CI reads, but a stalled review request or body cannot hold
their socket. FIFO ordering applies within each lane and priority class. Throttle headers update
budgets and shared cooldowns before reading the body. `status` distinguishes
`active_requests` from all `outstanding_requests` and reports the
`max_active_requests` bound (one for a single-slot queue).
Usage counters are per daemon process. `network_requests` counts attempts,
including retries; `conditional_requests` counts attempts carrying an HTTP cache
validator; `not_modified_responses` counts received HTTP 304 responses. A
conditional request can return changed content, so it does not alone establish
cache savings. These are traffic measurements, not exact quota charges; quota
headers also reflect other tools using the same GitHub account.
Scheduler diagnostics use a random `request_id` shared by one queued job's
retries and final `GitHub request finished` record. Completion includes attempt
count, current-attempt HTTP status when received, elapsed queue/work time and
transport/cache outcome; it does not establish PR completeness or readiness.
Dispatch records mark final head checks with `completion_validation`.
Completion also includes a fixed endpoint class (for example `check_runs`,
`timeline`, `repository`, or `branch_protection`) to distinguish source failures without exposing selectors.
Unknown REST paths use `rest_other`; no URL or query values become labels.
Separate jobs have separate IDs, including after restart. IDs do not encode
credentials, URLs, queries or response bodies. Correlate a retry with its own
completion, rather than inferring recovery from another job in the same bucket.
Account entries in `watches` also expose durable `last_cycle` and `ci_last_cycle`
summaries for the latest completed detail and CI hydration cycles. Each separates
successful refreshes, genuine failures, local budget interruptions and unvisited
deferred PRs. They survive restart and remain separate from aggregate success
timestamps, which can stay empty while work is queued. Discovery-only preparation
does not replace hydration progress; reading it neither contacts GitHub nor moves
observation cursors. Successful work counts do not establish freshness,
completeness, required-check satisfaction, or merge readiness.
Hydration cycle logs include `started_at_ms` and `finished_at_ms` matching the
saved watch summary. Use those clocks to correlate a summary with its own log
entry: independent status and log reads can straddle a cycle boundary.
Cycle logs use `roster_available` and `roster_cached_only` for the roster input.
A usable cached roster does not establish discovery recovery; consult the
account's `discovery_last_error` and `discovery_last_success_at_ms` in `watches`.
When GraphQL account discovery is denied, account status also tries bounded REST
discovery for new accessible PRs. It validates at most five new candidates within
30 seconds (or the remaining refresh budget), preserving known PRs and source
errors. Search results only add verified authored open PRs; missing, capped or
incomplete search results never establish closure. The discovery error remains
explicit, and new rows have unknown CI until hydrated. A later complete scan
retires the partial additions. The fully paginated `all_my_open_pull_requests`
SDK collection remains separate and never substitutes the partial REST overlay.
Private credential-scoped candidate permission errors cool down for five minutes
so inaccessible candidates cannot repeatedly consume the whole probe budget.
Explicit refresh probes access again.
 Remaining quota and reset headers adjust pacing. Primary exhaustion pauses its bucket; secondary limits impose a shared cooldown. Repeated secondary-limit episodes increase backoff across requests, even when intervening requests succeed; concurrent responses within a cooldown count as one episode. Local escalation caps at 15 minutes plus jitter and resets after 15 quiet minutes beyond the cooldown. Retries honor `Retry-After` (seconds or HTTP dates) as a minimum and reset times, use bounded backoff and jitter, and never retry ordinary authorization failures. Work that cannot finish within its deadline fails explicitly. HTTP bodies (16 MiB by default), collected data (64 MiB), bootstrap data (256 MiB), incremental page bodies (64 MiB, or one larger observation up to 256 MiB), pagination, attempts, and report execution time are bounded. Queue overload and rate limits become `503` responses with `Retry-After`; report deadlines become `504`.

Storage keeps one current value per source and compact cursor metadata. Historical
payloads are removed automatically when an older database is first opened by this
version. The migration preserves current data, cached responses, watch registrations,
feed cursors, and validation state, then compacts the file. Stop older daemon/SDK
processes before upgrading a shared database; the new journal schema does not support
old writers. Incremental vacuum reclaims freed pages on subsequent observation writes.

REST responses persist in SQLite with ETags, Last-Modified values, and pagination links. Pagination validates each page separately and rejects cycles and cross-origin links. GraphQL read queries use local freshness caching and the same scheduler; GraphQL errors or partial results are never cached as successful responses. Mutations are unsupported.

PR evidence is bound to GitHub's native node identity. If a recreated repository
reuses a PR number, validated identity changes fence that repository's response
and derived caches. Old comments, reviews, policy, and CI remain unavailable for
the new node until collected again, even when commit SHAs match. Ownership and
ordering survive restart; delayed older work cannot publish evidence or failure
health for the new node. Internal ownership changes alone do not advance cursors.
REST discovery can supply a validated replacement without clearing independent
GraphQL discovery failures or guessing the retired node's final lifecycle.

PR reads share evidence and recovery updates across repository-name casing.
Resource keys and displayed repository spelling stay stable for existing
incremental consumers. Account discovery treats casing changes as the same PR,
including REST additions and permission cooldowns, so they cannot create false
closure or removal events. Cached reads reuse legacy repository aliases; branch
names, refs, query values, credentials, and identity generations remain distinct.

The default database lives under your platform's cache directory at `hey-gh/cache.sqlite`. `serve --cache PATH` changes it. GitHub tokens are obtained with `gh auth token --hostname HOST`, kept in memory, and never stored in the database. Restart the daemon after changing gh accounts or credentials. The SDK reloads local API credentials on each request, so an existing client can reconnect after a daemon restart. Unix cache and local API credential files are created with mode `0600`. Local credentials live in the private `hey-gh/instances` cache directory and are removed at shutdown. Embedded `Api::router()` is deliberately unauthenticated for tests and trusted embedding; use `router_with_auth` to protect an embedded HTTP server. A file lock prevents two daemons owning the same database. GitHub Enterprise uses `serve --hostname github.example.com`.

CLI commands and `ApiClient` connect to the daemon, sharing its budget across processes. Clones of an embedded `Client` share a scheduler; separately constructed embedded clients and unrelated `gh` commands do not share that scheduler. GitHub's returned quota headers still reflect requests made elsewhere.

## Rust SDK

```rust,no_run
use hey_gh::{ApiClient, Freshness};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api = ApiClient::new("http://127.0.0.1:8787/".parse()?)?;
    api.watch("OWNER/REPO", Some(123), 30).await?;
    api.watch_repository("OWNER/REPO", vec!["release".into()], false, 30).await?;
    let ci = api.ci_for_pr("OWNER/REPO", 123, Freshness::Revalidate).await?;
    assert!(ci.complete);
    let baseline = api.bootstrap().await?;
    let changes = api.changes(Some(&baseline.cursor), 100, Duration::from_secs(30)).await?;
    // Apply changes.changes, then checkpoint changes.next_cursor.
    Ok(())
}
```

For embedding without the daemon, use `Client::from_gh(Config::default()).await?`. It provides generic conditional REST reads, paginated reads, read-only GraphQL queries, typed PR decoding, CI/PR reports, and the local feed. `Config` controls queue capacity, deadlines, pacing, retention, and body bounds. See [examples/sdk.rs](examples/sdk.rs).

## JavaScript / TypeScript (napi-rs)

The native Node package wraps the shared Rust daemon SDK, including PR/CI and required-check reports, repository commits, watches, and durable cursor reads. Start `hey-gh serve`, then build the addon:

```sh
cd bindings/node
npm ci
npm run build
```

```js
import { ApiClient } from './bindings/node/index.mjs'

const api = new ApiClient()
await api.watchRepository('OWNER/REPO', { branches: ['release'], intervalSeconds: 30 })
const ci = await api.ciForPr('OWNER/REPO', 123, { refresh: true })
const baseline = await api.bootstrap()
const updates = await api.changes({ cursor: baseline.cursor, waitSeconds: 30 })
```

All methods return Promises. Account PR pages use camelCase; other responses preserve the API's snake_case fields. TypeScript declarations cover the reports and incremental feed. `HeyGhError.code` supplies stable error codes, including `cursor_expired` and `rate_limited`; cached-only reads make no GitHub requests. Node callers use the daemon's automatically managed local credential, so no additional login is needed. See [bindings/node/README.md](bindings/node/README.md) for options, local packaging, and validation. The package has not been published to npm.

## HTTP routes

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/v1/status` | Queue, cache, and quota metrics |
| GET | `/v1/pr-status` | Account-wide PR status or changes since a PR cursor |
| GET | `/v1/prs/OWNER/REPO` | Your open PRs |
| GET | `/v1/prs/OWNER/REPO/NUMBER` | Full report |
| GET | `/v1/prs/OWNER/REPO/NUMBER/ci` | CI-only report |
| GET | `/v1/prs/OWNER/REPO/NUMBER/required-checks` | Required-check policy report |
| GET | `/v1/repos/OWNER/REPO` | Branch reconciliation and commits |
| GET | `/v1/repos/OWNER/REPO/prs` | All authors’ PRs; `state=open/closed/all` |
| GET | `/v1/snapshot` | Atomic bootstrap |
| GET | `/v1/changes` | Cursor feed; optional long polling |
| POST | `/v1/watches` | Persist a monitor |
| GET | `/v1/watches` | Monitor configuration and diagnostics |
| DELETE | `/v1/watches/ID` | Stop and remove a monitor |

Read routes accept `refresh=true`, `cached_only=true`, or `max_age_seconds=N` (at most 86,400). Watch bodies contain `repository`, optional `pull_number`, and optional `interval_seconds` (10..86,400, default 60). Omitting the pull number or setting it to zero discovers your open PRs. A branch watch body uses `kind="branches"`, `repository`, optional `branches` (JSON string array), `all_branches`, and `interval_seconds`; it cannot specify a nonzero PR number. Repository reads accept `all_branches=true` and `branches` as a JSON string array encoded in the query string (a comma-separated list also works for simple names). The SDK handles encoding automatically. Feed parameters are optional `cursor`, `limit` (1..1,000), and `wait_seconds` (0..30). Invalid account page limits are rejected before GitHub requests or durable monitoring registration.

## Validation and coverage

```sh
cargo fmt --all --check
cargo test --locked -j 2
cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings
```

The tests use a local GitHub mock, require no real login, and exercise persistence, cancellation/deduplication, overload, independent quota buckets, secondary cooldowns, pagination and thread replies, reruns, head changes, partial errors, cursor recovery, auth consumption, and daemon SDK access. The 125 Rust tests cover bounded log rotation, safe offline summaries, offline incremental reads, PR identity isolation across cache reuse, delayed responses, separate clients, and restart, repository-case aliases without cursor churn, false discovery lifecycle events, or case-sensitive ref confusion, and selected-field decoding with unchanged cursor boundaries and explicit corruption errors. The napi-rs addon has 11 passing Node integration tests plus TypeScript checks, including the authenticated Rust daemon, payload conversion, options, watches, replay/long polling/expiry, credential reload after restart, and stable error details. A read-only live smoke test also exercised an actual PR's checks, statuses, workflow jobs, comments, threads, review-event query, timeline, and conflicts through the existing `gh` login, plus repository commit snapshots, zero-network cache-only reads, and denied-policy handling.

A broader source search located `../ashby-mcp/github_scrape_pull_requests.py`, a concrete PR scraper. Its all-state discovery, issue/inline comment fields, review IDs/author types/state/timestamps, review-request/removal history (including GraphQL event IDs), merge/close timestamps, and thread reply/review associations are exposed by `prs --state all` and PR source snapshots. Its purpose-built archival SQLite schema and staged migration commands are not reproduced; hey-gh uses the durable snapshot/feed model. The candidate is not confirmed as the originally intended script.

The original source search did not establish exact legacy-script parity: `../poe-tooling` contains native TypeScript compiler queue tooling, not a GitHub PR-fetching script. Related review queries under `../poe2/scripts/review-enforcer` were inspected; their thread root comments, author type, reviews, and review-request timeline data are covered. No other project's files were changed.

The user subsequently supplied the intended standalone behavior: account-wide open PR status, gh-style commands, and a cursor feed with first-class closures/merges. That behavior replaces the unavailable-script prerequisite; SOURCE_AUDIT.md remains a historical source comparison.

### Durable local service

`hey-gh service start` installs a per-user launchd job on macOS or systemd user
service on Linux, then waits for authenticated local API health. It uses the
existing `gh` login, default cache and localhost:8787. The service survives the
requesting worker's exit and retries daemon exits, including clean shutdowns.
On Linux, a user service manager must be available; enable lingering with
`loginctl enable-linger USER` when the service must also survive logout.

`hey-gh service status` reports API health and whether the service is registered
and active. A healthy external listener is reused; the service stands by until
that listener exits. An occupied but unhealthy port is never killed. Concurrent
starts share an installation lock, and daemon startup binds the port and locks
the cache before authenticating or starting background watches.

`hey-gh service restart` reloads only the managed job and its owned child. Use
`hey-gh logs` plus `journalctl --user -u hey-gh.service` on Linux or
`launchctl print gui/$(id -u)/local.hey-gh` on macOS for recovery. Missing service
management and unhealthy startup return errors with recovery steps. Credential
environment values are never written to service definitions; authenticate `gh`
as the same OS user. Location overrides (`GH_CONFIG_DIR`, `XDG_CONFIG_HOME`,
`XDG_CACHE_HOME`) are retained. A normal start never replaces a running daemon;
explicit restart applies changes to the managed service environment.

Package upgrades install/reload the service on companions and update the command
card. Supervisors retain their existing PR monitor ownership unless a service was
explicitly installed. Custom `serve --listen/--cache/--hostname` instances remain
separately owned; the service command manages only the default local instance.
