# Database owner recovery

The database service serializes writes behind one elected owner. Its Unix socket,
owner lock, and startup lock live in `/tmp/hey-boss-db-UID`. These are live service
state, not disposable cache files, even when their modification times are old.
Harvester must preserve the directory and any saved cleanup cursor inside it.

An unlinked `flock` file remains locked through its open descriptor. Recreating
its pathname produces an independent lock. Therefore election must also refuse
to replace a listening socket, and verify that its lock descriptor still names
the current inode. A displaced owner drains its transactions before releasing
the listener; shutdown may only unlink the socket that owner actually bound.

For a suspected split owner, compare the lock descriptors and pathname inode
with `lsof` and `stat`; `lsof +L1` identifies an unlinked lock. Multiple database
descriptors alone do not establish split ownership. Measure actual web API calls
and check the owner lock before attributing a slow response to SQLite.

On 2026-09-28, hey-boss #685 reproduced this failure locally: the old service held
unlinked lock inode 602054868 while the supervisor held replacement inode
623469970 at the same pathname. Graceful service retirement restored a single
owner while all five poe2 worker processes remained running. No lock files or
database contents were manually removed or edited during recovery.
