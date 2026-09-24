---
name: hey-gh
description: Read GitHub PR activity, comments, CI status, and conflicts with cached hey-gh commands and resumable incremental updates.
---

# hey-gh command card

Use `hey-gh` for read-only GitHub PR monitoring. It shares a persistent cache,
request queue, and rate-limit backoff through a local daemon; authentication uses
the existing `gh` login. If the daemon is unavailable, start `hey-gh serve` in a
background terminal. Use `hey-gh COMMAND --help` for options.

```sh
hey-gh pr                                  # All your authored open PRs + cursor
hey-gh pr list -R OWNER/REPO                # Restrict to a repository
hey-gh pr view 123 -R OWNER/REPO             # Comments, reviews, conflicts, CI
hey-gh pr checks 123 -R OWNER/REPO           # Detailed head and test-merge CI
hey-gh required-checks OWNER/REPO 123        # Separate required-check policy
hey-gh pr view 123 -R OWNER/REPO --timeout 15 # Total caller read deadline
hey-gh pr checks 123 -R OWNER/REPO --timeout 15
hey-gh required-checks OWNER/REPO 123 --timeout 15
hey-gh pr --cursor 'CURSOR'                 # Changes + next cursor
hey-gh pr changes --cursor 'CURSOR' --wait 30 # Long poll
hey-gh pr --cached-only                     # Zero GitHub requests
hey-gh pr --refresh                         # Bounded detailed refresh
hey-gh watches                             # Polling health and source errors
hey-gh logs --tail 200                      # Recent rotated daemon diagnostics
hey-gh logs --summary --since 900           # Safe aggregates for the last 15 minutes
hey-gh logs --path                          # Stable log directory
hey-gh watch-repo OWNER/REPO --branch main   # Track branch tips and new commits
hey-gh snapshot                            # Source snapshots + source cursor
hey-gh changes --cursor 'SOURCE_CURSOR'     # Repository/source changes
```

For agent triage, compact CLI output with an explicit projection:

```sh
HEY_GH_PR_FIELDS='id,number,title,url,repository,state,removed,headRefOid,headCiState,conflicts,complete,sourceErrors'
hey-gh pr --json "$HEY_GH_PR_FIELDS"
hey-gh pr changes --cursor 'CURSOR' --json "$HEY_GH_PR_FIELDS" --wait 30
```

Keep identity, lifecycle, and health fields in the projection and use the same
fields for bootstrap and updates. Event kinds and activity retain new comments,
reviews, CI changes, and closures/merges. Projected rows replace only the selected
triage state; omitted fields are unknown, not deletions or complete PR evidence.
PR list/change projections compact both CLI stdout and the daemon HTTP response.
Projected stored reads skip decoding omitted row fields while preserving the
original stored-byte limits, page boundaries, scope/lifecycle filtering, and
activity events. Raw stored JSON is still read. HTTP/SDK
projections always retain `complete` and `sourceErrors`; CLI stdout retains only
requested fields. Use `fields=number,state` for HTTP, Rust
`ApiClient::pr_status_selected` with `PrStatusSelection`, or Node
`api.prStatus({ fields: ['number', 'state'], cursor, read })`. Omitted fields are
unknown, and projections do not impose a total wire-size bound.

For usage investigations, `logs --summary` reads retained rotations offline.
Check `retention_covers_start`, gaps and partial records before treating counts
as covering the requested window. It correlates each retry with its own final;
a missing final does not prove the job is active. Request counts describe completed
jobs, not exact window traffic, quota charges, freshness, or PR readiness. Account cycle summaries separate failed
reads from local budget interruptions; deferrals repeat across cycles and do not
count unique PRs.
`source_refresh_failure_records` separates discovery, CI, and detail collections:
review-thread/event access denials are distinct from discovery access denial.
Counts are distinct retained warning records after exact rotation deduplication,
not requests, attempts, unique PRs, or current health. Absence of a source in the
summary does not prove success. Only static source/error labels are emitted.
Endpoint timing percentiles include queuing and retries for completed jobs,
including failures; they do not isolate GitHub response time. Missing timings
are counted as unavailable.

