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

## Installed verification — September 20, 2026

The 51 distinct Rust tests passed (40 GitHub integration tests, seven repository
tests, two SDK unit tests, and two CLI unit tests). Clippy with warnings denied
and Rust formatting passed. The initial three issue regressions failed on the
original implementation because no recovery/failure replacements were emitted.

The installed daemon reproduced the issue's exact same-head recovery on
`poe-internal/poe2` PR 15033. Before the individual read its feed row retained
`details: request deadline exceeded` and `complete=false`. The cached-only
individual read returned `complete=true` at unchanged head
`4bd3fbb4b4a8b4bd20bdf30d9c7ac8ff5068f7dc`. The subsequent incremental read emitted
one replacement with `complete=true`, empty `sourceErrors`, and an advanced
cursor. No explicit upstream refresh was requested by this verification.

Installed on this MacBook, `kamils-macbook-pro.local`, and `devbox`. The Mac
binaries match SHA-256
`f255148ebcfe68c61e5d67840154c33139d06e45fb8153abf4b66a487e7f02bb`;
the command-card skills on all machines match
`64ce866a70d374c06fbeac9783f2fcab6f9168279bf59a045b27fa8ae9bbdfcc`.
Devbox's patched source hashes match this MacBook's source. The existing local
daemon was gracefully restarted to load the installed binary, retaining its
cache and watches. Devices without an existing hey-gh daemon received the CLI
and skills without starting additional services.

The isolated issue viewer passed visual review at 1440×1000 and 390×844,
responsive checks at 390, 768, and 1440 pixels, Markdown comment preview, and
keyboard navigation. There was no document overflow or browser console error.
Its Chrome session, server, screenshots, reports, and temporary build snapshot
were removed after review. Existing checkout edits and other browser sessions
were preserved.

The required hey-boss Fly deployment reused its current immutable mobile image
because this fix changes a local hey-gh daemon. Rolling deployment checks and
the public `/healthz` endpoint passed; uncommitted mobile changes were preserved.
