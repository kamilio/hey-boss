# Custom Messages API

The proxy exposes an Anthropic-compatible Messages interface:

- `POST /v1/custom/messages` (raw HTTP)
- `POST /custom/v1/messages` (Anthropic SDK / Claude Code base URL: `http://127.0.0.1:8080/custom`)
- Append `/count_tokens` to either path for native Gemini token counting.

**Gemini uses a direct path:** Messages input becomes native `GenerateContent`; native Gemini SSE becomes Messages events. Neither side passes through Chat Completions or Responses. Authentication and retry transport are shared with the other Gemini endpoints. OpenAI targets use the existing Chat-to-Responses pipeline behind the Messages facade.

Normal `/v1/messages`, `/v1/responses`, and `/v1/chat/completions` routes retain their existing behavior. Only the custom paths opt into translation. Standalone mode supplies upstream credentials. Host mode accepts its access key through `x-api-key` or `Authorization: Bearer`; client relays forward to the host for conversion. Client Anthropic credentials are never sent to Gemini.

## Claude Code

With a configured alias such as `gemini-test -> gemini/gemini-early-exp`:

```sh
ANTHROPIC_BASE_URL=http://127.0.0.1:8080/custom \
ANTHROPIC_API_KEY=hey-proxy \
ANTHROPIC_MODEL=gemini-test \
ANTHROPIC_DEFAULT_HAIKU_MODEL=gemini-test \
ANTHROPIC_DEFAULT_SONNET_MODEL=gemini-test \
ANTHROPIC_DEFAULT_OPUS_MODEL=gemini-test \
claude --model gemini-test
```

The example key is a placeholder for standalone mode; use your host access key for host mode. Redirect the auxiliary model names too, so summarization and helper calls use configured models. The adapter accepts Claude's `beta=true` query, adaptive thinking, omitted thinking display, and system-role environment updates. System updates preserve their position in the conversation; historical signed turns remain intact. Tool results and subsequent text occupy separate native turns, including post-compaction reminders.

Claude owns its automatic compaction policy. The proxy returns actual Gemini input/output/cache counts on every successful response, including final SSE `message_delta`. Output usage includes thinking tokens. Messages input usage excludes cache reads; cache reads are reported separately. Partial usage metadata in early SSE chunks is accumulated until final counts arrive. Missing terminal events or usage are errors, preventing a successful-looking response with an unknown context size. `/count_tokens` calls Google's native token counter, including system instructions and tool declarations; it never uses a character estimate.

Claude's own `autoCompactWindow` setting / `CLAUDE_CODE_AUTO_COMPACT_WINDOW` controls its compaction window. A custom model name may use Claude's conservative unknown-model default. Choose a window within your verified model limits; do not disable compaction to work around a context error. Pi's `model_registry` settings do not configure Claude's separate client settings.

## Tools, thinking, and continuity

The direct Gemini path supports text, inline base64 images/PDFs, plain text documents, client function tools, parallel calls, tool results (including errors), sampling, thinking budgets/levels, JSON schema output, JSON responses, and SSE. It validates partial native function-call assembly before emitting a complete tool argument object. The provider's actual tool names are restored on output.

Preserve the entire assistant `content` array, including `thinking.signature` and `redacted_thinking.data`, in the next request. These are authenticated, model-bound proxy carriers containing the original native turn and its Google thought signatures. They are not raw Anthropic signatures and cannot be replayed to another provider/model. Text and tool arguments must remain unchanged. Client-added `cache_control` hints are excluded from the integrity comparison, so compaction's cache annotations do not invalidate replay.

Anthropic cache hints are accepted as hints; Gemini manages its own caching and the shim does not create Anthropic cache entries. `clear_thinking_20251015` with `keep: "all"` is an explicit no-op. Other server-side context-management edits are rejected; Claude's client-side compaction uses ordinary Messages calls and remains available.

The direct Gemini path retries transient failures before streaming. It does not replay an interrupted generation or switch signed conversation history to another model. Configured cross-model fallback rules are not applied to the direct Messages path.

## Limits

This is a compatibility shim, not an Anthropic backend. Anthropic-hosted tools, MCP server execution, citations, remote media sources, batch APIs, server-side compaction blocks, nonempty stop sequences (Gemini does not report which sequence matched), and other unmapped features return explicit errors. Native Gemini thinking budgets are honored for budget-based models; level-based models use effort levels. Provider limits still apply. Tool calls to undeclared tools, malformed/incomplete arguments, missing usage, truncated streams, and replay mismatches fail explicitly. No successful `message_stop` follows a stream error.

Requests and retained native response data are bounded to 64 MiB, and SSE frames to 16 MiB. Reasoning replay carriers add overhead; they are preserved for correctness. Gemini counts can differ from Anthropic tokenization, but reported usage is the actual backend usage that Claude uses for its context accounting.

## Verification

Focused fixture tests cover direct native routing, signed tool replay, client cache annotations, parallel tools, Unicode frame boundaries, usage, truncation, authentication, and relay behavior:

```sh
cargo test --bin hey-proxy proxy::messages
```

`tests/messages_claude_live.py` first verifies SSE completion directly (Claude can silently retry failed streams as non-streaming), then runs real Claude Code tasks against an already running proxy, checks generated files/tests independently, forces a lower client compaction threshold for the test, and requires an automatic compaction boundary followed by successful tool use. It makes real model calls; run it deliberately, outside CI.

On 2026-09-29, Claude Code 2.1.283 with `gemini-early-exp` completed the invoice, ledger, and strict-mode follow-up tasks: 12 tests passed, both 700-row ledger totals matched an independent oracle, and the CLI rejected invalid data only in strict mode. Three automatic compactions succeeded, including 49,812 → 2,654 tokens, followed by successful tool work. The isolated proxy log recorded 21 successful streamed generations and one native token-count request, with no failed requests or non-streaming generation fallbacks. The reduced compaction threshold applies only to the test process.
