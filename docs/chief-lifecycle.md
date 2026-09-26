# Chief exit and event delivery

Chief process exit and reader-thread completion are separate observations. When
the receive timeout fires, a process may already have exited while its final
JSONL events are still buffered or waiting for the reader thread to run.

The scheduler checks the output pipe for hangup after observing process exit.
A closed writer lets the existing reader drain events and deliver EOF. A writer
still held by a descendant remains an abandoned-stream error. The inspection
uses its own read descriptor so reader completion cannot recycle the inspected
file descriptor. It adds no sleep, grace period, or test deadline increase.

Issue #365 reproduced on Linux during missing-thread replacement: the third
diagnostic lifecycle run became blocked with `Chief exited before closing its
event stream` despite the replacement process producing valid events. The
deterministic regression leaves a completed event in the pipe until after exit;
the old exit-only check fails it. A companion test preserves detection of a dead
parent whose descendant retains stdout.

Run `cargo test --locked --lib chief` and `cargo test --locked --test issues_chief`.
The integration test covers initial launch, resume, missing-thread replacement,
failed/malformed completion, inherited stdout, ownership, and disabling Chief.
Its 15-second waits identify the stage, persisted Chief state, worker exit, and
bounded log tails. Shutdown has a five-second graceful limit and cleans up the
fixture's owned Chief process group before a forced worker stop.
