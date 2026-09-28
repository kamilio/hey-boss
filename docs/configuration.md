# Configuration

The default file is `~/.hey-proxy/config.json`. Use `--config PATH` for another file. First run and `--init` create a missing minimal file with private permissions on Unix; neither overwrites an existing config.

OpenAI is the default provider for unprefixed model names on `/v1/responses`. Prefix a model with `gemini/` to use Gemini conversion. `openai/` is an optional explicit prefix. Gemini's native endpoints use the configured Gemini provider directly.

## More than one OpenAI credential

Give each credential a project name, then select a project in an alias:

```json
{
  "listen": "127.0.0.1:8080",
  "providers": {
    "openai": {
      "api_keys": {
        "personal": "op://Personal/OpenAI/credential",
        "work": "op://Work/OpenAI/credential"
      },
      "default": {"api_key": "personal"}
    }
  },
  "aliases": [
    {"from": "work-coding", "to": "gpt-4.1", "api_key": "work"}
  ]
}
```

An alias's `api_key` is the project name, never the key itself. A reasoning route can also specify `api_key`; it takes precedence over the alias's project. Gemini uses its own provider authentication.

`providers.openai.upstream_url` defaults to `https://api.openai.com`. Set it to a compatible API's service root if needed. `providers.openai.credential_cache_seconds` and `providers.gemini.credential_cache_seconds` control command-result caching; each defaults to 2400 seconds. Native ADC manages its own token refresh.

## API-specific overwrites

An alias may include `api_shape`: `responses`, `chat_completions`, or legacy `completions`. It gates the whole rule, including model, API key, reasoning overrides, and reasoning routes. Unscoped rules retain their previous behavior. The same `from` can appear in disjoint shapes; duplicate or overlapping shapes are invalid.

Shapes are identified from the endpoint, not payload fields. Responses includes HTTP, streaming, WebSockets, and `/v1/responses/compact`. Chat Completions includes `/v1/chat/completions` and the custom adapter at `/v1/custom/chat/completions`. Scoped rules do not affect model metadata, audio, Realtime, or native Gemini endpoints. Matching ignores a trailing slash and query parameters. Alias targets are still resolved only once.

## Model budget registry

`model_registry` is the single source for Pi context limits, output limits, retained history, and compaction reserve. `configure-pi` reads it from this config; edit it here rather than editing the generated Pi files. See the [complete example](../README.md#configure-pi--optional).

- `defaults` requires all four fields: `context_window`, `max_tokens`, `keep_recent_tokens`, `reserve_tokens` (integer token counts).
- `models` maps upstream model names to overrides of any of those fields. Omitted fields inherit `defaults`. Use `gemini/` for Gemini and omit the optional `openai/` prefix for OpenAI.
- Keys describe backend models, not frontend aliases. Resolution follows one alias rewrite, all reasoning routes, and all fallback destinations. It takes the minimum of each budget so switching routes remains safe. A direct target follows its own routing rule if one exists.
- Limits must be positive; output must fit within the reserve; reserve and retention must each fit within half the context; their sum must be smaller than context. All values must fit JavaScript's safe integer range. Invalid registry entries fail validation, including entries not currently served.

For Responses models, `configure-pi` writes `contextWindow` / `maxTokens` into `models.json` and `keepRecentTokens` / `reserveTokens` into `settings.json` under `compaction.modelOverrides["hey-proxy/MODEL"]`. It removes competing context/output fields from this provider's `modelOverrides`, preserving other metadata. Registry changes therefore replace earlier generated values on every run. Without a registry, the previous preservation and inherited-budget repair behavior remains available.

The registry configures Pi; it does not clamp arbitrary proxy requests or discover upstream limits. Defaults are conservative client budgets, not claims about an unknown backend's maximum. Verified experimental-model limits should be revisited when the backend changes. Run `hey-proxy configure-pi` after registry/routing changes, then restart Pi. Project-level `.pi/settings.json` can still override user-level settings and should not duplicate registry-managed budgets.

## Applying edits

Routing, providers, credentials, and fallback rules reload for new requests. In-flight requests keep their original snapshot. Invalid changes keep the previous configuration active and print an error. Changing `listen` or persistent logging options requires a restart.

Persistent request metadata is enabled by default. To disable it:

```json
"logging": {"enabled": false}
```

The default database is `requests.sqlite3` beside the proxy config. Use `logging.database` to change its path. Prompts, payloads, credentials, and raw error messages are excluded from this history. Retention does not automatically delete old records.

## Remote setup

Remote rollout requires SSH access, Python 3, and either a macOS GUI login session or a Linux systemd user session. It builds the Rust binary on each destination. Cargo is used if installed; otherwise rollout installs a minimal Rust toolchain for that user.

Add SSH aliases from your SSH config:

```json
"ssh_hosts": ["workstation", "build-server"]
```

Then deliberately install or update their proxy services:

```sh
hey-proxy rollout
hey-proxy rollout --host workstation
```

Rollout syncs the proxy's configuration and verifies the service. It never configures Codex. To opt in on a destination, run there:

```sh
hey-proxy configure-codex --base-url http://127.0.0.1:8080/v1
```

Credential references are copied without resolving them into the source bundle. Configure 1Password, command credentials, or ADC on the destination first. Existing literal credentials must already match on the destination; rollout does not distribute them in plaintext. Destination credential checks happen before replacing the service.

### Shared host and local clients

A host owns the provider credentials. Client machines run a local relay and receive only a host access key:

```json
"ssh_hosts": [
  {
    "host": "shared-server",
    "mode": "host",
    "listen": "0.0.0.0:8080",
    "url": "http://shared-server:8080"
  },
  {"host": "laptop", "mode": "client", "via": "shared-server"}
]
```

Use a trusted private network or an HTTPS endpoint for traffic between machines. Host mode requires generated access keys for both API and dashboard access. Rollout installs hosts before their dependent clients.

When explicitly configuring Codex on a host, include its proxy config so the command can use the generated local access key:

```sh
hey-proxy configure-codex \
  --base-url http://127.0.0.1:8080/v1 \
  --proxy-config ~/.hey-proxy/config.json
```

`hey-proxy verify` checks the proxy and its connection. Add `--codex` only if you also want to verify an already-configured Codex installation. Verification does not modify Codex files.
