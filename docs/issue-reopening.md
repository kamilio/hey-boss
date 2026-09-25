# Reopening issues with unfinished dependencies

A closed issue can reopen while linked issues or subtasks are unfinished. Reopening
clears closure metadata and retains dependency links. The issue waits for those
dependencies and remains ineligible for pickup until they finish. Reopening an
already blocked issue cannot bypass its dependencies. Version checks, retry IDs,
and protection of running claims still apply.

The detail action stays enabled on closed issues. Successful reopening reports
“Issue reopened; waiting for dependencies” when appropriate, and the existing
waiting notice explains why workers will not pick up the issue.

## Verification

On September 25, 2026, the browser regression reproduced the disabled action on
the old binary. Build `1fd82d4a2a05d238` (commit `6b3b8c7`) passed:

- Nine Rust explicit-dependency tests, including linked and subtask blockers,
  closure metadata, stale versions, request-ID retries, pickup refusal, and claims.
- The JavaScript action/classification regression and five mobile issue/bridge tests.
- 33 browser checks on each of the native and paired surfaces: keyboard reopening,
  failure recovery, the displayed version guard, retained dependencies and unsent
  comments, waiting feedback, reload persistence, and automatic readiness.
- Light/dark layouts at widths of 1440, 768, 390, and 320 pixels. Screenshots were
  visually reviewed; waiting notices fit without horizontal overflow.

Run `cargo test --test explicit_dependencies` and
`node tools/test_issue_blocked_types.cjs` for the focused regressions. Start
`node tools/serve_issue_reopen_fixture.mjs [binary]` for isolated native (`59661`)
and paired (`52061`) servers, then run `tools/issues_reopen_browser_checks.js`
through Playwright CLI on each origin. Stop the fixture with SIGINT/SIGTERM to
remove its temporary database and stop its child processes. Remove generated
`output/playwright/issue361` screenshots after review.

Production poe-code issue #1909 is intentionally cancelled and must remain closed;
all browser mutations use synthetic fixture databases.
