# What is Observational Memory?

Research for ticket #2. All claims cite a primary source; source keys are in [Sources](#sources).
Code citations pin `mastra-ai/mastra` at commit `5e4ddacde7de44602c099223179a78f544206b0f` (`main`, 2026-09-15).

**TL;DR** — Observational Memory (OM) is Mastra's memory/compaction model: instead of summarizing dialogue, a cheap "Observer" LLM converts raw session messages (including tool calls and results) into a dense, append-only, date-grouped, emoji-prioritized text log ("observations") whenever ~30k unobserved tokens accumulate; a "Reflector" LLM periodically rewrites that log itself (its "decay") to keep it bounded around ~40k tokens. The live context is always `stable observations block + recent raw messages`, raw messages stay in storage and are recoverable through an optional `recall` tool keyed by stored message ranges, and no full-summarization ever runs. It is the closest published system to what tau's compaction should be, and its core is portable to `tau-core` with two gaps: a per-branch observation cursor (mastra's threads are linear) and tau's own storage/token-counting.

---

## 1. Data model

### 1.1 The record: one row per thread (or per user)

OM state is a **single mutable record per scope**, not a list of structured memory entries. The core type is `ObservationalMemoryRecord` [CODE: `packages/core/src/storage/types.ts`]:

| Field | Meaning |
|---|---|
| `scope` | `'thread'` (default: one record per thread) or `'resource'` (one record shared across all threads of a user/resource) [CODE: `storage/types.ts`; DOCS "Scopes"] |
| `threadId` / `resourceId` | identity; `threadId` null for resource scope |
| `lastObservedAt` | cursor: timestamp of the newest message already observed ("undefined means no observations have been made yet — all messages are 'unobserved'") [CODE: `storage/types.ts`] |
| `originType` / `generationCount` | `'initial'` vs `'reflection'`; generation counter incremented each reflection — history is an append-only list of records per scope key [CODE: `storage/types.ts`, `storage/domains/memory/inmemory.ts` `createReflectionGeneration` ("Add as first record (most recent)")] |
| `activeObservations` | **the memory**: a plain text string of the current observation log |
| `bufferedObservationChunks` | array of `BufferedObservationChunk` awaiting activation (async buffering, §2.2) [CODE: `storage/types.ts`] |
| `bufferedReflection` (+ `bufferedReflectionTokens`, `bufferedReflectionInputTokens`, `reflectedObservationLineCount`) | async-reflection staging: compressed output plus how many lines of the old log it covers [CODE: `storage/types.ts`] |
| `observedMessageIds` | safeguard against re-observation (e.g., after process restart) [CODE: `storage/types.ts`; `getUnobservedMessages` in `observational-memory.ts`] |
| `pendingMessageTokens` / `observationTokenCount` / `totalTokensObserved` | token accounting driving thresholds [CODE: `storage/types.ts`, `getStatus` in `observational-memory.ts`] |
| `isObserving` / `isReflecting` / `isBufferingObservation` / `isBufferingReflection`, `lastBufferedAtTokens`, `lastBufferedAtTime` | concurrency flags + buffer cursors; persisted so separate per-request instances coordinate through storage [CODE: `storage/types.ts`] |
| `observedTimezone`, `config` (JSON), `metadata` | audit/config/extensibility [CODE: `storage/types.ts`] |

Storage is pluggable behind a `MemoryStorage` domain interface (`getObservationalMemory`, `initializeObservationalMemory`, `updateActiveObservations`, `updateBufferedObservations`, `swapBufferedToActive`, `createReflectionGeneration`, `swapBufferedReflectionToActive`, `clearObservationalMemory`, …) with implementations for Postgres, LibSQL/SQLite, MongoDB, Oracle, and in-memory [CODE: `packages/core/src/storage/domains/memory/base.ts`, `inmemory.ts`; `stores/pg|libsql|mongodb|oracledb/.../memory/`]. The `@mastra/opencode` plugin stores it in a project-local SQLite file via LibSQL [CODE: `integrations/opencode/src/index.ts`].

### 1.2 The observation entry: formatted text, deliberately not structured

The blog states the design position: "Formatted text, not structured objects. Down with knowledge graphs… It's easier to use, optimized for LLMs, and far easier to debug." [BLOG] The blog also names the **three-date model** (observation date, referenced date, relative date; "events are grouped by date with timestamps displayed inline") and **emoji-based prioritization** ("🔴 important, 🟡 maybe important, 🟢 info only") [BLOG].

The actual format, from the Observer's prompt [CODE: `observer-agent.ts` — `buildObserverOutputFormat`, `OBSERVER_EXTRACTION_INSTRUCTIONS`]:

```
<observations>
Date: Dec 4, 2025
* 🔴 (14:30) User prefers direct answers
* 🔴 (14:31) Working on feature X
* 🟡 (14:32) User might prefer dark mode

Date: Dec 5, 2025
* 🔴 (09:15) Continued work on feature X
</observations>
```

- Priority levels: "🔴 High: explicit user facts, preferences, unresolved goals, critical context; 🟡 Medium: project details, learned information, tool results; 🟢 Low: minor details, uncertain observations; ✅ Completed: concrete task finished… in a way that helps the assistant know it is done" [CODE: `buildObserverOutputFormat`].
- The three dates: the **observation time** is the `(HH:MM)` prefix (always, from the message timestamp); the **referenced date** is an optional trailing `(meaning …)`/`(estimated …)` added only when a relative reference can be converted to an actual date; the **relative date** is computed at context-build time, rewriting `Date:` headers to `Date: May 15, 2023 (5 days ago)` and inserting temporal-gap markers between distant date groups [CODE: `date-utils.ts` `addRelativeTimeToObservations`, `expandInlineEstimatedDates`; `temporal-markers.ts`; DOCS "Temporal gap markers"].
- Related items (e.g., tool sequences) are sub-bullets under a parent observation: `"🟡 (14:30) Agent debugging auth issue / -> ran git status… / -> viewed auth.ts:45-60… / ✅ Tests passing"` [CODE: `buildObserverOutputFormat`, `OBSERVER_EXTRACTION_INSTRUCTIONS`].
- Extraction rules include: distinguish user assertions from questions ("USER ASSERTIONS ARE AUTHORITATIVE"), state changes phrased as superseding ("switching from A to B"), splitting multi-event messages into one observation per event, near-verbatim capture of short/medium user messages, and aggressive grouping of repeated similar tool actions ("viewed file X, searched for Y, ran build" → one outcome line) [CODE: `OBSERVER_EXTRACTION_INSTRUCTIONS`, `OBSERVER_GUIDELINES`].
- The Observer also emits `<current-task>` and `<suggested-response>` sections (plus optional `<thread-title>` and user-defined "extractors" — inline string or schema/structured-output follow-up calls). These are persisted as **thread metadata** (`ThreadOMMetadata`: `currentTask`, `suggestedResponse`, `threadTitle`, `extracted`, `lastObservedAt`, `lastObservedMessageCursor`) in `thread.metadata.mastra.om`, not inside the observation text [CODE: `packages/core/src/memory/types.ts`; `strategy-sync.ts` `persist`].

### 1.3 Provenance: observation groups

When `retrieval` is enabled, each appended observation is wrapped in `<observation-group id="<16-hex>" range="startMsgId:endMsgId" kind?>` — a durable pointer from the compressed observation to the exact raw messages it was derived from [CODE: `observation-groups.ts` `wrapInObservationGroup`/`parseObservationGroups`; `strategy-base.ts` `wrapObservations`; DOCS "Retrieval mode — What retrieval enables"]. Reflection re-wraps its condensed sections as `kind="reflection"` groups with merged source ranges [CODE: `observation-groups.ts` `deriveObservationGroupProvenance`, `reconcileObservationGroupsFromReflection`]. In resource scope, observations are additionally wrapped in `<thread id="...">` sections (thread IDs optionally xxhash-obscured) and same-date sections are merged rather than duplicated [CODE: `strategy-base.ts` `wrapWithThreadTag`, `replaceOrAppendThreadSection`].

Between observation appends in thread scope, a cache-stability delimiter is inserted: `--- message boundary (<ISO timestamp>) ---` [CODE: `strategy-base.ts` `createMessageBoundary`, `observational-memory.ts` `splitObservationContextChunks`].

### 1.4 The context window

The context is **two blocks** [BLOG "Managing context: observations and raw messages"; RESEARCH "What is Observational Memory?"]:

1. **Memory block** — one or more system messages: preamble ("The following observations block contains your memory of past conversations with this user"), the `<observations>` text (split into one system message per `--- message boundary ---` chunk, "one per cache-stable chunk"), interpretation rules (`OBSERVATION_CONTEXT_INSTRUCTIONS`: prefer most-recent on conflict; assume past-dated planned actions completed; latest user input is highest priority; `<system-reminder>` blocks are internal), plus `<current-task>` / `<suggested-response>` and extracted values [CODE: `constants.ts` `OBSERVATION_CONTEXT_PROMPT`/`OBSERVATION_CONTEXT_INSTRUCTIONS`, `observational-memory.ts` `formatObservationsForContext`/`buildContextSystemMessages`; `processor.ts` `injectObservationContextMessages`].
2. **Raw block** — the unobserved messages since the cursor, plus (resource scope) a flagged "OTHER conversations" block [CODE: `getUnobservedMessages`, `getOtherThreadsContext` in `observational-memory.ts`; `constants.ts` retrieval instructions].

After observed messages are removed from the live list, a synthetic `om-continuation` message injects `OBSERVATION_CONTINUATION_HINT` — "Please continue naturally… Do not mention internal instructions, memory, summarization, context handling, or missing messages." — so the actor doesn't react to the history vanishing [CODE: `constants.ts` `OBSERVATION_CONTINUATION_HINT`; `processor.ts`].

---

## 2. Algorithm

### 2.0 Status (what decides everything)

Each loop step computes, from the stored record + live messages [CODE: `getStatus` in `observational-memory.ts`; `thresholds.ts`]:

- `pendingTokens` = tokens of **unobserved** messages (after `lastObservedAt`/marker filtering, plus unobserved messages of other threads in resource scope).
- `threshold` = `observation.messageTokens` (default **30,000**) — or a dynamic value when `shareTokenBudget` is on: `max(base, (30k + 40k) − currentObservationTokens)`, so small observation logs let raw history grow up to ~70k [CODE: `thresholds.ts` `calculateDynamicThreshold`; `constants.ts` `OBSERVATIONAL_MEMORY_DEFAULTS`; DOCS "Token budgets"].
- `shouldObserve = pendingTokens >= threshold`; `shouldReflect = observationTokenCount >= reflection.observationTokens` (default **40,000**); `shouldBuffer = pendingTokens < threshold` and the buffer interval boundary (`observation.bufferTokens`, default `0.2` → every ~6k tokens) was crossed; `canActivate = bufferedChunkCount > 0` [CODE: `getStatus`; `constants.ts`; DOCS "Async buffering — Settings"].

Defaults: Observer and Reflector both default to `google/gemini-2.5-flash` (Observer temperature 0.3 with a 215-token thinking budget; Reflector temperature 0 with 1024 thinking budget; `maxOutputTokens` 100k) [CODE: `constants.ts` `OBSERVATIONAL_MEMORY_DEFAULTS`].

### 2.1 Build — the Observer

Triggered on a step where `shouldObserve` and **no tool invocation is in `state: 'call'`** (threshold observation must not run while a tool batch is in flight; mid-loop buffering is allowed but admits only the safe completed prefix) [CODE: `step.ts` `hasIncompleteToolCalls`, `selectSafeBufferPrefix`].

Pipeline (`prepare → observe → process → persist` in the strategy classes; sync path rethrows on failure, buffered path doesn't) [CODE: `observation-strategies/base.ts`, `sync.ts`, `async-buffer.ts`, `resource-scoped.ts`]:

1. **Input**: existing observations (optionally truncated to a `previousObserverTokens` budget) + the new messages formatted into dated lines — each part becomes a line: `User`/`Assistant` text (truncatable), `Tool Call <name>` + JSON args, `Tool Result <name>` (result truncated to ~10k tokens by token estimation; encrypted-content redaction), `Reasoning`, `Image`/`File` placeholders (actual attachments forwarded to the Observer model when allowed via `observeAttachments`, default `image/*, application/pdf`) [CODE: `observer-agent.ts` `formatMessagesForObserver`/`formatObserverMessage`; `tool-result-helpers.ts`; `constants.ts`]. Prior `currentTask`/`suggestedResponse`/`threadTitle`/extracted values from thread metadata are passed in for continuity [CODE: `strategy-sync.ts` `observe`].
2. **Output**: the `<observations>` block (format §1.2) plus `<current-task>` / `<suggested-response>` (+ extractor values).
3. **Persist**: append to `activeObservations` (thread scope: after a new `--- message boundary ---`; resource scope: merge into the thread's `<thread>` section); update `observationTokenCount`, `lastObservedAt`, `observedMessageIds`, thread metadata (including a new `lastObservedMessageCursor`), update the thread title if suggested [CODE: `strategy-sync.ts` `process`/`persist`; `inmemory.ts` `updateActiveObservations`].
4. **Prune the live context**: remove the observed messages using marker-boundary pruning (the last message carrying a completed-observation marker part is the boundary; everything earlier is removed; a partially-observed message keeps only its unobserved parts) or record-cursor fallback; **tool-call/result pairs with pending results are never separated**; messages of the in-flight step are preserved by ID at step 0; the newest observed message is "sealed" (metadata flag) and the response message ID is rotated so subsequent streaming doesn't merge into observed content [CODE: `message-utils.ts` `filterObservedMessages`, `getObservableMessages`; `strategy-sync.ts` `prepare`; `step.ts` `runThresholdObservation`; retention floor `= threshold × (1 − bufferActivation)` — default 0.8 keeps ~20% ≈ 6k tokens — [CODE: `thresholds.ts` `resolveRetentionFloor`, `inmemory.ts` `swapBufferedToActive`]].

### 2.2 Update without blocking — async buffering (enabled by default)

"Observation currently runs synchronously — when the token threshold is hit, the conversation blocks while the Observer processes messages. We've solved this with an async background buffering mode… we're shipping it this week." [BLOG "Current limitations"] — in current code it is the default [CODE: `constants.ts` ("Async buffering defaults (enabled by default)")].

- Every `bufferTokens` interval (~6k tokens) a **fire-and-forget Observer** call runs over the safe completed prefix; its output is stored as a `BufferedObservationChunk` on the record (`observations`, `tokenCount`, `messageIds`, `messageTokens`, `lastObservedAt`, `createdAt`, optional `suggestedContinuation`/`currentTask`/`threadTitle`/`extractedValues`) [CODE: `storage/types.ts` `BufferedObservationChunk`; `buffer()` in `observational-memory.ts`; `inmemory.ts` `updateBufferedObservations`; `buffering-coordinator.ts` (static per-process maps shared across the per-step instances)].
- **Activation** (no LLM call) happens when `pendingTokens >= threshold`, or on idle (`activateAfterIdle`, with `"auto"` mapping to provider prompt-cache TTLs: 5m Anthropic/xAI/OpenRouter and OpenAI gpt-4*/gpt-5*, 1h DeepSeek and other OpenAI models, 2h Groq, 24h Gemini), or on provider/model change (`activateOnProviderChange`). Activation promotes buffered chunks to `activeObservations`, removes their raw messages, and keeps ~20% of the threshold as raw history: chunk selection finds the boundary nearest the target `pending − floor`, biased *over* (with a 95%-of-floor overshoot safeguard and `min(1000, floor)` minimum) [CODE: `inmemory.ts` `swapBufferedToActive`; `thresholds.ts` `calculateProjectedMessageRemoval`; `step.ts`; DOCS "Async buffering"].
- If buffered activation can't happen (no chunks), a **synchronous observation** runs at the threshold; above `blockAfter` (default 1.2× → ~36k) activation may overshoot instead of shrinking [CODE: `step.ts` `runThresholdObservation`; DOCS "How context changes over time"].
- Buffered chunks deliberately skip continuation hints (they'd be stale by activation); activation clears prior `suggestedResponse`/`currentTask` [DOCS "Async buffering — How it works"].

Net behavior with defaults, from Mastra's own description: "raw history oscillates between roughly 6k and 30k tokens. The observation log stays around 40k tokens, however long the conversation runs"; "messages keep getting appended until the threshold is hit — full cache hits on every turn… the observation prefix stays consistent, so you still get a partial cache hit. Only during reflection (infrequent) is the entire cache invalidated." [DOCS "How context changes over time"; BLOG]

### 2.3 Decay — the Reflector

When the observation log reaches `reflection.observationTokens` (default 40k) [CODE: `getStatus`]:

- **Async by default**: at 50% of the threshold (`reflection.bufferActivation: 0.5`) a background Reflector run produces a `bufferedReflection`; at the threshold (or idle TTL / provider change) it is *activated* without an LLM call. Activation replaces only the reflected prefix: `newActive = bufferedReflection + unreflected lines after reflectedObservationLineCount`, committed as a new generation [CODE: `reflector-runner.ts` `maybeReflect`/`tryActivateBufferedReflection`; `inmemory.ts` `swapBufferedReflectionToActive`].
- **Sync path** (blocking; the original design from the blog: "When observations hit 40k tokens (the default threshold, again configurable), a separate 'reflector agent' garbage collects observations that don't matter." [BLOG]): the Reflector rewrites the **entire** observation log [CODE: `reflector-runner.ts` sync path].

The Reflector's job, from its system prompt [CODE: `reflector-agent.ts` `buildReflectorSystemPrompt`]: it receives the Observer's instructions/format for context, then "re-organize and streamline… draw connections and conclusions"; "your reflections are THE ENTIRETY of the assistants memory. Any information you do not add… will be immediately forgotten"; "Condense older observations more aggressively, retain more detail for recent ones"; "Preserve ✅ completion markers"; "USER ASSERTIONS TAKE PRECEDENCE" over later questions; resource scope: consolidate universal cross-thread facts, keep thread attribution where it matters.

The result becomes a **new record generation** (`originType: 'reflection'`, `generationCount + 1`, cursor carried over, buffer state reset) — "Reflections don't accumulate as a separate, ever-growing layer. Each reflection rewrites the entire observation log… Memory stays bounded around the reflection threshold no matter how long the conversation runs." [CODE: `inmemory.ts` `createReflectionGeneration`; DOCS "Reflections" — the "three-tier system": recent messages / observations / reflections].

If the output isn't smaller than the threshold, a **compression ladder** retries with escalating instructions — level 1 "8/10 detail", 2 "6/10", 3 "4/10" (summarize oldest 50–70% into brief paragraphs; drop procedural tool-call details; keep names/dates/decisions/errors/preferences/architecture), 4 "EXTREME: 2/10 detail" ("Tool call observations are the biggest source of bloat. Collapse ALL tool call sequences into outcome-only observations… Never preserve individual tool calls") — accepting when output < threshold or at max level [CODE: `reflector-agent.ts` `COMPRESSION_GUIDANCE`; `reflector-runner.ts` `call` loop, `validateCompression`; unproductive attempts are suppressed while the input is unchanged].

### 2.4 Retrieve

Base mode does **no per-turn dynamic retrieval**: "No per-turn dynamic retrieval based on the user prompt. No dynamic injection. Just a stable log of what happened." [RESEARCH]. Retrieval is opt-in (`retrieval: true` / `{ vector, scope, instructions }`): the agent gets a `recall` tool with `mode: "messages"` (cursor-paginated raw messages behind an observation-group range; low-detail default → `partIndex` high-detail single part → `charOffset` chunking for oversized parts), `mode: "threads"` (list/browse other threads, date filters), and optionally `mode: "search"` (semantic search over observation-group texts, reusing the Memory's vector store — "No vector store needed" for browsing) [CODE: `constants.ts` `getRetrievalInstructions`, `packages/memory/src/tools/om-tools.ts` `recallTool`, `searchMessagesForResource`, `recallMessages`, `recallPart`; DOCS "Retrieval mode"]. Guidance in the prompt: "Default to using recall when the user references specific past content. Your observations capture the gist, not the details." [CODE: `constants.ts`]

### 2.5 Evidence (benchmarks)

Mastra reports LongMemEval (`longmemeval_s`, 500 questions, ~57M tokens of conversations, LLM-judged with the benchmark's question-specific prompts): **84.23%** with gpt-4o actor (beats the "oracle" 82.40% and full-context 60.20%), **93.27%** gemini-3-pro-preview, **94.87%** gpt-5-mini ("highest LongMemEval score on record"), **89.20%** gemini-3-flash — with gemini-2.5-flash as Observer/Reflector ingesting data; the benchmark runner is in-repo at `explorations/longmemeval` (README, src, tests, scripts present at the pinned commit). They attribute the edge to "a completely stable context window… predictable, reproducible, and fully prompt-cacheable" and note competitors (Hindsight, EmergenceMem, Supermemory) use multi-stage retrieval/reranking while OM is single-pass [RESEARCH; BLOG; CODE: `explorations/longmemeval`]. Compression: "For text-only content… 3–6× compression (around 6× in our benchmark runs). For tool-call-heavy agent workloads, we've anecdotally seen 5–40× compression — the noisier the tool output, the higher the ratio." [RESEARCH]. Caveat: these benchmarks measure *recall across long conversation archives*, not coding-task fidelity — the 60.20% full-context baseline is recall on 57M-token archives, not agent task success.

---

## 3. Difference vs summary-based (pi-style) compaction

Pi's compaction, per its own docs [PI → pi-mono `packages/coding-agent/src/core/compaction/compaction.ts`]: auto-compaction triggers when `contextTokens > contextWindow − reserveTokens` (default reserve 16384); it walks back from the newest message to a turn-boundary cut point keeping `keepRecentTokens` (default 20k), has the LLM produce a **structured summary** of the old span (iteratively — the previous summary is passed as input), appends a `CompactionEntry` (`summary` + `firstKeptEntryId`), and rebuilds context as `system + summary + messages from firstKeptEntryId`. A split turn (one turn > budget) yields two merged summaries (history + turn prefix). Cut points are user/assistant/bash/custom messages — "Never cut at tool results (they must stay with their tool call)." `/tree` navigation uses a separate `BranchSummaryEntry`. Compaction/branch-summary calls use fresh session IDs and disable prompt-cache writes (one-off prompts).

Contrast with OM (sources in brackets):

| Dimension | Pi compaction [PI] | OM [CODE/RESEARCH/DOCS] |
|---|---|---|
| Stored artifact | Prose summary of the summarized span ("gist of what happened") | Dated, prioritized **event log**: facts, decisions, state changes, tool outcomes, ✅ completions — "OM is compact event logging for LLM/human actions (with periodic reflection), compaction is bulk unstructured message summarization to avoid context overflow" [RESEARCH "Isn't this compaction?!"] |
| Cadence | One bulk LLM call at near-overflow | Many small incremental cycles (~6k tokens buffered in background; ~24k removed per activation at 5–40× compression) [DOCS "How context changes over time"; RESEARCH] |
| Boundedness | `window − 16k → summary + ~20k` | raw oscillates ~6k↔30k **plus** a bounded ~40k observation log; two independent budgets (shareable) [DOCS] |
| Loss | The summarized span is gone (raw entries stay in the session file, but context only carries the summary) | Raw messages stay in storage; observation groups keep `range="startId:endId"` pointers for `recall` — "the original messages are still available — use the **recall** tool to retrieve them" [CODE: `constants.ts`; DOCS] |
| Decay | Iterative re-summary of ever-growing spans | A dedicated second agent *rewrites the log itself* (GC of observations, not messages) with an escalating 0–4 compression ladder; "A full summarization never runs. Even during reflection, the observation block is only rewritten to find connections, and drop redundant data to save space, not to summarize messages" [CODE: `reflector-agent.ts`; RESEARCH] |
| Continuity | The summary text is all the model gets about old turns | `OBSERVATION_CONTINUATION_HINT` + persisted `currentTask`/`suggestedResponse` metadata + ✅ markers that "tell the assistant what is already resolved and help prevent repeated work" [CODE: `constants.ts`, `reflector-agent.ts`] |
| Caching | Summary changes the whole prefix; one-off calls disable cache writes [PI] | Append-only raw block → full cache hits until threshold; stable observation prefix → partial hits; only reflection invalidates fully [BLOG; DOCS] |
| Shared invariants | Never split a tool call from its result [PI] | Same: observed-pruning preserves pending tool-call/result pairs; observation never runs mid-batch [CODE: `message-utils.ts`, `step.ts`] |

Honest cost delta: OM adds recurring LLM calls (Observer every ~6k tokens, Reflector every ~40k observation tokens) where pi compaction makes one call per overflow; and Mastra's SoTA evidence is on recall benchmarks, not on coding-task outcomes.

---

## 4. Fit to an agent harness compacting a branching session tree with tool calls

**What transfers directly**

- The core is harness-agnostic: a cursor + text log + chunked async observation + thresholded reflection + range-based recall, all behind a storage interface. Proof it runs inside a real coding harness: the `@mastra/opencode` plugin [CODE: `integrations/opencode/src/index.ts`] converts opencode messages **including `tool-invocation` parts, files, images, reasoning** into Mastra's message format, stores records in a project-local SQLite file (LibSQL), initializes a record per `session.created`, and in the `experimental.chat.messages.transform` hook "runs observation if the threshold is met… inject[s] observation summary and filter[s] out already-observed messages," plus registers the recall tool. That is exactly tau's situation: a session tree of turns and tool calls needing compaction at the provider boundary.
- Tool-call handling is first-class: the Observer sees `Tool Call`/`Tool Result` lines (results pre-truncated to ~10k tokens), groups repetitive tool activity into outcome sub-bullets, and the in-batch gate maps to tau's existing turn semantics (a turn = assistant response + its tool-call batch; steering/follow-up are delivered at batch boundaries — the same point where OM defers threshold observation) [CODE: `observer-agent.ts`, `step.ts`; tau CONTEXT.md "Turn", "Steering", "Follow-up"].
- Sub-agents: OM records are per-thread, so a sub-agent session naturally gets its own record; the extracted `currentTask`/`suggestedResponse` pair is a ready-made structured payload for tau's Task/Handoff concept [CODE: `strategy-sync.ts` `persist`; tau CONTEXT.md "Sub-agent", "Task", "Handoff"].

**What assumes Mastra infrastructure (do not port verbatim)**

- The `MemoryStorage` domain (pg/libsql/mongodb/oracledb, multi-record history) → tau has its own session store; the record maps to a row/sidecar per session.
- The thread/**resource** (per-user) model and cross-thread batching (`maxTokensPerBatch`, multi-thread prompt, xxhash-obscured thread IDs, "OTHER conversations" context) → tau v0 scope is one session per agent instance; resource scope is a later, explicitly experimental Mastra feature ("you may need to tweak your system prompt to prevent one thread from continuing the work that another had already started (but hadn't finished)") [DOCS "Scopes"].
- Mastra's `MessageList` buckets, sealed-message + response-ID-rotation machinery, processor pipeline, gateway (server-side OM for mastra-gateway models), provider registry, token-counter with provider heuristics + cached per-part estimates, and the `Subconscious` extras (remind/curate agents, pins, semantic knowledge index — all `@experimental`) [CODE: `processor.ts`, `observation-strategies/`, `subconscious/`, `model-by-input-tokens.ts`, `token-counter.ts`].
- **Branching**: this is the one real gap. Everything in the OM data model — the `lastObservedAt` timestamp cursor, per-message observation markers, `observedMessageIds`, single-scope records — assumes a linear, append-only per-thread timeline; there is no branch/path concept anywhere in the types (the `MastraDBMessage` model is a flat list). Tau sessions are trees that branch in place, so OM has no notion of observing "the path to this leaf." (Pi's own analog is the separate `BranchSummaryEntry` mechanism for `/tree` navigation [PI].)

---

## 5. Recommendation for a native Rust implementation in `tau-core`

Port the **algorithm, port the prompts verbatim, keep the branching semantics a decision of the compaction ADR**. Suggested phases:

**Phase 1 — synchronous OM (v0 core).**
1. `OmRecord` per session in the session store: `active_observations: String`, observed cursor = **path-scoped** `(entry id, timestamp)` pair, `generation: u32`, `observation_tokens`, `pending_tokens`, concurrency flags. (Shape from [CODE: `storage/types.ts`].)
2. Observer: copy `OBSERVER_EXTRACTION_INSTRUCTIONS`, `buildObserverOutputFormat`, `OBSERVER_GUIDELINES`, `OBSERVATION_CONTEXT_PROMPT/INSTRUCTIONS`, `OBSERVATION_CONTINUATION_HINT` verbatim (they are plain text literals [CODE: `observer-agent.ts`, `constants.ts`]); run the configured OpenAI-compatible model at temperature 0.3. Trigger at **turn end** when pending tokens ≥ `messageTokens` (default 30k) and no tool batch is in flight; append output after a `--- message boundary (ISO) ---`; prune observed entries from the next request with the same cut rules pi already enforces (never split a tool pair; cut at entry boundaries); inject the continuation hint.
3. Reflection: at `observation_tokens` ≥ 40k, run the Reflector (temp 0) with the `COMPRESSION_GUIDANCE` ladder (start level 0, escalate to 4); commit as a new generation replacing `active_observations`.
4. Persist `current_task` / `suggested_response` extraction on the session (OM's `<current-task>`/`<suggested-response>` output sections [CODE: `buildObserverOutputFormat`]).
Token accounting needs a stable estimator for thresholding; mastra uses `tokenx` with provider-aware image heuristics and cached per-part estimates [DOCS "Token counting cache"] — for v0 a single estimator (e.g., tiktoken-family) is sufficient since thresholds are soft.

**Phase 2 — async buffering.** Background task per active session; observe the *safe completed prefix* every ~6k tokens (never mid-batch — the natural boundary in tau is exactly the completed tool batch / steering point); store chunks; activate at threshold or idle (fixed timeout in v0, not provider-TTL auto); retention floor math ports directly from `thresholds.ts` (`floor = threshold × (1 − activation)`, boundary selection biased over, `min(1000, floor)` minimum) [CODE: `thresholds.ts`, `inmemory.ts` `swapBufferedToActive`, `step.ts`].

**Phase 3 — retrieval (optional).** Wrap each appended observation in a group with `range = "entryIdFrom:entryIdTo"`; add a `recall` tool paging session entries (low-detail → single-part high-detail → char-offset chunking) [CODE: `om-tools.ts`]. Skip vector search in v0 (tau has no vector store; browsing-only recall is the documented no-vectors mode [DOCS]).

**Branching (the only genuinely new design surface — decide in the compaction ADR):** the simplest consistent rule is *per-branch records*: forking copies the parent's `OmRecord` (observations + cursor) at the fork point; each branch then evolves independently, and observed-cursor pruning is always evaluated on the active path. This mirrors how pi treats branch summarization as a separate entry type rather than corrupting the compaction chain [PI].

**Risks / open questions.** (1) Recurring Observer cost on tool-heavy sessions (mitigated by the 10k-token tool-result truncation before the Observer sees them [CODE: `tool-result-helpers.ts`]). (2) Observer quality is model-dependent; mastra's benchmark numbers used gemini-2.5-flash for observation — expect variance on other OpenAI-compatible endpoints. (3) Branch-scoped observation has no published prior art (mastra threads are linear) — plan for fixture-based tests; mastra keeps a fixture/repro-capture suite under `__fixtures__/repro-captures` that can seed scenarios [CODE: `processors/observational-memory/__fixtures__`, `__tests__`]. (4) Benchmarks evidence recall quality, not coding-task fidelity — measure tau-specific gains (rework rate, token spend, cache hit rate) before/after.

**ADR-worthy consequences** (for the map ticket that ADRs the compaction design after this ticket):
1. Compaction's artifact changes from pi-style `CompactionEntry`-style summaries to a three-tier context (recent raw / observation log / reflected log) — the session-entry vocabulary needs an observation concept, and CONTEXT.md's "Compaction" definition ("built on Observational Memory rather than plain summarization") should be pinned by the ADR.
2. Fork semantics: observation records branch with the session tree (per-branch records, §5 Phase 1 cursor decision).
3. The Task/Handoff contract gains natural structured fields: `currentTask` / `suggestedResponse` as handoff payloads between parent and sub-agent sessions.
4. Prompt-cache strategy: schedule reflection at idle, since it is the only step that fully invalidates the cache [BLOG; DOCS].

---

## Sources

- **[BLOG]** Mastra, "Announcing Observational Memory," <https://mastra.ai/blog/observational-memory> (accessed 2026-09-15).
- **[DOCS]** Mastra docs, "Observational Memory," <https://mastra.ai/docs/memory/observational-memory> (accessed 2026-09-15).
- **[RESEARCH]** Mastra, "Observational Memory: 95% on LongMemEval," <https://mastra.ai/research/observational-memory> (accessed 2026-09-15).
- **[CODE]** `mastra-ai/mastra` @ `5e4ddacde7de44602c099223179a78f544206b0f` (`main`, 2026-09-15):
  - `packages/core/src/storage/types.ts` (`ObservationalMemoryRecord`, `BufferedObservationChunk`, `*Input` types)
  - `packages/core/src/storage/domains/memory/base.ts`, `…/inmemory.ts` (storage contract + reference implementation: `updateActiveObservations`, `updateBufferedObservations`, `swapBufferedToActive`, `createReflectionGeneration`, `swapBufferedReflectionToActive`)
  - `packages/core/src/memory/types.ts` (`ThreadOMMetadata`)
  - `packages/memory/src/processors/observational-memory/`: `constants.ts` (threshold defaults, observation/continuation/retrieval prompts), `observational-memory.ts` (engine: `getStatus`, `buffer`, `activate`, `observe`, `buildContextSystemMessages`, `getUnobservedMessages`, `cleanupMessages`), `processor.ts`, `observation-turn/turn.ts`, `observation-turn/step.ts`, `observation-strategies/{base,sync,async-buffer,resource-scoped}.ts`, `observer-agent.ts` (extraction instructions, output format, guidelines, message formatting), `reflector-agent.ts` (system prompt, compression ladder), `reflector-runner.ts` (`maybeReflect`, retry loop), `observation-groups.ts`, `message-utils.ts`, `thresholds.ts`, `date-utils.ts`, `buffering-coordinator.ts`, `tool-result-helpers.ts`, `subconscious/` (experimental extras)
  - `packages/memory/src/tools/om-tools.ts` (`recall` tool)
  - `integrations/opencode/src/index.ts` (OM inside the opencode coding harness)
  - `explorations/longmemeval/` (benchmark runner)
- **[PI]** pi docs, "Compaction & Branch Summarization," <https://github.com/earendil-works/pi-mono> `packages/coding-agent/src/core/compaction/{compaction,branch-summarization,utils}.ts` (as documented in the installed `@earendil-works/pi-coding-agent` package, `docs/compaction.md`; accessed 2026-09-15).
