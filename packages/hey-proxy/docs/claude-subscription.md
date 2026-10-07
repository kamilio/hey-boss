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

Open **http://127.0.0.1:8080/** for RPM and estimated spend today, this week, and all time. Subscription limits and API setup are at **http://127.0.0.1:8080/apis**. `hey-proxy check-credentials` checks that the proxy can load/refresh its credentials without printing them.

## Transport and limits

- `POST /v1/messages`: native Anthropic JSON and SSE, including tools, thinking signatures, cache controls and feature headers. Request bodies and response chunks pass through unchanged. Native Claude requests do not apply model aliases or cross-provider fallbacks.
- `POST /v1/messages/count_tokens`: native Anthropic token counting.
- `GET /usage/v1/accounts`: configured provider/account aliases; no provider fetch or OAuth-store access.
- `GET /usage/v1/{provider}/{account}`: versioned, typed subscription quota and extra-spend reading. Currently `claude/default`.
- `GET /claude/usage`: compatibility endpoint for normalized account-wide subscription usage, fetched from `/api/oauth/usage` with the `oauth-2025-04-20` beta header. Includes five-hour, weekly, available model-specific windows, newer scoped `limits` entries, and extra-usage status. Missing values remain unknown, not zero.

Usage includes activity outside this proxy. The usage panel shows percent used, reset dates in your local timezone, and the last successful fetch time. Polling is coalesced and cached for 60 seconds by default. HTTP 429 honors `Retry-After` (up to a day); temporary failures retain the previous reading marked **stale**. An account/token change clears the usage cache. No OAuth token, email, or arbitrary profile metadata is included in usage responses.

Upstream inference errors, `Retry-After`, and Anthropic rate-limit headers pass back to Claude Code. A rejected access token can refresh and retry once after HTTP 401; started streams and rate-limited model calls are never replayed by this route. Interrupted streams without `message_stop` are recorded as incomplete failures. The spend dashboard uses reported tokens and published Claude rates to estimate the API-equivalent value of subscription usage; this does not represent additional subscription charges. Cache reads, five-minute and one-hour cache writes, supported fast-mode pricing and US inference geography are included. Unknown models or missing usage remain unpriced. Historical requests without cache-lifetime or speed metadata use standard rates.

## Credential ownership

For `config.json`, the default encrypted store is `config.claude.json`, with a private sibling `config.claude.key` and a lock file. Access and refresh tokens are encrypted together with AES-256-GCM-SIV. Token/key files are mode 0600 on Unix. Keep the store and key together; anyone able to read both can decrypt the tokens. They are never returned to clients or included in request history/config exports.

Access tokens are cached in memory. Active traffic revalidates the proxy-owned files in the background after 250 ms; file errors invalidate cached credentials. After two seconds without successful validation, requests revalidate before using the token, including the first request after idle time. HTTP 401 invalidates the rejected token immediately. Cold reads, decryption and durable saves run outside the async request workers.

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

Current token prices were checked against [Claude's published pricing](https://docs.claude.com/en/docs/about-claude/pricing) on **2026-10-01**, with explicit dated model aliases cross-checked against [LiteLLM's price catalog](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json). Existing recorded costs are preserved; missing historical Claude estimates are backfilled in bounded batches by the background writer.

## Research basis

Reviewed current upstream source on **2026-09-30**. The following MIT projects were active that day:

