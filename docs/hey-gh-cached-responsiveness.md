# hey-gh cached-read responsiveness (issue 72)

Cached PR reports and CI observations waited on the same asynchronous report
locks as upstream refreshes. A stalled refresh could therefore consume the
120-second report deadline even though `--cached-only` needed no GitHub request.

Cached PR/CI reads now try their report locks without waiting. On contention,
nested observations share a read-only publication scope: they assemble available
cached evidence and validation times without writing source snapshots or feed
replacements over an active refresh. Such observations have no source cursor.
When the locks are free, the completed issue 64 recovery path still publishes
tracked PR replacements and preserves independent CI/detail errors.

Both paths recheck cached metadata for head/base/merge changes. Cached-only
retries remain cached-only; they cannot unexpectedly validate against GitHub.
Missing reviews and CI keep their explicit errors and incomplete state. A cold
metadata cache yields unavailable JSON with `complete=false`, `available=false`,
`code=cache_miss`, no validation timestamp, and CLI exit 1. This works for `pr
view`, `pr checks`, legacy PR syntax, and `ci`; HTTP retains its 404 status and
adds explicit unavailable metadata. PR view reports expose `observedAtMs` and
retain source validation times/errors even when projecting selected fields.
Observation time describes the read, not a fresh upstream validation.

The existing hey-gh source is not a Git repository. The implementation, canonical
command-card update, and regressions are retained in
`patches/hey-gh/issue72-cached-responsiveness.patch`, based on the existing source
with issue 64 already applied. Apply using:

```sh
tools/apply_hey_gh_issue72.sh /path/to/existing/hey-gh
```

The helper validates all hunks before writing, accepts identical retries, and
rejects changed baselines. It leaves issue 64's patch and completed issue intact.

## Verification

The three new regressions failed on the original implementation: cached reads
waited behind a stalled refresh, partial account authorization failure plus
stalled CI blocked cached reads, and cold-cache CLI commands emitted no JSON.
The fixed tests cover actual SDK, HTTP, and CLI observations, including retained
head identity, old source validation times, no upstream requests, incomplete
reviews/CI, and no publication while refresh locks are held.

All 54 Rust tests passed serially: 43 GitHub integrations, seven repository
integrations, and four unit tests. This includes all issue 64 feed-recovery
regressions. Clippy with warnings denied and Rust formatting passed. The existing
200 ms account-detail timing test failed once during the concurrent suite, then
passed alone and in both serial full-suite runs.

The fix has no web UI. The isolated issue viewer was visually reviewed at
1440×1000 and 390×844, with responsive checks at 390 and 768 pixels, Markdown
comment preview, and keyboard focus traversal. There was no document overflow or
browser console error. Its Chrome session/profile, fixture server, screenshots,
database, and temporary source/build reports were removed after review.

## Installed verification — September 20, 2026

Installed the updated binary and command skill on this MacBook,
`kamils-macbook-pro.local`, and `devbox`. Both Mac binaries have SHA-256
`bd80907f852ee8b7faf253d4a689516507238f5dae907186c17fa08b893fc21e`.
The Linux release has SHA-256
`0bcc8a9f24ecdad3ddae86bd25ad64cd3e2a30fd31f5f4786c525293fe8edeec`;
all five patched source files match this MacBook's tested source. The skills on
all three machines have SHA-256
`c26a02a9efc3d8293263ef06a57fb9ac2fb42233a06280669fc5bc6d51dd71ee`.
Devices without existing hey-gh daemons received the binary/skill without extra
background services. The existing MacBook daemon was gracefully replaced, and
all 68 saved watch registrations survived.

Eighteen installed cached CLI observations (three rounds of PR view/checks for
15127, 15124, and 15033 in `poe-internal/poe2`) returned parseable JSON in
42–1,457 ms while background polling continued. PR 15127 returned complete cached
evidence at head `3aabc6dfc69362e2f7abfcda61d47779b5851629`; this does not assert
freshness or readiness. PR 15124 still reported missing review threads,
`complete=false`, and exit 1 at the explicitly older cached head
`85d3221dd5946c140af7090433f868cf80d79a8b`. PR 15033 retained complete cached
recovery at unchanged head `4bd3fbb4b4a8b4bd20bdf30d9c7ac8ff5068f7dc`.
Every response retained source validation times. No explicit refresh was used.

The required hey-boss Fly rolling deployment reused the current immutable mobile
image because this change only affects the local hey-gh daemon. Deployment smoke
and machine checks, DNS verification, and the public `/healthz` endpoint passed.
