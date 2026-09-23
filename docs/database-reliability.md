# Issue database reliability

The September 22 investigation confirmed a locking defect in `Store::open`.
It opened and closed a raw file descriptor for the live database before opening
SQLite. POSIX closing any descriptor for an inode releases that process's
record locks on the inode, including locks held by other SQLite connections.
Concurrent opens in supervisor threads could therefore remove protection from
an existing connection. Commit `9a31312` removed this operation.

Existing database files are now opened only through SQLite. A missing database
is published from a closed, private staging inode using a non-replacing hard
link. The store rejects nonregular database paths and opens the canonical file
without SQLite's create flag. `tests/database_locks.rs` verifies lock retention
with a separate process using `fcntl(F_SETLK)` and a journal-mode change; it also
checks concurrent creation and rejected paths.

The same regression also reproduced lock loss when a plan file was a hard link
to the live database. Store-owned plan reads and final reconciliation now check
inode identity before opening the plan or its sync lock. Plan, sync-lock and
sync-paused aliases to the active database, WAL or shared-memory file are
rejected. Separate-process tests cover hard links and symlinks and verify that
ordinary Markdown reads still work.

A terminal-workflow regression additionally reproduced direct database header
overwrite: resuming a saved plan wrote `Startup reconciliation pending` through
a pause-marker hard link to the database. The CLI now performs the same inode
check before startup locks, marker writes, reconciliation writes and background
sync. The regression verifies that the SQLite header and draft contents survive
rejection and that no coding agent launches. The demonstrated overwrite contains
text and does not establish the writer behind the historical zeroed header.
These checks protect existing aliases; the fleet still assumes database files
are not replaced or relinked during an operation.

Two historical failures must be distinguished. The first database had a zeroed
4,096-byte header and was recovered by restoring its schema catalog. The exact
writer responsible for that damage has not been established. The subsequent
failure was extended SQLite code 522, `SQLITE_IOERR_SHORT_READ`, with an intact
main database, an empty WAL and stale shared-memory state. The confirmed lock
defect is fixed; those historical observations do not prove every earlier
failure came from it. SQLite errors now retain primary and extended numeric
codes to support future diagnosis.

The macOS production path is
`~/Library/Application Support/hey-boss/issues.db`. Database and WAL/SHM files
must not be replaced, renamed or unlinked while any process holds the database.
Recovery preserves the failed files and checks holders before changing files.
SQLite's online backup API makes consistent inspection copies; copying the
main file alone while WAL transactions are active does not.

## Size and performance

The initial approximately 349 MiB database was mostly synchronization history:
the outgoing journal used 222 MiB, saved idempotent request responses 44 MiB,
worker activity 18 MiB and worker runs 16 MiB. Issues themselves used 6 MiB.
Full before/after issue bodies accounted for about 125 MiB of the journal.

The supervisor retains the latest 10,000 canonical journal entries and removes
at most 1,000 per maintenance pass. A durable floor makes old cursors recover
through a verified snapshot. Companion outgoing changes, replay receipts and
conflict evidence remain durable. Deletion releases SQLite pages for reuse;
the file's apparent size need not shrink. A disposable backup shrank to about
153 MiB after retention and SQLite compaction, while its canonical snapshot,
cursor high watermark and replay identities remained identical. This is a
measurement on a backup, not a report of live production compaction.

At the September 23 04:34 UTC checkpoint, a fresh online backup and a separate
disposable proof copy were retained. The proof preserved the full canonical
snapshot, journal high watermark and replay identities across compaction.
Production then completed SQLite `VACUUM` with the same database inode, shrinking
from 402,132,992 to 186,552,320 bytes (about 384 to 178 MiB). Integrity and
foreign-key checks passed afterward, and all three machines remained connected.
No live database or sidecar file was replaced manually. Continued writes can
change these sizes after the checkpoint.

A later storage check found 2,834 saved requests occupying about 51 MiB: their
payloads contained 14.2 MB and their original responses 36.7 MB of text. These
records preserve replay identity and return the original result after later edits.
They remain intact. Cache hits now use a coherent read snapshot, so completed
request retries no longer wait for an unrelated writer. Cache misses still
recheck under the mutation lock to keep concurrent retries from executing twice.

Heartbeat timestamps no longer rewrite large machine snapshots. Idle journal
maintenance remains a reader. Machine activity now comes from one coherent WAL
snapshot and reads the worker overview once. On a September 23 private copy
containing 34 workers, the median poll fell from 1.63 seconds to 68 milliseconds
with identical activity results. Run history and recent events remain bounded
and indexed; these improvements preserve their existing contents and ordering.

Equivalent worker project/tag filters now share queue counts within that poll's
snapshot. A regression with 30 workers and 2,000 issues reduced SQLite VM work
from 8.36 million to under 816,000 steps, while matching public status and
refreshing counts after later issue changes. Supervisor and companion build
reports identify their loaded code rather than a replacement executable on disk.
Startup connection errors also retain the bounded SSH error tail, so a transport
failure before the first hello remains distinguishable from an application error.

A fresh September 23 backup with 34 workers measured the current coherent poll
at a 32 ms median, versus 189 ms for repeated current public-status calls, with
identical activity results. These are separate measurements from the earlier
checkpoint. Chief status also uses a worker lookup index: selecting three visible
chiefs beside 9,996 unrelated records fell from 110,138 to 181 SQLite VM steps,
preserving running-first ordering, hidden-project filtering and lifecycle fields.

Explicit project queue filters use the existing project index. Counting 20
selected issues beside 10,000 unrelated open issues fell from 123,800 to fewer
than 3,800 SQLite VM steps. Duplicate project IDs do not double counts; empty
project filters retain unrestricted status behavior.

Worker pickup reads an ordered prefix from each selected project, then merges
the prefixes in global issue order while retaining only the requested batch.
A partial index covers those project queues. Selecting three issues from small
projects beside 10,000 unrelated issues fell from 123,300 to fewer than 2,300
SQLite VM steps. Selecting three early ready issues in a 10,000-issue project
stays below 1,400 steps.

Workers without required tags skip JSON label-membership subqueries. Queue
aggregates also avoid rechecking open, live and visible-project conditions that
their outer query already guarantees. A 10,000-issue unrestricted queue count
fell from about 1.72 million to fewer than 1.40 million SQLite VM steps. Assigned,
draft, closed, deleted and hidden-project behavior, tagged filtering and pickup
order remain unchanged. The fresh 34-worker backup had no tagged workers.

## Ongoing verification

The eight-hour September 23 reliability audit uses a private database with four
writer processes, a reader and periodic quick-check/foreign-key checks. An
isolated supervisor adds concurrent same-process store opens and journal
maintenance, with no workers or SSH inventory. These soaks remain in progress
until their scheduled deadline; intermediate clean checks are not final soak
results. Production integrity and fleet convergence are checked separately.
