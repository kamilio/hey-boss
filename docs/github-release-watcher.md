# GitHub Release Watcher

An explicitly started, read-only release queue. It does not assign agents,
change issues, send notifications, or add prompts. The existing issue watcher
is named **GitHub PR watcher**.

```sh
hey-gh release profile poe2 > release.json
hey-gh release add --config release.json --state release.sqlite 17884 17872
hey-gh release poll --state release.sqlite
hey-gh release watch --state release.sqlite
hey-gh release status --state release.sqlite
hey-gh release remove --state release.sqlite 17884
```

`poe2` defaults to merged PR numbers. `poe-code` defaults to commit SHAs and
checks main validation without requiring publication. The JSON project profile
selects `repository`, `branch`, `target` (`commit` or `pull_request`), and gates.
Each gate names a workflow file, its purpose, and required job/step names.
Job names are exact unless they end in `*`; `count` requires the exact number
of matching matrix jobs. Every selected job and required step must succeed.
`step_counts` requires exact counts for repeated composite-action step names
(one by default).
Changing the policy requires a separate queue, preserving the old evidence.

The poe2 profile covers post-merge tests, paired Convex, Joiner/Poe workers,
and app publication. Other services or mobile releases need their own gates.
Profiles describe reviewed workflow layouts; a renamed/missing job or step
does not silently become successful. Deployment no-change alternatives must
be configured explicitly with `unchanged_steps` and a successful named proof.
They produce `unchanged`, not `passed` or a claim of a new deployment.

A PR resolves to its merged commit on the configured branch. A commit must
be an ancestor of that branch. Cancelled, queued, skipped, and missing builds
remain watched; later runs count only when Git ancestry proves they contain
the target and still belong to the branch. The report identifies `exact` or
`successor` coverage and links the run attempt that supplied the evidence.
Successful evidence and the branch are rechecked before certification.
Cancellation after all configured jobs succeeded preserves their proof;
`workflow_conclusion` always exposes the raw workflow result separately.

`verified` means the configured evidence passed; `recovered` also retains
observed failures. Neither establishes that a particular commit caused a
failure, or that the original commit passed when only its successor did.
Each deployment gate reports the selected jobs' completion time, a confirmation
from the deployment pipeline, not an independently measured production switch
time. A no-change confirmation has no new deployment time.

`checked_at_ms` is the local observation time. `oldest_source_validation_ms`
and the poll response's `validations` expose source freshness. `status` is
offline and does not refresh those times. Errors produce `unknown` while
retaining older evidence and observed failures. Inspect the JSON even when
`poll` exits nonzero.
An interrupted gate retains the run records already collected, with
`history_complete=false`, `satisfied=false`, and no new gate confirmation.
An interrupted, unknown report with no completed gates or confirmations stops
without another branch read. Watching/failed reports, completed gates, and
confirmations still require the final branch check.
Historical `confirmations` retain when a deployment was confirmed even if a
later rerun, force-push, or read error changes the current report.

`watch` polls every minute through the shared daemon's background scheduler.
Manual `poll` uses its interactive lane. GitHub App routing remains limited
to CI reads in configured installations. A poll has bounded work per target
and rotates the queue after partial batches. Restarts retain the queue;
concurrent pollers are excluded while targets can still be added or removed.
Completed entries remain available for rerun detection until explicitly removed.

History is paginated and dense date windows are split below GitHub's 1,000
result search cap. Missing pages, changing counts, limits, permission errors,
and deadlines never establish success. Branch comparisons follow up to 100
immutable pages within the collection byte budget. Only complete, consistent
rosters support run filtering and parent-link proofs; truncated or malformed
rosters retain individual ancestry checks. Parent traversal visits edges once,
including when commits arrive out of order. A complete small comparison uses
per-commit workflow history directly, avoiding unrelated days. Date-based
discovery can reuse closed-day pages after a complete branch comparison. Large
archives with usable node IDs refresh known workflow metadata in GraphQL batches
of at most 100, using the requested freshness and shared collection byte budget.
Current-day REST discovery retains new dispatches on those same commits. Missing or mismatched nodes and
API errors remain unknown. Nullable branches or unsynchronized suite metadata
retain REST history reads. App-backed repositories, explicit refreshes, and
cached-only reads keep their existing REST path.
Completed first attempts can collect complete job/step evidence in groups of up
to 20 runs. A fresh version check after collection detects a rerun starting
during the query. Rerun attempts, oversized job/step rosters, and unsynchronized
suite metadata retain explicit REST attempt reads. Completed job versions are
reused for up to one day after fresh parent validation; explicit refresh bypasses
that memo. Interrupted batches retain earlier completed evidence without
confirming partial history.
A page observed before that day closed must be revalidated, or remains incomplete
for cached-only reads. Current-day discovery keeps its normal freshness.
Date-based history older than a year or requiring
over 100 listing requests per workflow is reported incomplete. Cached progress
is reused on subsequent polls.

Tests replay sanitized job/step evidence from 48 real workflow runs across
both repositories, including successful workflows whose validation was skipped.
Synthetic HTTP tests exercise ancestry, cancellation chains, rerun and branch
races, pagination, and shared-cache reuse; queue tests cover restarts and stale
results. These fixtures are offline and do not run or cancel real builds.
