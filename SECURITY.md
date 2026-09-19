hey-boss is a local desktop tool. The socket and installation configuration are owner-only; the state directory is private to the user. Processes running as the same user can submit notifications and questions. It is not a security boundary between local applications.

History includes message contents and launcher metadata. Do not attach your database or logs to public issues. For a security report, contact the repository owner privately and share only a minimal reproduction with synthetic data.

The remote companion uses SSH authentication and owner-private Unix sockets.
Server queue files contain notification text, questions, and remote origin metadata.
Processes with access to the server account can submit items and read queued data;
only connect servers you trust. No arbitrary command execution protocol is exposed.

The issue web listener remains loopback-only. Opt-in `--mobile-origin` allows
one exact Tailscale HTTPS origin for a private Serve proxy; it adds no application
login. Tailnet grants/ACLs must limit the endpoint to trusted personal devices.
Allowed devices can read all issue projects, act as Boss, and control workers.
Never expose it through Funnel or a public proxy. The authoritative database is
not copied to a mobile hub or phone. HTTPS issue pages do not persist drafts or
retry payloads in browser storage, and HTTP responses use `no-store`; displayed
contents still reach device memory and can be captured by the browser or OS.

Desktop actions use the authenticated companion channel and execute immediately;
they are never queued offline. Browser sessions are host- and connection-bound,
HTTP(S)-only and origin-bound. SSH browser forwards listen on IPv4 loopback only.
HTTP response bodies and concurrency are bounded; redirects are not followed by
the request API. The API does not import browser cookies, run shell commands or
execute arbitrary browser scripts. See docs/browser-actions.md for retry limits.

Secret requests have a dedicated synchronous path: the desktop keeps only a live
prompt and socket reply, and the companion never persists/replays the request or
response. They bypass Record, SQLite, mobile sync, action caching, and normal task
status/wait. Closing/disconnecting or timing out cancels entry. Error messages do
not interpolate secret responses. Secure fields are masked, reveal undo is off,
and values are cleared from controls on completion. Screenshot sharing is disabled
for the secret window; this is not protection from same-user accessibility tools.

The CLI requires an explicit sink. Stdout accepts only an empty, owner-private
regular file, never a terminal or pipe. File writes reject symlinks/hard links,
nonprivate and tracked destinations, and conflicting existing keys; temporary
files are mode 600. Environment injection uses process environment rather than
argv; child stdin/stdout/stderr go to /dev/null. Child code may still write its own
logs or files. These controls prevent ordinary agent transcript/history exposure,
not access by same-user processes or OS memory/swap inspection. Swift/JSON may
create temporary in-memory copies; no complete memory-erasure claim is made.
