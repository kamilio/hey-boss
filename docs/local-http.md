# Local HTTP and hey-boss.test

The normal `hey-boss issue web` service serves the same application, CSRF token,
and issue database on IPv4 loopback ports 4781 and 80. It never redirects URLs:
paths, queries, and browser fragments stay intact. The old address remains
`http://127.0.0.1:4781/`; the short address is `http://hey-boss.test/`.

For example:

```text
http://hey-boss.test/#project=github.com%2Fpoe-platform%2Fpoe-code&view=issues&inbox_state=unread&state=open&owner=all
```

An explicit custom port (including `--port 0`) starts only that listener. The
exact local hostnames `127.0.0.1`, `localhost`, and `hey-boss.test` are accepted
only on the service's actual ports. HTTP port 80 may be omitted or explicit.
Origins must match the request's hostname, scheme, and normalized port. Knowing
another allowed hostname does not permit cross-origin writes. Writes still need
the bootstrap CSRF token and JSON content type. Forwarded headers do not grant
access. `--mobile-origin https://machine.tailnet.ts.net` retains the existing
explicit private Tailscale proxy policy; `.test` is local to each machine.

Browsers treat plain HTTP on `.test` as an insecure context even though it
resolves to loopback. Shared UI helpers therefore use `getRandomValues` for
request IDs and a SHA-256 fallback for retry keys when `randomUUID` and
SubtleCrypto are unavailable. No browser security flags or HTTPS exceptions are
needed. Secure contexts keep using native Web Crypto.

## macOS setup

macOS may refuse an unprivileged bind to port 80. In that case Hey Boss reports
the error and keeps port 4781 working. Do not run the web service with `sudo`.
The optional setup uses a launchd system job with `UserName` set to the invoking
user. launchd opens only `127.0.0.1:80` and passes the socket to the unprivileged
Hey Boss process, which also opens 4781. There is no reverse proxy or root-run
application/database service.

After installing the current CLI, compile the small setup utility:

```sh
rustc --edition 2024 src/issues/web/setup_local_http.rs -o /tmp/hey-boss-local-http-setup
sudo /tmp/hey-boss-local-http-setup install "$(command -v hey-boss)"
rm /tmp/hey-boss-local-http-setup
```

The setup preserves `/etc/hosts`, adding `127.0.0.1 hey-boss.test` only when
needed, and refuses a conflicting existing alias. It installs
`/Library/LaunchDaemons/local.hey-boss.web.plist`. Existing listeners are not
killed. If another Hey Boss web service owns 4781, stop that specific service
normally, then start the installed job:

```sh
sudo launchctl bootstrap system /Library/LaunchDaemons/local.hey-boss.web.plist
```

Use the installed executable path, not a development build or a versioned
Homebrew Cellar path. The job uses that installation's state file and the user's
home directory. If private Tailscale access is configured, pass the same
`--mobile-origin https://machine.tailnet.ts.net` to the setup utility. Keep
Tailscale Serve forwarding to 4781.

The job starts at boot, runs as the selected user, and restarts after executable
upgrades. A socket-activated service exits normally on upgrade so launchd can
return a fresh socket to the replacement process. Foreground services release
both sockets before executing the replacement binary.

## Verification and troubleshooting

```sh
curl --fail http://127.0.0.1:4781/api/bootstrap
curl --fail http://hey-boss.test/api/bootstrap
lsof -nP -iTCP:80 -iTCP:4781 -sTCP:LISTEN
sudo launchctl print system/local.hey-boss.web
```

Both bootstrap responses must identify the same project and CSRF token. Open
both URLs in a browser, create a disposable issue at one address, and verify it
at the other. Check the deep link above and the configured Tailscale URL.
An alias lookup failure means the local hosts entry is missing; it is not a
reason to change the application's allowed hosts. A permission-denied warning
means the process has no activated socket and cannot bind 80 itself. An
address-in-use warning means another service owns the port. Inspect that owner;
do not kill it or add a broad wildcard listener. The old 4781 URL remains usable
when the optional port-80 listener cannot start.

The launchd job writes diagnostics to `/var/log/hey-boss-web.log`. It can serve
only while the user installation and state are accessible (for example, after
FileVault unlock). On another OS, an unprivileged port-80 bind may work directly;
otherwise use that OS's socket-activation/service provisioning rather than
running Hey Boss as root. Custom ports remain available everywhere.

## Rollback

Compile the same utility and run `sudo /tmp/hey-boss-local-http-setup uninstall`.
It unloads only this job and removes only its own marked hosts entry and plist;
pre-existing alias entries remain. Then start `hey-boss issue web` as the normal
user (or open Issues in the menu-bar app). Port 4781 and all stored issues remain
available. Remove the temporary setup executable afterward.
