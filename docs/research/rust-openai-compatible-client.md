# Which Rust client approach for OpenAI-compatible streaming?

Research findings for ticket aaronlockhartdev/tau#4 (parent of the provider-contract decision).
Researched 2026-09-15 from primary sources: vendor API docs and vendor/crate source code.

Standing constraint (ADR-0003): tau v0 supports **only** OpenAI-compatible endpoints
(base URL + key + model id), and tau is lightweight — single binary, minimal dependency surface.

## TL;DR

The de-facto stable surface across all six servers (OpenAI, OpenRouter, Groq, Ollama, vLLM,
llama.cpp) is **`POST /v1/chat/completions` with SSE streaming**. The newer Responses API
(`/v1/responses`) is implemented with wildly different fidelity: native on OpenAI and
OpenRouter, "fully compatible" on Groq, non-stateful on Ollama, a chat-completions *shim* on
llama.cpp, and recently added (text-gen models only) on vLLM. The reasoning-field surface is
a mess of dialects (`reasoning` vs `reasoning_content` vs `reasoning_details` vs `thinking`)
that only OpenAI itself keeps uniform (and even there, Chat Completions exposes **no**
reasoning field at all).

**Recommendation:** a small hand-rolled client in `tau-core` — `reqwest` + an in-tree SSE
frame parser (~100 LOC) + `serde` types for chat-completions chunks with a per-dialect
`reasoning` alias decoder. No extra framework dependency. Details and rationale in
[Recommendation](#recommendation).

---

## 1. The OpenAI-compatible API surface (state of the art, 2026-09)

### 1.1 OpenAI (the reference implementation)

- **Both APIs exist; Responses is now the recommended primitive.** The migration guide says
  "While Chat Completions remains supported, Responses is recommended for all new projects"
  and closes with "We recommend migrating all flows to the Responses API over time".
  ([platform.openai.com/docs/guides/migrate-to-responses](https://platform.openai.com/docs/guides/migrate-to-responses), accessed 2026-09-15)
- **Chat Completions status details** (changelog, accessed 2026-09-15,
  [platform.openai.com/docs/changelog](https://platform.openai.com/docs/changelog)):
  - GPT-6 Astra (current frontier model): "Tool calling requires the Responses API. If you use
    tools with Chat Completions, follow the Responses migration guide." Chat Completions has
    *no* function calling with GPT-6 Astra (also in the [reasoning guide](https://platform.openai.com/docs/guides/reasoning)).
  - "Starting with GPT-5.4, Chat Completions does not support tool calling with
    `reasoning_effort` values other than `none`" (migrate guide).
  - Assistants API was sunset Aug 26, 2026 — Responses is where OpenAI invests.
- **Chat Completions streaming format** ([api-reference/chat](https://platform.openai.com/docs/api-reference/chat), accessed 2026-09-15):
  - SSE stream of `data:` lines, each a chunk with `choices[].delta` (content fragments);
    the stream ends with `data: [DONE]`.
  - `stream_options: {"include_usage": true}`: "an additional chunk will be streamed before
    the `data: [DONE]` message. The `usage` field on this chunk shows the token usage
    statistics … and the `choices` field will always be an **empty array**."
  - New 2026 quirk: `stream_options.include_obfuscation` — random padding in an `obfuscation`
    delta field to normalize chunk sizes (side-channel mitigation); on by default.
  - **No reasoning field in the response schema.** Grepping the full Chat Completions API
    reference schema for "reasoning" yields zero hits: requests accept
    `reasoning_effort`/`max_completion_tokens`, but reasoning *content* is not returned on
    this endpoint at all (only `usage` token counts elsewhere).
- **Responses API** ([migrate guide](https://platform.openai.com/docs/guides/migrate-to-responses),
  [streaming guide](https://platform.openai.com/docs/guides/streaming-responses),
  [reasoning guide](https://platform.openai.com/docs/guides/reasoning), accessed 2026-09-15):
  - Typed SSE events: "Chat Completions streaming returns incremental chunks with a `delta`
    field. Responses streaming uses typed server-sent events." Key events:
    `response.created`, `response.output_text.delta`, `response.completed`, `error`
    (streaming guide); function-call streams add `response.function_call_arguments.delta/.done`
    (migrate guide).
  - Request: `input` (string or message array) + `instructions`; `store: true` by default
    (stateful; `previous_response_id` chains turns).
  - **Reasoning**: `reasoning: {effort, summary, mode, context}`. Reasoning text is **not
    returned by default**; you must opt in with `reasoning.summary`
    ("auto"/"detailed"/"concise"), which fills the `summary` array of the `reasoning` output
    item. For stateless/ZDR flows, reasoning items come back with opaque `encrypted_content`
    that you must replay in subsequent requests.
  - Usage is carried in the `response.completed` event / response object
    (`usage.output_tokens_details.reasoning_tokens`).
  - Responses also has WebSocket mode, async tool calling, mid-turn steering, and
    Conversations API (changelog) — all Responses-only.

### 1.2 OpenRouter

- Endpoints: `POST /api/v1/chat/completions` (the standard entry point; "Send standard HTTP
  requests to the `/api/v1/chat/completions` endpoint",
  [quickstart](https://openrouter.ai/docs/quickstart)); the Responses API at
  `POST /api/v1/responses` — "designed to be a drop-in replacement for OpenAI's Responses
  API" ([overview](https://openrouter.ai/docs/api_reference/responses/overview)); the
  streaming page also references `completions` and `messages` endpoints and an
  `X-Generation-Id` header on all of them
  ([streaming](https://openrouter.ai/docs/api_reference/streaming)).
- **SSE quirks (documented by OpenRouter itself,
  [streaming page](https://openrouter.ai/docs/api_reference/streaming))**:
  - Keeps the connection alive with SSE **comment** lines: `: OPENROUTER PROCESSING`.
    "If you parse the stream by hand, skip lines that start with `:` before calling
    JSON.parse."
  - **Deviates from OpenAI's usage-chunk shape**: OpenAI's final usage chunk has an empty
    `choices` array; OpenRouter's final usage chunk "contains one choice with a content-free
    `delta` that repeats the `finish_reason` (and `native_finish_reason`)" — so the terminal
    `finish_reason` appears twice and a naive client must treat the usage chunk as an
    accounting frame.
  - Mid-stream errors arrive as normal `data:` events with an `error` field.
- **Reasoning fields** ([reasoning guide](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens)):
  - Response: `reasoning` (string) on each message/delta, plus structured
    `reasoning_details` blocks (`reasoning.summary`, `reasoning.encrypted`, `reasoning.text`).
    `reasoning_content` is accepted as an alias for `reasoning`.
  - Request: `reasoning: {effort | max_tokens, exclude, enabled}` — effort values
    `max/xhigh/high/medium/low/minimal/none`; per-model capability discoverable from
    `GET /api/v1/models` (`reasoning.supported_efforts`, `mandatory`, …).
  - Usage: `usage.completion_tokens_details.reasoning_tokens`.

### 1.3 Groq

- Endpoint: `https://api.groq.com/openai/v1` — "designed to be mostly compatible with
  OpenAI's client libraries" ([OpenAI Compatibility](https://console.groq.com/docs/openai)).
  Documented *incompatibilities*: `logprobs`, `logit_bias`, `top_logprobs`,
  `messages[].name` → 400 error; `n` must be 1; `temperature: 0` coerced to `1e-8`.
- Responses API: "Groq's Responses API is fully compatible with OpenAI's Responses API"
  ([Responses API](https://console.groq.com/docs/responses-api)); requests accept
  `reasoning: {effort}` (e.g. `openai/gpt-oss-20b`,
  [responses-api page](https://console.groq.com/docs/responses-api)).
- **Reasoning fields** ([Reasoning](https://console.groq.com/docs/reasoning)):
  - `reasoning_format`: `parsed` → dedicated `message.reasoning` field; `raw` → `think` tags
    inside `content`; `hidden` → final answer only.
  - `include_reasoning`: `true`/`false` — mutually exclusive with `reasoning_format`.
  - `reasoning_effort`: `none`/`default` (Qwen) and `low`/`medium`/`high` (GPT-OSS, Qwen 3.8).
  - GPT-OSS models: reasoning in the `reasoning` field by default; `reasoning_format` not
    supported for them.

### 1.4 Ollama

- Endpoints (base URLs: local `http://localhost:11434`, cloud `https://ollama.com`;
  [API intro](https://docs.ollama.com/api)):
  - Native API `POST /api/chat` — **NDJSON streaming** (not SSE); thinking-capable models
    "emit a `thinking` field alongside regular content in each chunk"
    ([streaming capability](https://docs.ollama.com/capabilities/streaming)).
  - OpenAI-compatible `/v1`: `POST /v1/chat/completions`, `/v1/completions`,
    `/v1/models`, `/v1/embeddings` ([OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)).
    Streaming is SSE; `stream_options.include_usage` supported.
  - `POST /v1/responses` — "Added in Ollama v0.13.3. Ollama supports the OpenAI Responses
    API. **Only the non-stateful flavor is supported** (no `previous_response_id` or
    `conversation` support)." Features: streaming, tools, "Reasoning summaries (for thinking
    models)".
  - (There is also an Anthropic-compatible `/v1/messages` — outside v0 scope per ADR-0003.)
- **Reasoning field mapping** — verified in source,
  [ollama/ollama `openai/openai.go`](https://github.com/ollama/ollama/blob/main/openai/openai.go) (accessed 2026-09-15):
  - The OpenAI layer serializes the internal `Message.Thinking` to
    `json:"reasoning,omitempty"` on both `message.reasoning` (final) and
    `choices[0].delta.reasoning` (stream) — i.e. Ollama's OpenAI surface uses the
    `reasoning` field name (not `reasoning_content`).
  - Request accepts `reasoning_effort` / `reasoning.effort` with values
    `minimal/low/medium/high/xhigh/ultra/max/none`.

### 1.5 vLLM

- Endpoint: `http://localhost:8000/v1` — OpenAI-compatible server. Current docs list
  ([Online Serving](https://docs.vllm.ai/en/latest/serving/online_serving/)):
  `/v1/chat/completions` (+ `/v1/chat/completions/batch`),
  **Responses API (`/v1/responses`, `/v1/responses/{id}`, `/v1/responses/{id}/cancel`) —
  "Only applicable to text generation models"**, `/v1/completions`, `/v1/embeddings`,
  audio transcriptions/translations. (Responses support was a community-requested feature,
  e.g. [issue #14721](https://github.com/vllm-project/vllm/issues/14721).)
- **Reasoning** ([Reasoning Outputs](https://docs.vllm.ai/en/latest/features/reasoning_outputs/)):
  - Requires the server flag `--reasoning-parser <name>` (deepseek_r1, qwen3, gemma4, glm45,
    … per-model table).
  - Response field is now **`reasoning`**; the docs carry an explicit migration warning:
    "`reasoning` used to be called `reasoning_content`. To migrate, directly replace
    `reasoning_content` with `reasoning`. It is important that you also update your client
    code. Otherwise, your client code could silently read an empty `reasoning_content`…"
  - Streaming chat completions carry `delta.reasoning`.
  - Reasoning toggling on the request goes through `chat_template_kwargs`
    (`thinking`, `enable_thinking`) and/or `reasoning_effort` — model-dependent.

### 1.6 llama.cpp (`llama-server`)

- Endpoints — [tools/server/README.md](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md) (accessed 2026-09-15):
  - OpenAI-compatible: `GET /v1/models`, `POST /v1/completions`, `POST /v1/chat/completions`
    ("Both synchronous and streaming mode are supported… While no strong claims of
    compatibility with OpenAI API spec is being made, in our experience it suffices to
    support many apps"), **`POST /v1/responses`** — "This endpoint works by converting
    Responses request into Chat Completions request" (i.e. a shim, not a native
    implementation), `/v1/embeddings`, plus token-counting endpoints.
  - Anthropic-compatible: `POST /v1/messages`, `/v1/messages/count_tokens`.
  - llama.cpp-specific extras on chat completions: `/v1/chat/completions/control`
    (realtime `reasoning_end`), `reasoning_control`, `parse_tool_calls`,
    `generation_prompt`.
- **Reasoning** (same README + [llama.app/docs/api](https://llama.app/docs/api)):
  - `--reasoning-format`: `none` (raw `think` tags in content) / `deepseek`
    ("puts thoughts in `message.reasoning_content`") / `deepseek-legacy` / `auto`
    (default); "The server supports parsing and returning reasoning via the
    `reasoning_content` field, similar to Deepseek API."
  - Request: `reasoning` (`on/off/auto`), `reasoning_effort`
    (`minimal/low/medium/high/xhigh/max`), `reasoning_budget`, `reasoning_format`.

### 1.7 Compatibility matrix

| Server | `/v1/chat/completions` | `/v1/completions` | `/v1/responses` fidelity | Chat-completions SSE | Reasoning field(s) in chat-completions output |
|---|---|---|---|---|---|
| OpenAI | ✅ (older primitive; no tool-calling on GPT-6 Astra) | ✅ | **Native**, recommended | `data:` + `[DONE]`; optional usage chunk with **empty** `choices`; `obfuscation` padding | **None** (usage only) |
| OpenRouter | ✅ | ✅ | **Native** ("drop-in replacement") | `data:` + `[DONE]` + **`: ` comment keep-alives**; usage chunk has **one empty delta** + `native_finish_reason` | `reasoning` + `reasoning_details` (`summary`/`encrypted`/`text`) |
| Groq | ✅ (`api.groq.com/openai/v1`; rejects `logprobs`, `n>1`, …) | not documented in pages reviewed | "Fully compatible with OpenAI's" | SSE (OpenAI-style) | `message.reasoning` (parsed) / `think` tags (raw) / hidden |
| Ollama | ✅ (`/v1`, SSE; native `/api/chat` is **NDJSON**) | ✅ | Non-stateful only (since v0.13.3) | SSE | `reasoning` (maps native `thinking`) |
| vLLM | ✅ (`localhost:8000/v1`) | ✅ | Recently added; text-gen models only | SSE | `reasoning` (renamed from `reasoning_content`) via `--reasoning-parser` |
| llama.cpp | ✅ (best-effort compat, extras on top) | ✅ | **Shim** over chat completions | SSE | `reasoning_content` (DeepSeek-style) |

**What "OpenAI-compatible" practically includes (the union a client must survive):**

1. SSE framing per the WHATWG SSE spec: `data:` lines, blank-line-delimited events;
   comment lines (`: …`) that must be ignored (OpenRouter keep-alives).
2. Multi-byte UTF-8 split across TCP chunk boundaries (a parser must buffer partial code
   points; both `llm`'s in-house SSE buffer and `eventsource-stream` handle this).
3. `[DONE]` sentinel — and *not* treating pre-sentinel errors as success
   (mid-stream `data: {"error": …}` events, e.g. OpenRouter; rig has an explicit test for
   "error event must not terminate the stream / must not commit a zero-usage success").
4. Usage accounting: `stream_options.include_usage` final chunk, which OpenAI sends with an
   **empty** `choices` array while OpenRouter sends **one empty delta** repeating
   `finish_reason` — a client must not double-count the finish or crash on `choices[0]`.
5. Reasoning dialects: `reasoning` (OpenAI-Responses-style, Ollama, vLLM-new, OpenRouter),
   `reasoning_content` (llama.cpp, vLLM-old, DeepSeek-style), `reasoning_details`
   (OpenRouter structured blocks), `thinking` (Ollama native API), `think`-tags-in-content
   (Groq raw, vLLM/llama.cpp `none` mode).
6. Request-side dialects: `reasoning_effort` vs `reasoning.effort` vs
   `reasoning: {effort, max_tokens, exclude}` (OpenRouter) vs `reasoning_format` +
   `include_reasoning` (Groq) vs `chat_template_kwargs` (vLLM) vs `reasoning_budget`
   (llama.cpp).

---

## 2. The Rust crate landscape

Crate metadata from the crates.io API (accessed 2026-09-15). "30d" = recent_downloads.

### 2.1 `openai-rs` — **not an API client (name-collision trap)**

- crates.io `openai-rs`: v0.1.1, last updated **2022-05-31**, single owner `Lypt0x`,
  144 downloads/30d, [crates.io](https://crates.io/crates/openai-rs).
- Its `Cargo.toml` keywords are `["openai", "ai", "neural", "genetic", "evolutionary"]` and
  the description is "A Rust implementation of OpenAI" — it implements the **OpenAI
  neuroevolution algorithm** (gym-style controller evolution), over `hyper` +
  `hyper-openssl`. Not an HTTP client for the OpenAI API at all.
  ([Lypt0x/openai-rs Cargo.toml](https://github.com/Lypt0x/openai-rs))
- The older API-client repo people often mean by this name, `1rgs/openai-rs`, **no longer
  exists on GitHub** (repo returns 404; its lineage continued as other crates).

Conclusion: `openai-rs` as named in the ticket is not a viable candidate — eliminated.

### 2.2 `llm` (graniet) — multi-backend HTTP client, mid-maintenance

- v1.3.8, updated 2026-04-19, ~20k downloads/30d,
  [crates.io](https://crates.io/crates/llm),
  [graniet/llm](https://github.com/graniet/llm).
  (Note in its README: the crate name previously belonged to `rustformers/llm`, now
  archived; the current project is a different library.)
- Feature-gated backends in one crate: OpenAI, OpenRouter, Groq, Ollama, DeepSeek, Google,
  Anthropic, Mistral, xAI, Cohere, Azure, HuggingFace, Bedrock, ElevenLabs
  (README, [Cargo.toml](https://github.com/graniet/llm/blob/main/Cargo.toml)). **Default
  features include the interactive CLI** (`cli = ["full", clap, ratatui, crossterm,
  syntect, …]`) — a library consumer must set `default-features = false`.
- Streaming: reqwest `bytes_stream()` + an in-house `SseState` buffer that splits on
  `\n\n` and handles partial UTF-8 (`valid_up_to`) — no external SSE crate
  ([src/chat/sse.rs](https://github.com/graniet/llm/blob/main/src/chat/sse.rs),
  [src/backends/openai/responses/stream/sse.rs](https://github.com/graniet/llm/blob/main/src/backends/openai/responses/stream/sse.rs)).
- OpenAI backend: custom `base_url` supported; `SUPPORTS_REASONING_EFFORT`,
  `SUPPORTS_STREAM_OPTIONS`; the structured stream path now goes through the **Responses
  API** (`send_responses_request` → typed response-stream events)
  ([src/backends/openai.rs](https://github.com/graniet/llm/blob/main/src/backends/openai.rs)).
- Typed `Usage` with dual naming: `prompt_tokens` aliased to `input_tokens`,
  `completion_tokens_details` aliased to `output_tokens_details`, incl.
  `reasoning_tokens` ([src/chat/usage.rs](https://github.com/graniet/llm/blob/main/src/chat/usage.rs)).
- Assessment: viable, but (a) the default-feature weight is a footgun, (b) 20k/30d is an
  order of magnitude less battle-tested than the alternatives, (c) its provider catalog
  model is exactly the thing ADR-0003 deliberately rejects for v0.

### 2.3 `rig` (0xPlaygrounds) — agent framework with the best compatible-SSE engine

- v0.42.0, updated 2026-08-17, ~70k downloads/30d, very active release cadence,
  [crates.io](https://crates.io/crates/rig), [0xPlaygrounds/rig](https://github.com/0xPlaygrounds/rig).
- 23 provider integrations (OpenAI, OpenRouter, Groq, Ollama, llama.cpp, Azure, …) plus a
  **generic** path: "for OpenAI-chat-compatible APIs: completions driven by
  `GenericCompletionModel` via an `OpenAICompatibleProvider` impl on the provider type
  (never a hand-rolled completion model… dialect differences go in the trait's hooks)"
  ([providers/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/mod.rs)).
  Both API surfaces are first-class: `openai::completion` *and* `openai::responses_api`.
- Its internal `openai_chat_completions_compatible` module is the most complete dialect
  handling I found in any crate:
  - SSE data payload classified per-provider profile; streaming `error` events are surfaced
    and **must not** terminate the stream or commit a zero-usage success (explicit test
    history, [openai_chat_completions_compatible.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/internal/openai_chat_completions_compatible.rs)).
  - `CompatibleChoice` decodes `#[serde(rename = "reasoning_content", alias = "reasoning")]`
    (llama.cpp/DeepSeek dialect + OpenRouter alias) and `reasoning_details`
    (`reasoning.summary`/`reasoning.encrypted`/`reasoning.text`)
    ([openai/completion/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/openai/completion/mod.rs)).
  - `native_finish_reason` handling for gateways that expose a second upstream-native
    reason (OpenRouter); usage from terminal events; reasoning lifecycle
    (`ReasoningStart`/`ReasoningEnd`) with per-wire quirks (Ollama `thinking`, Gemini
    signatures) ([chunk_lifecycle.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/internal/chunk_lifecycle.rs)).
- Dependencies: `rig-core` pulls `reqwest` (via its HTTP client), `schemars`, `serde`,
  `serde_json`, `thiserror`, `tracing`, `url`, `http`, `bytes`, `futures`,
  `eventsource-stream`, `indexmap`, optional `lopdf`/`epub`/`quick-xml`
  ([rig-core Cargo.toml](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/Cargo.toml)).
  It is a **framework** (prompts, tools, agents, memory) — far more than a transport.

### 2.4 `async-openai` (context: the de-facto standard)

- v0.42.0, updated 2026-09-09, **~2.5M downloads/30d** — by far the most-used Rust OpenAI
  client, [crates.io](https://crates.io/crates/async-openai),
  [64bit/async-openai](https://github.com/64bit/async-openai).
- Mirrors OpenAI's own API 1:1, feature-gated by surface (`responses`, `chat-completion`,
  …); full typed Responses streaming including `ResponseReasoningSummaryTextDelta`,
  `ResponseReasoningTextDelta`, function-call and MCP events
  ([types/responses/stream.rs](https://github.com/64bit/async-openai/blob/main/async-openai/src/types/responses/stream.rs)).
- Its chat-completions request type carries `reasoning_effort`/`max_completion_tokens` but
  the message types have **no** reasoning-content field — mirroring OpenAI's own API, which
  is exactly why it does *not* decode `reasoning`/`reasoning_content`/`reasoning_details`
  that OpenRouter, Ollama, vLLM and llama.cpp emit. Pointing it at other OpenAI-compatible
  servers works for plain text, but you lose the reasoning surface.

### 2.5 Hand-rolled: `reqwest` + an SSE line parser

- `reqwest` 0.13.5 (updated 2026-09-08, 182M/30d) — the standard async HTTP client;
  `Response::bytes_stream()` is the primitive every crate above builds on
  ([crates.io](https://crates.io/crates/reqwest)).
- Off-the-shelf SSE parsers:
  - `eventsource-stream` 0.2.3 (2022; repo last pushed 2024; 38★) — spec-faithful SSE
    parser as a stream of `Event`s; used inside `rig-core`.
  - `sse-stream` 0.2.6 (active 2026; **~9.3M/30d**) — minimal http-body→SSE-events
    conversion ([4t145/sse-stream](https://github.com/4t145/sse-stream)).
  - `reqwest-eventsource` 0.6.0 (2024; 71★) — reqwest-specific eventsource.
  - In-house precedent: `llm`'s ~80-LOC `SseState` buffer (§2.2) is the minimal viable
    design: accumulate bytes, split UTF-8-safely, find `\n\n`, hand `data:` payloads to a
    parser; skip `:`-prefixed lines; stop (or error) on `[DONE]`.
- Cost of going this route: you own the robustness list in §1.7 items 1–5. That list is
  short, well-documented by OpenRouter alone, and is exactly the code the in-house `llm`
  buffer and `eventsource-stream` both implement.

---

## 3. Recommendation

**`tau-core` should use a small hand-rolled OpenAI-compatible chat-completions client:
`reqwest` + an in-tree SSE frame parser + `serde` types. Do not adopt `rig`, `llm`, or
`openai-rs` in v0.**

1. **Primary surface = `POST {base}/chat/completions` with `stream: true` and
   `stream_options.include_usage`.** It is the only endpoint with a *uniform* contract
   across all six servers in the ADR-0003 compatibility set. Make the Responses API
   (`/v1/responses`) an *optional, provider-gated* second surface behind the same event
   types — it is native only on OpenAI/OpenRouter, a shim on llama.cpp, non-stateful on
   Ollama. The provider-contract ticket (which blocks on this one) should decide when a
   given provider gets routed there; the transport layer should make both cheap.
2. **Dependency surface stays at what the harness already needs:** `reqwest` (rustls),
   `serde`/`serde_json`, `tokio`, `bytes`. The SSE parser is ~100 lines in-tree:
   UTF-8-safe buffering, `\n\n` event splitting, `data:` concatenation, `:`-comment
   skipping, `[DONE]` termination. This matches the standing lightweight preference
   (single binary, minimal deps) — versus `rig-core` (framework: schemars, thiserror,
   tracing, futures, 23 providers, agent/tool abstractions) or `llm` (multi-backend
   catalog + default-on TUI CLI), both of which import exactly the catalog/weight ADR-0003
   rejects for v0.
3. **Type the reasoning dialect once, decode it many ways:** one
   `Reasoning: Option<String>` on the message/delta with
   `#[serde(default, rename = "reasoning_content", alias = "reasoning")]` plus an optional
   `reasoning_details` decode for OpenRouter; map Ollama-native `thinking` if the native
   API is ever needed. Precedent: `rig` does precisely this and its test history shows the
   failure modes (double finish-reason, mid-stream error, empty-choices usage chunk) —
   the in-tree parser should carry the same tests, which also protects against the
   vLLM `reasoning_content`→`reasoning` rename class of breakage.
4. **Known residual costs, accepted:** (a) we own the SSE edge cases (mitigated by
   porting the test cases above); (b) `reqwest`'s hyper/`h2` tree is the heaviest dep either
   way — any framework choice stacks on top of it, so hand-rolled is strictly lighter.
5. **Revisit triggers:** if tau later ships a provider catalog or needs Responses-native
   semantics (encrypted reasoning replay, async tool calls) for many providers, re-evaluate
   `rig` (the most actively maintained and the only crate with both API surfaces and
   generic-compatible handling) as a transport layer; `async-openai` remains the right
   choice *only* if a code path is OpenAI-proper-specific.