| Project | Findings used |
| --- | --- |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI/tree/97f244b8ddb9cbf564b6e6faab0159102cca8617) | Current OAuth authorization/token endpoints, client ID, profile/inference scopes, PKCE, refresh rotation, and preserving future Claude feature headers. See [OAuth implementation](https://github.com/router-for-me/CLIProxyAPI/blob/97f244b8ddb9cbf564b6e6faab0159102cca8617/internal/auth/claude/anthropic_auth.go). |
| [CodexBar](https://github.com/steipete/CodexBar/tree/5de8b9ccfdcf3ed13d7c67e3639a2dd18d11230f) | OAuth usage endpoint/beta header, optional quota windows, the current scoped `limits` schema, stale readings, and usage-endpoint backoff. See [usage fetcher](https://github.com/steipete/CodexBar/blob/5de8b9ccfdcf3ed13d7c67e3639a2dd18d11230f/Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift). |
| [Quotio](https://github.com/nguyenphutrong/quotio/tree/a9cc501c69378a2e6606d76450c749fdbc7ac90c) | Maintained quota-dashboard alternative; broader multi-provider application rather than an embedded Rust proxy. |

This implementation uses native Claude Code traffic and independent proxy credentials. It does not embed these projects or translate other clients into Claude Code. Subscription OAuth and usage endpoints are service-specific and may change; actual subscription access requires completing Claude authorization. Offline tests use synthetic credentials and local upstreams.

For the optional installed-client smoke test, run `uv run --with cryptography tests/claude_code_smoke.py target/debug/hey-proxy` after `cargo build`. It uses isolated Claude settings, synthetic encrypted credentials, and a local upstream; it does not contact the subscription service.

## CLI and Rust SDK

```sh
hey-proxy usage
hey-proxy --config /path/to/config.json usage --json
hey-proxy usage --accounts --json
hey-proxy usage --provider claude --account default
hey-proxy usage --base-url https://your-proxy --token-env HEY_PROXY_TOKEN
```

`usage` reads the existing config and contacts the running proxy. It does not start the proxy, create a config, sign in, or read subscription OAuth credentials. In host mode it reads the existing local proxy access key. In client mode it contacts the local relay, which authenticates to the host. An explicit `--base-url` skips config access entirely and uses only the named environment variable for authentication. Never put a Claude OAuth token in that variable. `--timeout-seconds` defaults to 30; it covers the whole HTTP response. There is no automatic polling or forced cache bypass.

Exit status 0 means a fresh (`ok`) reading or successful account listing. Stale, disabled, unknown-state and provider-error readings print their data and exit 1; transport/authentication/unsupported-account errors also exit 1. Invalid CLI arguments exit 2. JSON goes to stdout, diagnostics to stderr. Account listing returns configured aliases without checking sign-in validity; currently the alias is `claude/default`. Selecting another Claude account returns HTTP 404. Codex and other unimplemented providers return HTTP 501, and are not listed as available accounts.

The public Rust library exposes the same types used by the server and CLI:

```toml
[dependencies]
hey-proxy = { git = "https://github.com/kamilio/hey-proxy" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust,no_run
use hey_proxy::usage::{Client, State};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("HEY_PROXY_TOKEN").ok();
    let client = Client::new("http://127.0.0.1:8080", token.as_deref())?;
    for account in client.accounts().await?.accounts {
        let usage = client.usage(&account.provider, &account.id).await?;
        if usage.reading.state != State::Ok {
            eprintln!("Reading is {:?}; last update {:?}", usage.reading.state, usage.reading.updated_at);
        }
        if let Some(data) = usage.reading.data {
            for window in data.windows {
                println!("{}: {:?}% left; resets {:?}", window.label, window.remaining_percent, window.resets_at);
            }
            if let Some(spend) = data.extra_usage.and_then(|extra| extra.spend) {
                println!("Extra spent: {:?} {}; budget left: {:?}", spend.used, spend.currency, spend.remaining);
            }
        }
    }
    Ok(())
}
```

SDK HTTP failures are `usage::Error`; HTTP-success readings can still be stale or unavailable, so inspect `reading.state`. The client rejects redirects, bounds response bodies to 1 MiB, checks schema versions and returned account identity, and never includes access keys or remote response bodies in errors. Provider/account identifiers are strings rather than a closed provider enum, allowing future adapters and multiple account aliases. Unknown future state values deserialize as `State::Unknown`.

## Extra spend and API contract

`GET /usage/v1/claude/default` responds with `schema_version: 1`, `account: {provider: "claude", id: "default"}`, plus `state`, `updated_at` (Unix seconds), `retry_after_seconds`, `error`, and `data`. Data contains `windows` and optional `extra_usage`. Every window has an ID, label, optional group, used and remaining percentages, and optional reset time. Remaining percentage is `max(0, 100 - used_percent)`; missing utilization is never converted to zero. Scoped windows are separate constraints; do not add or average their percentages into one account allowance.

Extra usage includes `enabled`, `used_percent`, `remaining_percent`, and optional `spend`:

```json
{
  "currency": "USD",
  "period": "monthly",
  "used": 12.5,
  "limit": 50.0,
  "remaining": 37.5,
  "over_limit": 0.0,
  "resets_at": null
}
```

These are provider-reported extra-usage amounts in **major currency units**. Claude's OAuth `used_credits` and `monthly_limit` are divided by 100; an omitted currency uses the endpoint's USD default. The compatibility endpoint retains the raw fields as well. Unrecognized currency values suppress normalized money rather than guessing. This follows the [current CodexBar OAuth amount normalization](https://github.com/steipete/CodexBar/blob/f46a125af227254ac14de591968bce88d9fcc0f5/Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift#L1099) (checked 2026-10-01).

`used` is spending beyond included subscription usage; `over_limit` is the amount above the **extra-spend cap**, not the amount above the subscription allowance. `remaining` is cap headroom, not a prepaid balance. Missing amounts/caps stay null; a zero cap stays zero. Turning extra usage off does not erase previously reported spending. Reset dates are not guessed. These readings are not an invoice and do not include tax, subscription fees, daily/weekly billing breakdowns, or reliable per-request charge attribution. The RPM/spend dashboard continues to show API-equivalent estimates; actual extra spend is displayed in the `/apis` subscription panel and the usage CLI/API.

The new endpoints share Claude's existing quota cache, single-flight fetching, credential-identity invalidation and rate-limit backoff. Account listing does no provider work. Neither new endpoint logs inference requests or changes RPM, including unauthorized checks and client relays. No new polling, database work, network requests, or credential reads run at proxy startup or on the inference path.
