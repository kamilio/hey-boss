# Detail-watch publication consistency

Issue #360 exposed a contention race: the PR status feed contained a newly
fetched comment, but the combined `pr://` bootstrap snapshot still held the old
conversation after the detail watch reported success.

The independent detail loop refreshes conversation sources and then builds a
combined report using cached CI. A concurrent CI poll can hold the CI report
lock. Treating this internal projection like an ordinary cached caller made
lock contention mark the entire observation read-only, silently skipping its
combined snapshot publication.

Commit `b2da29d8a3be05691011fadae49cf5781ac36b9e` makes the background
projection wait for report locks within the existing deadline. It preserves
independent CI failure health and makes no additional CI requests. Ordinary
cached callers still return without waiting and cannot overwrite an active
refresh.

Regression coverage:

- `monitored_projection_waits_for_publication_while_cached_read_stays_nonblocking`
  holds the lock explicitly, checks both caller behaviors, and releases it to
  verify that background publication remains enabled.
- `independent_detail_watch_reuses_ci_without_clearing_ci_failure_health`
  retains the original new-comment assertion, checks conversation equality
  between the PR feed, combined bootstrap, and source change feed, retains the
  CI failure, and verifies that only the CI loop fetches checks.

CI evidence:

- [Original Linux failure, run 36185694642](https://github.com/kamilio/hey-boss/actions/runs/36185694642/job/108238375614).
- [Complete successful Check run 36194851078](https://github.com/kamilio/hey-boss/actions/runs/36194851078)
  at `13c4b188dfbd5ad5a8f25cbac5f2d2cd86bbe977`, containing the fix:
  macOS job `108268235060`, Linux job `108268235096`, and mobile job
  `108268234906` all succeeded.
- [Subsequent run 36195615972](https://github.com/kamilio/hey-boss/actions/runs/36195615972)
  passed Linux and mobile. macOS failed the unrelated
  `issues::worker_approvals::tests::inbox_tool_skip_returns_cancellation_without_stopping_the_worker`
  assertion. That run is not a complete successful validation.
