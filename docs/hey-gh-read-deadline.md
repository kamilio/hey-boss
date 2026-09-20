# hey-gh caller read deadlines (issue 73)

Single-PR CLI reads accept an opt-in total deadline:

```sh
hey-gh pr view 15077 -R poe-internal/poe2 --timeout 15
hey-gh pr checks 15127 -R poe-internal/poe2 --timeout 15
hey-gh required-checks poe-internal/poe2 15127 --timeout 15
hey-gh ci poe-internal/poe2 15127 --timeout 15
```

`--timeout SECONDS` accepts 1..3600. The timer starts before command execution
and covers local repository discovery, branch-to-PR resolution, cache transport,
and waiting for upstream evidence. It applies to view/checks, legacy single-PR
syntax, CI, and required-check policy. It rejects lists, feeds, cursor arguments,
and long polling; `--wait` retains its existing cursor-read meaning. Commands
without a timeout keep their existing behavior and output.

PR and CI reads capture cached evidence before their ordinary read. Successful
reads retain the existing JSON, field projections, and exit status. Expiry emits
parseable JSON with exit 1, `code=deadline`, `deadlineExceeded=true`,
`complete=false`, explicit `pendingSources`, and a deadline source error.
Available cache evidence retains its original head identity, validation times,
observation time, and independent review/CI errors, including in `--json`
projections. `available=true` describes evidence, not current validation or merge
readiness. There is no fallback request after the deadline.

Cold cache, unfinished selector resolution, or required-check policy expiry
returns structured unavailable evidence with `available=false`, `state=unknown`,
empty validations, and no asserted validation time. Policy reports do not expose
source validation times and cached policy reads can wait on refresh locks;
returning unavailable avoids inventing policy freshness or satisfaction.

The deadline belongs to the CLI. Expiry drops its HTTP wait, while the existing
daemon handler and shared scheduler continue. It does not restart the daemon,
cancel background work, remove watches, or change saved feed cursors. Fallback
JSON has `cursor=null`, so consumers preserve their previous incremental cursor.
No observation or recovery replacement is published by the deadline fallback.
The preliminary cached read uses the existing issue 72 behavior; cached-only
reads remain prompt during refresh contention and make no GitHub requests.

## Source delivery

The existing hey-gh source has no Git repository. This checkout retains the
implementation, canonical skill update, and regressions in
`patches/hey-gh/issue73-read-deadline.patch`, based on the existing source with
issues 64 and 72 applied. Apply it with:

```sh
tools/apply_hey_gh_issue73.sh /path/to/existing/hey-gh
```

The helper validates every hunk before writing, accepts identical retries, and
rejects incompatible baselines. Existing issue 64 and 72 fixes remain intact.

## Verification

The two initial regressions failed against the original implementation because
`--timeout` was unsupported and produced no parseable deadline result. The final
three integration regressions cover stale and cold caches, ordinary and explicit
refresh reads, PR checks/view/legacy syntax, CI, required-check policy, projected
fields, stalled branch lookup, and a 60-second GitHub secondary-limit backoff.
They check original validation times, incomplete deadline evidence, no fallback
cursor, continued shared requests, eventual lock release, valid saved cursors,
fast successful reads, cached-only responsiveness under contention, timeout
bounds, incompatible feed arguments, and CLI help discoverability.

All 57 Rust tests passed serially: 46 GitHub integrations, seven repository
integrations, and four unit tests. This includes all issue 64 and 72 regressions.
Clippy with warnings denied and Rust formatting checks passed.

The change has no web interface. The existing isolated issue viewer was visually
reviewed at 1440×1000, 390×844, and 768×1024, including wrapped command text,
Markdown comment preview, and visible keyboard focus. No document overflow,
browser warnings, or console errors occurred. Its Chrome session, fixture
server/database, screenshots, and temporary reports were removed after review.

## Installed verification — September 20, 2026

The locked-dependency macOS release and updated canonical command skill were
installed on this MacBook and `kamils-macbook-pro.local`; devbox built and
installed its Linux release from the same baseline-verified patch. The existing
local daemon was left running because its API and background work are unchanged.
No new daemon was started on connected devices.

Both Mac binaries have SHA-256
`88c3b55afc29cabc897c20e832f622f6a8b87295202a76aa7f2c2fd88a9d4fc1`.
Devbox's Linux release has SHA-256
`3fa4159139c6ce73c5aa54ab67621011a9161269d5b416cbbcbadb7691f32951`.
All installed command skills have SHA-256
`7abdd73d53487b56447d804c1d9a42b5a2e1a31cb1ee02329991c8e704255189`.
Devbox's deadline module matches the tested MacBook source, SHA-256
`9e6d1793e4277db10a84b199f6a95d5d0f172bb35e1c1ad5c6e1d77447e02aff`.

Five installed ordinary reads against the live MacBook daemon (`pr view` for
15077, 15096, and 15124; checks for 15127; required-check policy for 15127 in
`poe-internal/poe2`) returned parseable deadline JSON in 2,015–2,035 ms with
`--timeout 2`. PR and CI evidence retained original head identities and validation
times; PR 15096 retained its explicit missing CI sources. Required-check policy
returned unavailable/unknown instead of asserting satisfaction. A bounded
cached-only PR 15124 read returned in 61 ms. All 68 saved watch IDs were unchanged.
These observations do not assert current validation or merge readiness.

The required Fly rolling deployment reused the current immutable mobile image,
`sha256:5c411f4a66b6ffdd32d8f1fb06c328aca03f9353ba51d6d14f5a1ff7cc32763f`,
because this change affects only the local hey-gh CLI. Deployment smoke and
machine checks, DNS verification, and the public `/healthz` endpoint passed.

Both connected installed binaries also passed three runtime checks through a
synthetic stalled loopback API forwarded over temporary SSH connections. Bounded
CI reads retained the deliberately older validation time and independent missing
check-run error; cold PR views and required-check lookups returned structured
unavailable/unknown evidence. Every command returned exit 1 and deadline JSON.
The temporary API and SSH connections were closed after the checks.
