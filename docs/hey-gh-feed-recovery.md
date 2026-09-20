# hey-gh PR feed recovery (issue 64)

Individual PR and CI reports previously published source snapshots without
updating their account PR feed row. Account-loop errors remained in that row
even after a complete cached report for the same head. Consumers could advance
their cursors indefinitely without receiving a recovered replacement.

The existing hey-gh source at `/Users/kjopek/Workspace/hey-gh` has no Git
repository. The implementation and regression tests are therefore retained as
`patches/hey-gh/issue64-feed-recovery.patch` in this repository. Apply it to the
existing source with `tools/apply_hey_gh_issue64.sh /path/to/hey-gh`. The helper
validates all hunks before applying, recognizes an already applied patch, and
rejects changed baselines without writing anything.

Individual observations now rebuild only an already tracked PR from source
snapshots, without GitHub requests or a roster scan. CI observations replace
only CI health. Full PR observations replace CI and detail health independently.
Failures remain explicit even when an older successful source snapshot remains
available. Feed publication uses the existing per-row lock, head/merge SHA
checks, comparison, cursor persistence, and notification path. Repeated
unchanged cached observations do not emit duplicate replacements. Account
cycles retain their existing single publication per PR; nested report reads
do not publish intermediate rows.

The patch also documents the CLI exit convention in the canonical hey-gh skill:
exit 1 can accompany a valid JSON envelope containing source errors. Consumers
must parse that envelope, apply replacements, drain `hasMore`, and checkpoint
state and cursor together. An exit 0 fast read can still be hydrating. Cached
recovery confirms available cached evidence and does not assert freshness or
merge readiness. PR and source cursor scopes remain separate.

Validation uses hey-gh's existing local GitHub mock, with regressions for
same-head cached recovery without upstream requests, duplicate suppression,
CI recovery retaining detail failures, individual CI/detail failures, new-head
rollup invalidation, and exclusion of untracked PRs. Run:

```sh
cargo test --locked --test github
cargo test --locked --lib
cargo test --locked --test repository
cargo clippy --locked --all-targets -- -D warnings
cargo install --path . --locked
hey-gh install
```

The CLI contains the daemon. Replace the installed binary and restart an
existing daemon gracefully to load the new implementation, preserving its
cache and watches. Recovery replacements appear when individual observations
or subsequent account cycles validate their sources. There is no hey-gh web
UI or Fly service; the hey-boss issue viewer is checked separately at desktop
and phone widths.
