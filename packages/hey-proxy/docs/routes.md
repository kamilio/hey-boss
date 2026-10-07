# Ordered provider routes

This is the file-based routing **data and resolution contract**. Automatic forwarding across these legs and recommendation policy are separate consumers; this release stages the contract and exposes `resolve-route` for verification. Existing inference forwarding still uses the legacy path. Do not remove working aliases until the forwarding consumer is installed.

The existing [`accounts`](named-accounts.md) map names provider instances. Top-level `routes` matches the original logical model exactly and lists provider legs in attempt order. It is independent of worker settings. [`routes.config.json`](../examples/routes.config.json) is a complete synthetic example:

```json
"routes": [
  {"model":"gpt-6-astra", "legs":[{"provider":"ultima", "override":"astra"}]},
  {"model":"gpt-6.1-sol", "legs":[{"provider":"codex-personal"}, {"provider":"codex-work"}, {"provider":"ultima"}]},
  {"model":"claude-sonnet-5-5", "legs":[{"provider":"claude-personal"}, {"provider":"sonnet-api"}]}
],
"overrides": {
  "ultima": {"astra":{"from":"gpt-6-astra", "to":"ultima-alpha"}}
}
```

Astra has only an Ultima leg. Sol has two included subscriptions before Ultima API. Sonnet has an included subscription before a paid OpenAI-compatible Sonnet gateway. Add another account and insert its leg wherever it belongs; map insertion order never selects an account. Example endpoints, credential references and subscription stores are placeholders, not live setup instructions.

## Resolution and precedence

1. An explicitly pinned provider/account binding remains authoritative; callers do not apply the automatic route plan to it.
2. Match `routes` against the original model and optional `api_shape`. A matching route owns the complete ordered chain, bypassing legacy `aliases` and `fallbacks`.
3. Select the leg's named provider. Its optional `model` replaces the logical model for that leg; otherwise it stays unchanged. Apply only the explicitly referenced `overrides[provider][override]`, once. No other provider's override participates.
4. If no route matches, use the existing legacy path: alias once, then the existing fallback graph keyed by the rewritten upstream model. Fallback destinations are not aliased again. Client relays leave policy to their host.

Overrides reuse alias syntax (`from`, `to`, `reasoning`, `reasoning_routes`, optional `api_shape`), but cannot choose API keys: the selected account owns its credential. `from` must match the leg model, and an override's shape must match the route's shape. Incoming effort selects a reasoning destination before the override's forced effort is applied. `model` and `to` are literal terminal upstream IDs, never references to other routes or overrides; recursive references are rejected. Legacy fallback cycles remain invalid.

Optional shapes are `responses`, `chat_completions`, `completions`, `messages`, `gemini`, and `realtime`. Omission matches any request shape. Responses includes compact and WebSocket requests; Messages includes native/custom and token-counting endpoints; Gemini includes native generation and token counting. Transport capability checks belong to the forwarding consumer, not schema matching.

Validation rejects overlapping rules for a model, empty/duplicate legs, unknown provider/override references, mismatched override scopes, duplicate JSON keys, and unknown fields. Limits: 1024 routes, 16 legs per route, 128 provider scopes and 1024 overrides per scope. Route destination IDs are not reinterpreted as logical model names, so the same logical model may appear on several independent accounts without a routing cycle.

## Snapshots and inspection

`Config::route_plan` retains the complete immutable config. `Plan::select` returns the selected provider config with all rewrite/fallback rules removed, plus separate source model, resolved upstream model, provider name, implementation, billing mode, reasoning effort and opaque config revision. The forwarding consumer supplies the resolved model/effort exactly once. Provider names identify configured connections; live subscription identity remains the pinned `account_ref` from the account API.

Billing mode is derived from the selected account: `included_subscription` or `pay_per_token`. This records the routing policy category, not proof of available quota or permission for subscription overage. The forwarding consumer must gate included capacity; recommendations must independently exclude paid capacity. A paid-only route is valid even when nothing can be recommended.

A revision identifies one loaded snapshot, survives clones/selection, and changes on a new load. It is neither a credential fingerprint nor a cross-process content hash. Hot reload validates the whole file before publication; invalid edits retain the previous snapshot. In-flight plans retain their original account definitions and overrides across later attempts. No profile setup command is invoked by loading or resolving routes.

```sh
hey-proxy --config examples/routes.config.json resolve-route gpt-6-astra
hey-proxy --config examples/routes.config.json resolve-route gpt-6.1-sol
hey-proxy --config examples/routes.config.json resolve-route claude-sonnet-5-5 --path /v1/messages
```

The diagnostic emits safe metadata and `forwarding: "staged"`, never resolves credentials or sends inference requests. An unmatched model reports `legacy`; a relay reports `relay`.

Model checks during implementation (2026-10-07): the local proxy's `/v1/models` advertised `gpt-6-astra` and `gpt-6.1-sol`; allowlisted alias metadata on both Macs defined `gpt-6-astra → ultima-alpha`, with no Sol rewrite. Anthropic's [model overview](https://platform.claude.com/docs/en/about-claude/models/overview) listed `claude-sonnet-5-5`. The Sonnet example uses that explicit ID, not a guessed mapping from a display label. Gateway availability and quota are not asserted by these metadata checks.

## Worker recommendations

`worker_candidates` is an ordered list of `{ "provider": "codex-personal",
"model": "gpt-6.1-sol", "runtime": "codex" }` entries. It ranks worker choices
without changing route legs. The example prefers Codex 6.1 Sol (personal, then
work) before Claude Sonnet. There is no implicit candidate or paid fallback.

`GET /usage/v2/recommend?runtimes=codex,claude` returns schema version 2;
`usage::Client::recommend_workers(&[Runtime::Codex, Runtime::Claude])` is its typed
SDK. Supported runtimes are `codex`, `claude`, and `pi`. An empty capabilities
list selects nothing; omitting the query permits all supported runtimes.
Host authentication and client relay credentials match the usage API.

Each candidate must reference a compatible subscription leg of its logical
model route. Selection requires a successful reading less than five minutes old
and positive included capacity across every applicable reported limit. Unknown
scope/utilization, stale readings, and elapsed resets fail closed. Known model
scopes follow the resolved upstream model. Refresh is required after a reset;
paid overage and credits never establish eligibility. Subscription overrides with
effort-dependent destinations cannot be worker candidates.

The response includes the config revision, selected model/runtime and named
account, applicable quota evidence, reading/expiry timestamps, recheck time, and
one bounded reason per skipped candidate. `status: "no_recommendation"` and a
null selection are normal even when paid inference routes remain usable. Treat
`expires_at` as an exclusive validity deadline and re-query after a config change.
The version 1 provider-only API remains wire-compatible but also rejects paid
extra usage and unverified quota; use version 2 for worker assignment.
