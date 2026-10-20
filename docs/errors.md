# Error inbox

| Part | First version |
| --- | --- |
| Ingest | Generic JSON batches through `hey-boss errors` or same-origin `/api/errors` |
| Collection | Separate Poe adapter, polling historical edge/store error logs every 30 seconds |
| Grouping | Project + source + stable fingerprint; original samples remain available |
| Replay | Same project/source/event ID counts once, including overlapping backfills |
| Inbox | Most observed occurrences in the last 24 hours; previous day and lifetime context |
| Tasks | Create task from a row or its evidence dialog; editable prompt; retries reuse one task and frozen evidence |
| Retention | SQLite samples: 30 days; JSONL: four files up to 8 MiB each |
| Enablement | Off by default; explicitly enabled for `named:poe2-legacy` |

Collection and inbox data are local to the collecting machine. Created tasks use the existing task store and fleet sync. The mobile companion does not expose the local inbox in this version. Counts describe collected observations, not unique failed requests; upstream sampling can omit events. Browser symptoms and server causes retain request references but are not automatically merged.

## Generic ingest

Producers send sanitized diagnostics, never headers, credentials, chat text or full request bodies. `fingerprint` is a stable code/operation/route, with volatile IDs and release-specific line numbers removed. It is optional; the inbox otherwise normalizes the first message line. Release and request reference are evidence, not grouping keys.

```sh
hey-boss errors --project named:example enable true
printf '%s' '[{"source":"worker","event_id":"upstream-123","timestamp":1792512000000,"message":"Upload failed","fingerprint":"upload:storage-scope","count":1}]' \
  | hey-boss errors --project named:example ingest
hey-boss errors --project named:example list
```

Use the current Unix millisecond timestamp. Maximum 500 events / 16 MiB per atomic batch; message 8 KiB, stack 16 KiB. Optional fields: `stack`, `release`, `request_ref`, `fingerprint`; `count` defaults to 1 and preserves already-coalesced browser reports. More than 1,000,000 retained events or 10,000 groups refuses the batch rather than silently dropping evidence. Logs sit beside the issue database as `issues.errors.jsonl[.1-.3]`; SQLite uses `issues.errors.db` and reuses freed pages. Journal lines can repeat after a crash; use event IDs when reading them.

The HTTP form is `{ "action": "ingest", "project": "named:example", "events": [...] }`. It uses the existing same-origin and CSRF boundary. Other actions: `list`, `detail` (`id`), `enable` (`enabled`), `promote` (`id`, optional `prompt`). The default task prompt lives in `src/issues/prompts/error_task.md` and can be overridden under Project settings → Error task; `{{command}}` expands to the error lookup command. The evidence dialog lets you edit the prompt for one task. Untrusted diagnostic text is rendered as text, never HTML.

## Poe adapter

Run from the Poe checkout so Bun loads its existing Cloudflare credentials:

```sh
bun /path/to/hey-boss/tools/poe-errors.ts          # one pass
bun /path/to/hey-boss/tools/poe-errors.ts --watch  # continuous
```

The adapter queries the exact legacy preview edge and store workers in the development account. It maps accepted browser reports as well as Worker errors; browser `count` is retained. It only keeps diagnostic fields, redacts common credentials/email/query strings, and does not copy user IDs or arbitrary payload fields. Generic ingestion still relies on producers to sanitize their diagnostics.

The first pass reads the last hour. Later passes overlap the durable checkpoint by two minutes, with a 30-second settling delay. Five-minute queries split when capped; a capped one-second query fails and keeps the checkpoint. A checkpoint advances only after both workers have been ingested. If disabled, no upstream query runs. Sampling still applies even to completed queries.

For a persistent Mac supervisor service, run `bash tools/install-poe-errors.sh /path/to/poe2`. It copies the adapter into the application support directory and installs one LaunchAgent; it contains no credentials.

State is in `poe-error-collector/` beside the issue database. A process lock prevents duplicate collectors. Interrupted processes recover automatically after their PID disappears. A checkpoint older than retention requires an explicit reset/backfill decision; stop the collector and remove its checkpoint to restart from the last hour. `HEY_BOSS_BIN` selects a test CLI; `POE_REPO` selects the Poe checkout. Credentials remain in that checkout's environment.

Grouping rules live in `tools/poe-errors-core.ts`. Start by inspecting real samples and changing that mapping; there is no separate merge/split editor or automatic task generator.
