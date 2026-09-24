# Worker outage recovery

Database transport disconnects and model-proxy recovery-budget exhaustion are
infrastructure failures. An unfinished attempt retains its session, checkout,
history, and the specific outage explanation. It releases its claim and worker
slot, then retries after a persisted exponential delay: 30, 60, 120, 240, then
300 seconds. There is no five-attempt cutoff. An explicit reopen permits an
immediate resumed attempt and resets the delay; if the service is still down,
the new attempt reports that outage and schedules another retry.

A database disconnect does not authorize replaying an unknown mutation. Worker
finalization first saves the result durably, then reconciles the run's committed
state on a fresh connection. The transaction checks whether the run already
finished before applying a handoff. A lost reply after COMMIT therefore produces
one handoff, while a rolled-back transaction can safely finish after recovery.
Resumed-agent instructions require a read/reconciliation or the original
deduplication request ID before repeating an uncertain write.

Policy denials and requests for human approval remain distinct from service
outages. Successful recovery within the same turn does not override a verified
completion. Ordinary code/test failures keep their own outcome; an infrastructure
retry never grants permissions or establishes that verification succeeded.

The Agents page shows pending retries above collapsed history, with their timing
and saved conversations. Database failures explain how to handle uncertain
writes. Ended attempts have no steering action. New attempts supersede the old
retry card while preserving history. Long project names wrap without hiding the
issue-navigation link on narrow screens.

## Regression coverage

- `worker_infrastructure` tests distinguish captured database/proxy failures
  from application errors, successful tool output, and policy denials.
- `worker_store` tests disconnect before and after COMMIT and require exactly
  one handoff; captured outages retain the session and schedule retries.
- `issues_worker` tests automatic recovery, explicit reopening during outages,
  capacity release, same-turn recovery, and six alternating database/proxy
  failures across scheduler restarts followed by successful continuation.
- `tools/infrastructure_hold_browser_checks.js` runs on an isolated Agents
  fixture through Playwright CLI. It supplies synthetic fleet/conversation
  responses and requires 101 completed assertions across light/dark themes at
  1440, 390, and 320 pixels, plus long project names at 360 pixels and keyboard access.
  Its screenshots belong in `output/playwright/issue138-delivery/` and should
  be removed after review.

Use the Cargo and native audit commands in README.md for the required project
checks. An interrupted command or missing target summary is incomplete even
when its process reports exit zero.