Default PR reads register durable background monitoring. Initial details can be
pending (`complete=false`); inspect `sourceErrors` before interpreting missing
data. `headCiState` is a head-commit rollup, while `ci` includes test-merge results;
neither alone proves merge eligibility. Refresh can be slow under GitHub limits.
Use normal cursor reads for ongoing updates; the background watch polls GitHub.
Avoid repeatedly refreshing the entire account. For a readiness decision, fetch
the specific PR's checks and required-check policy and confirm current conflicts,
reviews, and freshness. A comment-source timeout does not establish CI failure;
missing or incomplete evidence does not establish readiness.
The account view already monitors your PRs; additional per-PR watches are usually
unnecessary. `watches` marks overlapping registrations `covered_by_account` when
the account watch owns their polling.

For a full incremental PR mirror, bootstrap with `hey-gh pr` without a display
limit or `--json` projection.
Apply each `changes[].pullRequest` as a full replacement; `removed=true` removes
it from the open list. Events identify opened, closed, merged, reopened, comments,
CI, commit, and conflict activity. Save the returned `cursor` atomically with
consumer state. Drain `hasMore` even when `changes` is empty. Keep the same `-R`
selection. On `cursor_expired`, bootstrap again and replace local state. PR and
source cursors are separate. The feed records observations; polling can miss
transitions entirely between reads.

PR list/change commands can emit valid JSON and exit 1 when the envelope or
returned rows contain source errors. Parse stdout even on that exit; preserve
the cursor, replacements, hasMore and explicit errors atomically. Exit 0 from
a fast read does not prove complete=true: details may still be hydrating.
`coverage` names the repository filter and reports `returnedRows` and
`returnedRowsComplete` for this page's stored PR evidence, before projection.
`accountDiscovery` separately reports last-known account scan completeness
(null means unknown), errors and poll/success times. An unrelated organization's
restriction can leave returned-row coverage complete while discovery and the
aggregate envelope remain incomplete (exit 1). This does not prove the selected
repository roster is complete; an empty page is not proof of no PRs. Preserve
target row source errors and validate specific PRs with bounded reads as needed.
Transport/validation failures can instead have no envelope; never advance a
cursor without a successfully parsed response. A complete individual PR read
also publishes a replacement for an already tracked PR, clearing its recovered
CI/detail errors. CI-only recovery preserves independent detail failures.
Cached PR/CI reads never wait for an active upstream refresh or make GitHub
requests, including on head-change retries. During lock contention they read
available evidence without publishing source snapshots or recovery replacements;
no observation cursor is supplied. Missing metadata returns unavailable JSON
(`complete=false`, `available=false`, `code=cache_miss`) and CLI exit 1.
Missing review/CI sources remain explicit errors. Source validation times identify
older evidence; `observedAtMs` is the read time, not a fresh GitHub validation.
PR view projections retain validation times and source errors. Cached-only
recovery validates cached evidence, not freshness or merge readiness.

Single-PR `pr view`, `pr checks`, legacy PR reads, `ci`, and `required-checks`
accept `--timeout SECONDS` (1..3600). This opt-in total CLI budget includes PR
selector/branch resolution and cache transport; `--wait` still controls cursor
long polling and cannot be combined with it. Successful reads retain their
existing output. A caller deadline emits parseable JSON and exit 1 with
`code=deadline`, `deadlineExceeded=true`, `complete=false`, and explicit
`pendingSources`/`sourceErrors`. Parse stdout even on exit 1.
PR/CI reads capture cached evidence before waiting for validation. Deadline
fallback retains that evidence's original head, validation times, and independent
source errors; `available=true` means evidence exists, not that it is current.
Cold cache, unresolved selectors, or required-check policy deadline results are
structured unavailable (`available=false`, `state=unknown`, no validations).
Policy expiry never synthesizes satisfied/not_required or merge readiness.
Fallbacks have `cursor=null`; preserve saved feed cursors and resume them normally.
The deadline stops the CLI's wait and additional calls to the daemon. Previously
started daemon handlers and shared GitHub work can continue; background watches
and registrations persist.

Rust uses `ApiClient::pr_status`; HTTP uses `GET /v1/pr-status`; napi-rs Node
bindings use `api.prStatus({ cursor, waitSeconds: 30 })`. These share the daemon.
`hey-gh` implements a read-only subset of `gh pr` with an extended JSON envelope.
