# Dependency notices during rolling upgrades

Installing a new executable does not replace an active worker process. Older
workers can keep their sessions and use the shared SQLite service, but their
compiled scheduler still applies the old sibling-order rule. Before this guard,
their derived comments and steering could name former siblings even while the
current CLI correctly displayed explicit scheduling and no blockers. Local
comment provenance distinguishes this path from incoming fleet history.

The store now admits generated dependency notices against the saved scheduling
policy. In explicit mode, every named prerequisite must be a declared blocker or
an attached descendant required for parent completion. The guard uses SQLite
built-ins so it also applies to older clients. It covers the comment, both event
records, and pending agent steering. A rejected comment cannot leave a misleading
event pointing at an unrelated previous insert. Fleet replay acknowledges a
suppressed append without inventing a comment-ID mapping.

Legacy reconcilers also continue from their notice to an automatic state write.
The store rejects a transition to Blocked in explicit mode when no declared
prerequisite or descendant needs work, and suppresses audit events that cite
obsolete siblings. This preserves a completed Ready handoff, its version, and
any live worker reservation. Real dependency blocking and manual holds remain
effective. The guard checks only reachable dependencies, including prerequisites
of Ready tasks; it does not scan unrelated issues or change existing history.

`issue allocation` and issue-view JSON now include `worker_reservation` separately
from fleet allocation. The summary identifies its run, actor, machine, and claim
deadline, so an unallocated fleet issue does not imply that a worker's pending
claim can be bypassed. Finished attempts disappear from this readout. An expired
deadline does not itself end an attempt or authorize releasing its ownership.

A notice mixing real dependencies with obsolete siblings is suppressed as a
whole: delivering that stack would give the worker incorrect instructions. Its
deduplication event is also suppressed, allowing the current scheduler to issue
the correct notice for real dependencies on its next reconciliation. Sequential
projects, declared-dependency notices, ordinary comments, and delivered history
are preserved. Switching to explicit mode rejects obsolete queued notices with
an explanation; it does not change claims, reservations, hierarchy, or processes.

Regression coverage executes the old client's SQL write sequence, exercises
queued steering across a mode switch, and checks fleet replay and current Ready
handoff. The browser walkthrough uses disposable projects and verifies desktop
and phone layouts, settings persistence, parent grouping, dependency notices,
and claim-conflict handling in both themes.
