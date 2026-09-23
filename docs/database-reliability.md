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

Heartbeat timestamps no longer rewrite large machine snapshots. Idle journal
maintenance remains a reader. Machine activity now comes from one coherent WAL
snapshot and reads the worker overview once. On a September 23 private copy
containing 34 workers, the median poll fell from 1.63 seconds to 68 milliseconds
with identical activity results. Run history and recent events remain bounded
and indexed; these improvements preserve their existing contents and ordering.

## Ongoing verification

The eight-hour September 23 reliability audit uses a private database with four
writer processes, a reader and periodic integrity/foreign-key checks. An
isolated supervisor adds concurrent same-process store opens and journal
maintenance, with no workers or SSH inventory. These soaks remain in progress
until their scheduled deadline; intermediate clean checks are not final soak
results. Production integrity and fleet convergence are checked separately.
