# Desktop action protocol

Actions use the existing authenticated SSH/private Unix-socket request/reply
channel. They execute immediately and are never placed in the notification
queue. Offline callers receive a nonzero exit and a clear “Desktop companion is not
connected” error; reconnect does not replay actions. Automatic opener selection
propagates connection failures instead of falling back to printing a URL. Only
an absent hey-boss executable permits the normal browser/print-URL fallback.

`hey-boss action METHOD --params JSON --request-id ID` sends:

```json
{"version":1,"id":"my-request","method":"browser.open","params":{"url":"http://127.0.0.1:4123/review/token"}}
```

The outer request has `command: action`, `sync: false` and the serialized
envelope in `question`. The broker stamps its SSH host alias and connection
generation. The outer reply has status `ok` or `error`; its `result` string is a
JSON envelope with the same version/id and either `result` or `error`.

## Methods

- `capabilities`: discover supported methods and protocol version.
- `browser.open`: HTTP(S) URL only, no embedded credentials. Returns `session`,
  the browser-facing `url`, original `source_url`, and `opened`. Remote loopback
  URLs forward to remote IPv4 loopback using the authenticated SSH master;
  paths and query strings are preserved.
- `browser.request`: session plus origin-relative `path` (default `/`), HTTP
  `method` (default GET), optional string `body` and string-valued `headers`.
  Supports GET, POST, PUT, PATCH, DELETE, HEAD and OPTIONS. Returns HTTP status,
  headers and UTF-8 body, or a base64 body for binary responses. Redirects are
  returned without following them. Non-2xx responses are normal HTTP results.
- `browser.close`: releases the session and its forward. It does not close an
  external browser tab.

Requests are HTTP calls to the selected origin, not DOM automation. They do not
share the external browser’s cookies or signed-in profile. Websites can send
requests back to their server through the forwarded endpoint in the usual way;
there is no arbitrary remote shell or JavaScript execution API.

## Bounds and failure behavior

Maximum 32 website sessions, 16 concurrent actions, 32 cached successful
responses, 256 KiB envelope, 8 KiB URL/path, 128 KiB request/response body,
32 headers. Request timeout is 5 seconds, resource timeout 6 seconds; SSH mux
operations have a 3-second deadline and do not start new SSH logins.

Sessions are owned by their originating host and connection generation. After
reconnect or daemon restart, open a new session. Stale sessions cannot be used
against a newly allocated tunnel. Opening sessions reserve capacity while the
browser launches. Failed browser launches release their forward.

An identical request ID/payload can reuse a cached successful result within the
same connection and daemon lifetime; conflicting reuse is rejected. This is a
bounded in-memory convenience, not durable exactly-once delivery. A timeout may
occur after a website accepted a POST. Use application-level idempotency keys
for mutations whose retries could cause duplicate effects. Failures are not
cached. Unsupported methods and versions return errors, with no side effects.
