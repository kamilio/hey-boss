# Named provider accounts

See [ordered provider routes](routes.md) for the staged top-level routing contract, provider-scoped overrides, precedence and safe resolution diagnostics.

Agent runtimes (Codex, Claude Code, Pi) are clients. A connection selects a provider implementation and its billing/authentication mode; its name is a local alias, not a subscription identity. No account priority or paid-capacity policy is implied.

```json
{
  "listen": "127.0.0.1:8080",
  "account_schema_version": 1,
  "accounts": {
    "codex-personal": {"implementation":"codex", "auth":"subscription", "credentials_file":"personal.codex.json"},
    "codex-work": {"implementation":"codex", "auth":"subscription", "credentials_file":"work.codex.json"},
    "claude-personal": {"implementation":"claude", "auth":"subscription", "credentials_file":"personal.claude.json"},
    "ultima": {"implementation":"openai", "auth":"api", "endpoint":"https://api.example.com", "credential":"op://Private/Ultima/key"}
  }
}
```

Names use ASCII letters, digits, hyphens and underscores (1–128 characters). `default` is reserved for legacy usage clients. API endpoints are service roots. Subscription endpoints and the Codex OAuth issuer can be overridden with `endpoint` and `issuer`. Unknown schemas, implementations, authentication modes, names and fields fail validation. Named API connections require `op://` or an absolute `file://` reference; file credentials must be owner-only regular files, never symlinks.

## Connect on the credential-owning host

`hey-boss proxy codex-login --account codex-work` signs into the selected store. `--import-codex-home` reads the existing Codex login without writing it. `hey-boss proxy claude-login --account claude-personal` uses an independent proxy-owned OAuth store. Relative stores resolve beside the proxy config. Existing unrelated files and CLI login directories cannot be used as login destinations. Tokens are encrypted locally; relays query the host and receive no credentials.

`hey-boss proxy usage --provider codex --account codex-work` reads one subscription. `usage --accounts` lists names. The API setup page shows readiness, duplicate-subscription aliases and per-account limits. API connections have no subscription quota reading.

## Pin a connection

`GET /providers/v1` and `hey_proxy::usage::Client::connections()` return schema version 1, capabilities and allowlisted connections: name, implementation, auth, readiness and opaque `account_ref`. No raw identity, token, endpoint credential or local credential path is returned. Missing stores or unverifiable subscription identities are not ready. Older hosts return 404; unsupported schemas must be rejected. Existing `/usage/v1` clients continue to use the same schema and `provider/default` aliases.

Persist both the connection name and `account_ref` with a session. Send `x-hey-proxy-provider` and `x-hey-proxy-account` on every inference request. The reference is a host-local, secret-salted identity digest. Missing, removed or changed bindings return HTTP 409; clients must require explicit account selection and must not silently replace a saved reference. A rename therefore requires an explicit session update. Copy connection on the API setup page copies only these safe values.

Codex subscription connections accept the Responses API; Claude subscriptions accept native Messages; OpenAI-compatible API connections use the existing API adapters. Explicit selection disables legacy model aliases and fallback credentials for that request. Requests retain their selected configuration and credentials through reload. OAuth expiry is refreshed within the selected store; an upstream rejection of already pinned credentials requires a new request rather than changing identity in flight.

## Compatibility and state

Legacy `providers.codex`, `providers.claude`, `providers.openai` and top-level OpenAI fields retain their endpoints, paths, aliases and default behavior. The deterministic compatibility adapter maps their default credential stores into the same path-indexed runtime registry; no credential files are moved or CLI logins changed. Named connections are opt-in and never become the default by insertion order. Config serialization retains all named definitions and the account schema version. Reload validates a complete config and publishes one immutable snapshot; invalid edits retain the last valid config.

OAuth managers and refresh backoffs are indexed by canonical credential-store path, not by implementation singleton. Different stores never share mutable tokens. Quota readings and rate-limit cooldowns are shared by opaque provider-reported subscription identity, including aliases using independently stored tokens. The host's private `.credentials.key` makes references stable across restarts; preserve it with the encrypted stores. API key rotation changes its connection reference. Quota readings remain in-memory caches and refresh after service restart.
