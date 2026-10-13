# Stateful LLM request APIs vs. stateless chat-completions

Research: which providers offer stateful / conversation-scoped / session-based APIs,
what they actually do, the transfer/latency math, the tradeoffs, and what it means
for tau (Rust agent, OpenAI-Responses-dialect client against a self-hosted vLLM).

Method: web search + provider documentation (ketch), vLLM source inspection, tau
code inspection. All claims carry a source. Researched 2026-09-24.

## 1. Provider landscape

### 1.1 True stateful / conversation-scoped APIs

**OpenAI — Responses API** (the de-facto reference)
- `POST /v1/responses` with `store: true` (default): OpenAI retains the response
  object; subsequent turns chain with `previous_response_id` and send **only the
  new input**. `store: false` is fully stateless.
  (developers.openai.com/api/docs/guides/conversation-state)
- Retention: response objects saved **30 days** by default; items attached to a
  conversation have no 30-day TTL.
- Documented recovery path: if a `previous_response_id` cannot be resolved
  (not cached), the client sends `previous_response_id: null` **plus full input
  context**. A connection-local cache keeps recent responses in memory for
  low-latency continuation; `stream_id` lanes can fork from it.
- Billing caveat, verbatim: "Even when using `previous_response_id`, all
  previous input tokens for responses in the chain are billed as input tokens."
  Stateful ≠ cheaper compute; it saves transfer, not prefill.

**Google — Gemini Interactions API** (GA June 2026; `generateContent` now legacy)
- `interactions.create` + `previous_interaction_id` = **optional server-side
  conversation state**. Stored by default (`store=true`); `store=false` opts
  into stateless mode, which is ZDR-compliant but disables chaining and
  background execution.
  (ai.google.dev/gemini-api/docs/interactions-overview)
- A stored `Interaction` is a session record: chronological execution steps
  (thoughts, `function_call`/`function_result`, `model_output`, `user_input`),
  retrievable via `interactions.get`.
- Retention: **55 days paid tier** (configurable 7/14/28/55), **1 day free
  tier**; deletable via `interactions.delete`.
- Implicit caching works in both stateful and stateless modes; stateful is
  described as letting the system "more easily utilize implicit caching for the
  conversation history". Explicit cache objects are *not* supported on the
  Interactions API.

**Anthropic — no conversation state; the "server-side context management" betas**
- **Context editing** (beta, header `context-management-2025-06-27`): the
  server edits the history *the client still sends in full*, before it reaches
  the model. Not a delta protocol.
  (platform.claude.com/docs/en/build-with-claude/context-editing)
  - `clear_tool_uses_20250919`: clears oldest tool results chronologically when
    context exceeds a trigger (`input_tokens`), keeps the N most recent
    (`keep`), optional `clear_at_least` and `exclude_tools`; cleared results
    are replaced with placeholder text.
  - `clear_thinking_20251015`: manages extended-thinking blocks; per-model-class
    defaults for how much prior thinking to keep.
  - "Context editing happens server-side. Your client application maintains the
    full, unmodified conversation history."
  - Cache interaction: clearing invalidates cached prompt prefixes at the
    cleared point (hence `clear_at_least`); kept thinking preserves the cache.
- **Memory tool** (`memory_20250818`): explicitly **client-side** — Claude
  requests file ops under `/memories`; your app implements the handler against
  storage you control. Cross-conversation persistence without server state.
  (platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool)
- **Server-side compaction** (beta, `compact-2026-09-04`): the model writes a
  summary replacing older turns, on demand or at a token threshold, optionally
  in the background. Anthropic positions this as the primary long-conversation
  strategy; context editing is for fine-grained curation.
  (platform.claude.com/docs/en/build-with-claude/compaction)

### 1.2 No stateful API — automatic prefix/KV caching instead

**Cerebras** (inference-docs.cerebras.ai/capabilities/prompt-caching)
- Prompt caching is **fully automatic** on all supported requests — no
  breakpoints, no header changes.
- Exact prefix match: "Even a single character difference in the first token
  will result in a cache miss for that block and all subsequent blocks."
- TTL: **5 minutes guaranteed**, up to 1 hour depending on load.
- **No price discount**: cached input is billed at the standard input rate.
  Benefit is TTFT plus capacity: cached tokens count toward total TPM but **not**
  the uncached-TPM bucket (the primary throughput constraint).
