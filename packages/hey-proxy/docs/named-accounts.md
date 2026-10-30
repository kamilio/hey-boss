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

## Claude Console workspaces

Keep `providers.claude` for the default subscription and add a separate named connection for a Console workspace. Console access uses workspace API billing, not subscription quota. No API key is created.

```json
{
  "providers": {"claude": {}},
  "account_schema_version": 1,
  "accounts": {
    "workspace": {"implementation":"claude", "auth":"console", "workspace_id":"wrkspc_YOUR_WORKSPACE", "credentials_file":"workspace.claude.json"}
  },
  "routes": [{"model":"workspace", "api_shape":"messages", "legs":[{"provider":"workspace", "override":"model"}]}],
  "overrides": {"workspace":{"model":{"from":"workspace", "to":"YOUR_PROVISIONED_MODEL_ID"}}}
}
```

Merge these fields with your existing accounts and routes. Run `hey-proxy claude-login --account workspace`, authorize the workspace in the browser, then use `workspace` as the Messages model. Ordinary Claude model names retain the default login. `hey-proxy resolve-route workspace --path /v1/messages` checks the mapping; `hey-proxy check-credentials` checks enrollment. A route does not grant access to a model: the workspace must already have access.

Login and refresh use the [official Anthropic CLI OAuth flow](https://github.com/anthropics/anthropic-cli/blob/main/pkg/cmd/cmd_auth.go), with workspace and organization binding retained in the encrypted store. Refresh cannot switch that binding. Console accounts do not call subscription quota endpoints.

## Multiple servers

`hey-proxy rollout` distributes named account definitions, model routes and overrides to standalone/host servers. Relative stores resolve beside each server's config. OAuth stores and encryption keys are never copied: each independent server requires its own login. For interactive use, forward local port 54545 to the target server, then run `hey-proxy claude-login --account workspace --no-browser` there and open its URL locally. Keep the tunnel until login completes.

Client relays hold no provider credentials or routing policy. Enroll once on their host; all its relays use the same host-owned account selection and model overrides. Do not copy one rotating refresh token to independent servers.

For unattended workloads, use [Workload Identity Federation](https://platform.claude.com/docs/en/manage-claude/workload-identity-federation). AWS uses regional STS `GetWebIdentityToken`, an enabled outbound-federation account setting, and an IAM role allowed to call that API. Bind the exact role to a service account in the target workspace. See [Anthropic's AWS setup](https://platform.claude.com/docs/en/manage-claude/wif-providers/aws) and [official SDK federation provider](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/lib/credentials/oidc-federation.ts). The SDK re-exchanges a fresh identity token at expiry; federation grants have no refresh token. Use a separate named connection with `auth: "wif"` for these jobs:

```json
"workload": {
  "implementation": "claude",
  "auth": "wif",
  "workspace_id": "wrkspc_YOUR_WORKSPACE",
  "federation": {
    "organization_id": "YOUR_ORGANIZATION_ID",
    "service_account_id": "svac_YOUR_SERVICE_ACCOUNT",
    "federation_rule_id": "fdrl_YOUR_RULE",
    "identity_token": "sh:///usr/local/bin/aws sts get-web-identity-token --region us-east-1 --audience https://api.anthropic.com --signing-algorithm RS256 --duration-seconds 900 --query WebIdentityToken --output text"
  }
}
```

Choose the AWS CLI path and regional STS endpoint for your servers. Each server uses its own ambient IAM credentials. The proxy caches the exchanged token in memory, re-exchanges before expiry or after rejection, and never writes a federation refresh-token store. Exchange failures back off and never fall through to a browser account. A protected `file:///absolute/path/to/jwt` also works when an identity provider rotates that file; it must be a regular file owned by the proxy user with mode 0600. JWT sources are read again at each exchange, not copied by rollout.

Route jobs to `workload` using the same `routes` and `overrides` contract as browser accounts. Config rollout supports accounts that have not yet been enrolled; run `check-credentials` on the destination afterward to verify browser enrollment or the live WIF exchange. WIF issuer/rule/service-account setup must be completed before that check can succeed.
