# Fleet liveness under contention

The supervisor separates connection scheduling, fresh local observations, and
maintenance into fixed loops. Worker reconciliation, signal application, journal
pruning, and deployment admission stay in the maintenance loop. A slow collection
does not delay scheduling another companion connection, and slow maintenance does
not prevent a successful fresh collection from becoming visible.
Maintenance copies only worker definitions from the observation, leaving agent
histories in the live snapshot.

The local heartbeat advances only after worker collection succeeds. Failed
observations leave the last known snapshot and its timestamp intact. Companion
heartbeat deadlines remain fifteen seconds; no timeout, claim, allocation deadline,
or worker concurrency limit is increased.

Machine updates first change the in-memory snapshot under the state mutex. A
separate mutex serializes durable machine saves; another update never waits for
that mutex. Updates arriving during a save remain dirty for the next save. A failed
save restores the dirty bit. Maintenance retries dirty snapshots even when no new
substantive update arrives. Pure heartbeat and sync timestamps never initiate disk
writes. The saved snapshot remains available for offline inspection after restart.

Signals and worker configuration changes share a configuration mutex, including
desired-file edits in maintenance. They do not hold the liveness mutex during
SQLite work. Journal mutation transactions, allocation checks, configuration
revision checks, and receipt publication retain their existing ordering.

## Regression evidence

The maintained supervisor tests hold an actual SQLite `BEGIN IMMEDIATE` writer
while a machine save or queued signal waits. Before the fix, unrelated peer
heartbeat/startup updates and fresh local observations fail the progress deadline.
After the fix, they progress before the writer is released. After release, the
tests verify the queued signal and all dirty machine changes, including changes
that arrived while an older snapshot was being written.

Additional tests verify that connection scheduling does not acquire collection or
maintenance locks, failed collection does not refresh heartbeat age, unchanged
machine fields do not acquire a writer lock, and failed saves retry.

These tests reproduce concrete blocking mechanisms in the owning supervisor
paths. They do not identify the individual writer or prove that either mechanism
alone caused the production stall observed on 23 September 2026. The recorded
production sample had stripped symbols and cannot establish that attribution.