- Optional `prompt_cache_key` routing hint to keep one conversation's turns on
  the same cache under load.

**Fireworks** (docs.fireworks.ai/guides/prompt-caching)
- Enabled by default. "In many situations, it can reduce **TTFT by as much as
  80%**."
- Serverless: cached tokens discounted, **default 50%** (model-specific).
- Multi-replica serving: send a session identifier in the `user` field or
  `x-session-affinity` header to maximize hit rates;
  `x-prompt-cache-isolation-key` for per-tenant isolation (docs note a minor
  timing-attack surface without it).
- Cache lifetime: "at least several minutes … up to several hours", LRU eviction.

**Baseten** (baseten.co guides)
- No stateful API. **KV-cache-aware routing**: requests are routed to replicas
  that already have the needed context cached; built on SGLang RadixAttention
  (block size 1 → higher hit rates than 32-block systems).

**vLLM — core (the backend tau uses)**
- **No stateful conversation API in core.** The mechanism is
  **Automatic Prefix Caching (APC)**: vLLM caches the KV cache of processed
  queries; a new query sharing the same token prefix **skips prefill of the
  shared part**. The docs name multi-round conversation as a target workload:
  "APC allows vLLM to reuse the processing results of the chat history across
  all future rounds of conversation, allowing vLLM to serve future requests
  with much higher throughput and much lower latency."
  (docs.vllm.ai/en/latest/features/automatic_prefix_caching)
- **Enabled by default in V1**: "Thanks to the near-zero overhead, we now
  enable prefix caching by default in V1." (vllm.ai/blog/2025-01-27-v1-alpha-release;
  `--enable-prefix-caching` / `--no-enable-prefix-caching`)
- Documented limits: APC only shortens the **prefill** phase, not decode; no
  gain when prefixes don't match; cache is ephemeral GPU memory (no
  persistence, no eviction contract).
- **Minimal stateful stub in core**: the OpenAI-compatible `/v1/responses`
  endpoint accepts `previous_response_id` + `store` (default true), backed by
  a **per-process in-memory dict** (`self.response_store: dict[str,
  ResponsesResponse] = {}`, `vllm/entrypoints/openai/responses/serving.py`,
  main branch). Unknown ID → 404. No persistence, no cross-instance, lost on
  restart. A separate `previous_input_messages` (harmony format) parameter
  exists as the client-side alternative.
