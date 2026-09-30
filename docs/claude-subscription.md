# Claude subscription

hey-proxy owns an independent Claude OAuth login, encrypted token storage, and automatic refresh. Claude Code only receives the proxy base URL and a local token. No Claude Code credential files or Keychain entries are read or changed.

## Setup

Add `claude` to `providers` in `~/.hey-proxy/config.json` (keep any existing providers):

```json
"providers": {
  "claude": {}
}
```

Or use the complete [Claude-only config](../examples/claude.config.json).

Sign in once on the machine running the proxy:

```sh
hey-proxy claude-login
hey-proxy
```

The login opens Claude's authorization page, uses PKCE and state validation, and receives the result on `http://localhost:54545/callback`. The callback listener closes after authorization or a five-minute timeout. `--no-browser` prints the link without opening it. For a headless host, forward port 54545 over SSH before opening the link locally. Finish the authorization in your browser; never paste tokens into chat or a command argument.

Start Claude Code with exactly two overrides:

```sh
ANTHROPIC_BASE_URL=http://127.0.0.1:8080 \
ANTHROPIC_AUTH_TOKEN=hey-proxy \
claude
```

The base URL has **no `/v1` suffix**. `hey-proxy` is a placeholder in standalone loopback mode. In host mode, supply a generated proxy access key instead. The proxy removes the client's authorization and API-key headers and injects its own subscription bearer token. Claude Code's subscription login is not needed.

Open **http://127.0.0.1:8080/** to see subscription limits. The Traffic tab shows request token counts and timing. `hey-proxy check-credentials` checks that the proxy can load/refresh its credentials without printing them.

## Transport and limits

- `POST /v1/messages`: native Anthropic JSON and SSE, including tools, thinking signatures, cache controls and feature headers. Request bodies and response chunks pass through unchanged. Native Claude requests do not apply model aliases or cross-provider fallbacks.
- `POST /v1/messages/count_tokens`: native Anthropic token counting.
- `GET /claude/usage`: normalized account-wide subscription usage, fetched from `/api/oauth/usage` with the `oauth-2025-04-20` beta header. Includes five-hour, weekly, available model-specific windows, newer scoped `limits` entries, and extra-usage status. Missing values remain unknown, not zero.

Usage includes activity outside this proxy. The usage panel shows percent used, reset dates in your local timezone, and the last successful fetch time. Polling is coalesced and cached for 60 seconds by default. HTTP 429 honors `Retry-After` (up to a day); temporary failures retain the previous reading marked **stale**. An account/token change clears the usage cache. No OAuth token, email, or arbitrary profile metadata is included in usage responses.

Upstream inference errors, `Retry-After`, and Anthropic rate-limit headers pass back to Claude Code. A rejected access token can refresh and retry once after HTTP 401; started streams and rate-limited model calls are never replayed by this route. Interrupted streams without `message_stop` are recorded as incomplete failures. Subscription request counts are not presented as API spend.

## Credential ownership

For `config.json`, the default encrypted store is `config.claude.json`, with a private sibling `config.claude.key` and a lock file. Access and refresh tokens are encrypted together with AES-256-GCM-SIV. Token/key files are mode 0600 on Unix. Keep the store and key together; anyone able to read both can decrypt the tokens. They are never returned to clients or included in request history/config exports.

The proxy refreshes near expiry, saves rotated tokens atomically, and locks across processes to prevent two refreshes using the same token. Refresh work finishes its durable save even if the requesting client disconnects. Transient refresh failures back off. If authorization is revoked, run `hey-proxy claude-login` again. Login replaces only the proxy's own credentials.

Optional provider settings:

```json
"claude": {
  "credentials_file": "personal.claude.json",
  "usage_cache_seconds": 60,
  "upstream_url": "https://api.anthropic.com"
}
```

Relative credential paths resolve beside the proxy config. `credentials_file` must end in `.json`; its `.key` and `.lock` siblings are reserved. `usage_cache_seconds` accepts 30–3600 seconds. A custom upstream must be an HTTPS root URL; loopback HTTP is supported for testing. It receives your access token, so use only a trusted upstream.

Host mode protects usage with the same access key/session as the dashboard. Client relays request the host's usage and hold no Claude OAuth credentials. Remote rollout does not copy the OAuth store: sign in separately on the machine hosting Claude requests.

## Research basis

Reviewed current upstream source on **2026-09-30**. The following MIT projects were active that day:

| Project | Findings used |
| --- | --- |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI/tree/97f244b8ddb9cbf564b6e6faab0159102cca8617) | Current OAuth authorization/token endpoints, client ID, profile/inference scopes, PKCE, refresh rotation, and preserving future Claude feature headers. See [OAuth implementation](https://github.com/router-for-me/CLIProxyAPI/blob/97f244b8ddb9cbf564b6e6faab0159102cca8617/internal/auth/claude/anthropic_auth.go). |
| [CodexBar](https://github.com/steipete/CodexBar/tree/5de8b9ccfdcf3ed13d7c67e3639a2dd18d11230f) | OAuth usage endpoint/beta header, optional quota windows, the current scoped `limits` schema, stale readings, and usage-endpoint backoff. See [usage fetcher](https://github.com/steipete/CodexBar/blob/5de8b9ccfdcf3ed13d7c67e3639a2dd18d11230f/Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift). |
| [Quotio](https://github.com/nguyenphutrong/quotio/tree/a9cc501c69378a2e6606d76450c749fdbc7ac90c) | Maintained quota-dashboard alternative; broader multi-provider application rather than an embedded Rust proxy. |

This implementation uses native Claude Code traffic and independent proxy credentials. It does not embed these projects or translate other clients into Claude Code. Subscription OAuth and usage endpoints are service-specific and may change; actual subscription access requires completing Claude authorization. Offline tests use synthetic credentials and local upstreams.

For the optional installed-client smoke test, run `uv run --with cryptography tests/claude_code_smoke.py target/debug/hey-proxy` after `cargo build`. It uses isolated Claude settings, synthetic encrypted credentials, and a local upstream; it does not contact the subscription service.