- A conversation-scoped feature request (vllm-project/vllm#24479) was
  **closed as not planned** — core vLLM deliberately stays stateless.

**vllm-project/agentic-api — the vLLM ecosystem's stateful layer**
- "The stateful, agentic API layer for vLLM, written in Rust" — a gateway
  **in front of** vLLM (vLLM is one supported backend, not the product).
  (github.com/vllm-project/agentic-api)
- Stateful conversations: server manages history via `previous_response_id` or
  the Conversations API (`/v1/conversations` + items); "no client-side message
  tracking, no replaying full transcripts".
- Persistence: **SQLite (default) / PostgreSQL** — survives restarts.
- **Compaction**: standalone `POST /v1/responses/compact` **and automatic
  compaction** on `POST /v1/responses`; output is a normalized window with one
  `compaction` item. (docs/guides/responses-compaction.md)
- Also: server-side tool execution (web search/MCP/code interpreter),
  multi-agent orchestration, HTTP/SSE/WebSocket transports, OpenTelemetry.
  Validated against the Open Responses compatibility suite.

**SGLang** (docs + arXiv:2312.07104)
- **RadixAttention**: radix tree keyed by token sequence → KV cache; automatic
  reuse, no application changes; new requests compute from the branch point.
  No stateful conversation API in core.

**LM Studio (local dev tool, data point)**
- `/api/v1/chat` is **stateful by default** — "you don't need to pass the full
  conversation history in every request" (lmstudio.ai/docs/developer/rest/stateful-chats).
  Local serving is moving toward stateful for exactly the reasons in this doc.

## 2. Data-transfer and latency benefits

**Stateful (delta) protocol — what it saves**
- Request body shrinks from the full history to the new turn (a 100k-token
  agent window becomes a few-KB payload). Client-side serialization, network
  transfer, and server-side tokenization of the unchanged prefix all vanish.
- But: compute is not saved for the provider — OpenAI bills all previous input
  tokens in the chain.

**Stateless + server-side prefix caching — what the providers actually sell**
- OpenAI: up to **95% discount** on cached input tokens (current docs);
  implicit caching automatic on the Responses API; 30-minute minimum TTL
  (`prompt_cache_options.ttl`); documented example deployments report **~70%**
  and **>90%** token cache-hit rates.
- Anthropic: cache reads at **10% of base input price** (90% off); 5-minute
  cache writes at 1.25×, 1-hour at 2×; minimums 512–4096 tokens by model.
  Third-party reports: up to ~85–90% latency/cost reduction on long prompts.
- Fireworks: **up to 80% TTFT reduction**; 50% default serverless discount.
- Cerebras: no discount, but lower TTFT and uncached-TPM headroom.
- Gemini: implicit caching on by default for 2.5+ models, both modes, savings
  passed through automatically.
- vLLM APC / SGLang RadixAttention: free (your own hardware); eliminates
  prefill of the shared prefix — the same latency win, with no billing layer.

**The transfer math for a local endpoint (tau's case)**
A 100k-token context is roughly 0.5–1 MB of JSON. Over loopback/LAN that is
single-digit milliseconds of transfer. The expensive cost in a re-send design
is **prefill compute**, not bytes — and APC already removes it when the prefix
is byte-stable. Transfer savings only become first-order when the endpoint is
remote (cloud) and the context is large.

## 3. Tradeoffs and risks of stateful APIs

- **Vendor lock-in / portability**: `previous_response_id` (OpenAI),
  `previous_interaction_id` (Gemini), and Anthropic's context-management betas
  are all proprietary; none is part of the OpenAI-compatible contract that
  self-hosted servers implement. An agent built on a stateful API cannot move
  providers (or to self-hosted) without re-implementation.
- **State eviction / timeout semantics**: OpenAI 30 days; Gemini 55 days paid /
  1 day free; KV caches ephemeral (minutes). Self-hosted stateful layers
  (agentic-api) make eviction your own SQLite/PG policy.
- **When state is dropped**: OpenAI documents the fallback (unresolvable ID →
  `null` + full input); vLLM core returns 404 from its in-memory store (and
  loses everything on restart); agentic-api survives restarts because state is
  persisted.
- **Replay / debugging**: with server-side state the client no longer owns the
  canonical transcript; reconstruction depends on provider retrieval APIs
  (`responses.get`, `interactions.get`) with their own retention windows.
  Offline replay, A/B re-runs, and incident forensics get harder.
- **Consistency / determinism**: prefix caches invalidate on a single-token
  change (Cerebras, Fireworks docs); Anthropic's context editing invalidates
  cached prefixes at the edit point; multi-replica serving needs session
  affinity hints (Cerebras `prompt_cache_key`, Fireworks
  `x-session-affinity`, Baseten KV-aware routing). Cache hits are best-effort
  and non-deterministic either way.
- **Security / data retention**: stateful means the server retains your
  conversations (OpenAI 30 days, Gemini 55 days). Mitigations exist
  (`store=false`, ZDR, per-account cache isolation), but a self-hosted stateful
  gateway at least keeps the data on your own storage.
- **Cost model unchanged**: full input tokens are billed even in stateful mode
  (OpenAI, verbatim above).

## 4. Assessment for tau

**Current state (from code)**
- tau talks to an OpenAI-compatible **vLLM** endpoint over the **OpenAI
  Responses API dialect** (`POST {base}/responses`, streaming; ADR-0003,
  spec §6: "every OpenAI-compatible server implements" this family).
- Already sends `prompt_cache_key` (stable per-session cache identity) and
  `prompt_cache_options` retention 24h/7d (spec §12, #35;
  `crates/tau-core/src/provider/request.rs`).
- Already parses `prompt_tokens_details.cached_tokens` for turn-usage
  telemetry (`crates/tau-protocol/src/payload.rs`).
- Reassembles and resends the **full input window** (instructions + all input
  entries) every turn.

**Does vLLM support a stateful mode?**
- **Core vLLM: no** designed stateful conversation API (feature request
  #24479 closed as not planned). It has (a) APC — default-on in V1 — which
  delivers the latency win of a stateful API (prefill skip) for free, provided
  the prefix is byte-stable; and (b) a minimal in-memory `previous_response_id`
  / `store` stub on `/v1/responses` (per-process dict, 404 on miss, gone on
  restart) — usable on a single-instance local server, not a designed feature.
- **The ecosystem's stateful mode is `vllm-project/agentic-api`**: a separate
  Rust gateway in front of vLLM with persisted conversation state
  (SQLite/PG), a Conversations API, automatic + on-demand compaction, and
  server-side tools.

**Would tau benefit? No — not for its current deployment.**
- The transfer savings are marginal on loopback/LAN (KB-scale wire delta
  replacing MB-scale JSON; both are milliseconds locally).
- The real cost (prefill of the re-sent context) is already eliminated by APC
  on tau's own hardware, at no billing layer, with no new dependency — and
  tau is already in the exact configuration the provider docs recommend:
  stable prefix + stable `prompt_cache_key` + full resend.
- What a stateful API would add, and why tau already has better answers:
  - server-side compaction (agentic-api `/v1/responses/compact`) → tau owns
    compaction in tau-core (spec §8) and controls its own context;
  - less client-side state management → tau owning the full context window is a
    *feature*: portability across any OpenAI-compatible backend, replay,
    deterministic debugging;
  - smaller request bodies → marginal locally, and only first-order for a
    remote/cloud endpoint.
- The risks are real and asymmetric for tau: binding to `previous_response_id`
  against core vLLM means a per-process in-memory store (silent 404s after a
  restart); adopting agentic-api means a gateway + persistence stack in front
  of vLLM for marginal local gain; both reduce portability — the explicit
  reason tau speaks the OpenAI-compatible Responses dialect (spec §6).

**Conclusion.** For a self-hosted vLLM backend, the stateless full-resend
protocol + prefix caching is the better architecture for tau: same prefill
latency win, full portability, full replay/debugging, no extra component.
Stateful / conversation-scoped APIs become attractive only when (a) the
endpoint is remote and the provider offers first-class stateful state +
caching (OpenAI Responses, Gemini Interactions), or (b) server-side compaction
and tool execution become requirements that justify an agentic-api-style
gateway. Neither is in scope for v0.

**Follow-ups (if/when relevant; not implemented)**
1. Verify APC is active on the vLLM build tau targets: check startup logs for
   prefix caching, and confirm `cached_tokens` > 0 from turn 2 onward in
   tau's usage telemetry (already parsed).
2. Audit context assembly for prefix stability: any volatile token (timestamp,
   per-turn nonce, non-deterministic ordering) *before* the stable prefix
   invalidates the entire request's cache.
3. Revisit agentic-api only if server-side compaction/tools become a product
   requirement.

## Sources

- OpenAI: developers.openai.com/api/docs/guides/conversation-state
  (retention, billing, fallback); /guides/prompt-caching (95% discount, 30m
  TTL, 70–90% hit rates); openai.com/index/api-prompt-caching/ (launch)
- Google: ai.google.dev/gemini-api/docs/interactions-overview (GA,
  `previous_interaction_id`, retention 55d/1d); /docs/caching (implicit
  caching, both modes)
- Anthropic: platform.claude.com/docs/en/build-with-claude/context-editing
  (beta header, strategies, cache interaction); /tool-use/memory-tool
  (client-side); /compaction (on-demand + threshold, beta)
- Cerebras: inference-docs.cerebras.ai/capabilities/prompt-caching
- Fireworks: docs.fireworks.ai/guides/prompt-caching (80% TTFT, 50% discount,
  session affinity, isolation)
- Baseten: baseten.co/resources/guide/the-baseten-inference-stack/
  (KV-cache-aware routing)
- vLLM: docs.vllm.ai …/features/automatic_prefix_caching (workloads, limits);
  vllm.ai/blog/2025-01-27-v1-alpha-release (APC default-on in V1);
  vllm/entrypoints/openai/responses/{protocol.py,serving.py} @ main
  (`previous_response_id`, `store`, in-memory `response_store`);
  github.com/vllm-project/vllm/issues/24479 (closed as not planned)
- vllm-project/agentic-api: README (stateful layer, persistence, API surface);
  docs/guides/responses-compaction.md
- SGLang: prefix-caching docs; arXiv:2312.07104 (RadixAttention)
- LM Studio: lmstudio.ai/docs/developer/rest/stateful-chats
