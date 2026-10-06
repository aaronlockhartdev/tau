# Research: mastra's async observational memory — cadence, coordination, and failure semantics (design input for tau OM parity)

Researched 2026-10-06. Primary source: `mastra-ai/mastra` @ `main`,
`packages/memory/src/processors/observational-memory/` (files fetched 2026-10-06 and quoted
verbatim below), plus the in-repo user docs `docs/src/content/en/docs/memory/observational-memory.mdx`.
Tau side: `crates/tau-core/src/om.rs`, `om_integration.rs` / `om_integration/turn_end.rs`,
`agent/turn.rs`, `harness/forwarding.rs`, `harness/turn.rs`, `crates/tau-acp/src/pump.rs`,
and the #82 fix commit `63e62d4`. No implementation in this doc — it is the design input for
the parity implementation, which is planned separately. (Issue #85.)

**Question.** How does mastra run its observer *continuously, on token boundaries, in the
background* — what is the trigger math, the coordination state, the retry/failure policy, and
the activation interplay — and how does each mechanism map onto tau's current turn-end-only,
synchronous OM pass, given tau's turn-flag/session-lock discipline, the `calls`/`completed`
stream reconciliation, one-episode-per-turn headless usage, and the ACP settle path?

**Short answer.**

1. **Trigger cadence.** Async observation fires when the unobserved token count crosses a
   *new interval boundary* of `floor(tokens / bufferTokens)` — not a one-shot threshold — and
   only while `pendingTokens < threshold` (the sync range). Near the threshold the interval
   *halves* (a ramp: `effectiveBufferTokens = bufferTokens / 2` once
   `currentTokens >= threshold − 1.1·bufferTokens`). The boundary is the max of a DB-persisted
   value (`lastBufferedAtTokens`) and an in-memory one (`lastBufferedBoundary`).
2. **Coordination.** A process-level coordinator (`BufferingCoordinator` static maps +
   `operation-registry`) dedups in-flight cycles per thread/resource; a DB flag
   (`isBufferingObservation`) survives crashes and is treated as *stale* — and cleared — when
   no matching op is active in the process. A boundary write happens **immediately, before any
   async work**, so a triggered interval never re-triggers.
3. **The async cycle.** It observes the unobserved-minus-already-buffered candidates (filtered
   by a timestamp cursor, minimum `bufferTokens/2` new tokens), seals + persists them, runs the
   Observer under `withRetry`, and appends the result to `bufferedObservationChunks` — it takes
   **no lock, triggers no reflection, and swallows failures** (`rethrowOnFailure: false`),
   persisting a `data-om-observation-failed` marker instead. Cycles chain via the cursor: each
   success advances the buffer cursor, so runs are disjoint.
4. **Retries.** `withRetry`: 8 retries by default, exponential backoff 1s → 2s → … → 120s cap
   with ±20% symmetric jitter, **only for transient transport-class errors** (undici/`UND_ERR_*`
   messages, 408/425/429/5xx, `isRetryable: true` flags, early-finished streams), walking the
   `cause`/`error` chain; aborts are never retried. The underlying model call runs with
   provider-level retries zeroed so `withRetry` is the single retry knob.
5. **Exhausted cycles.** A permanently failed async cycle leaves: a persisted failure marker,
   the buffering flag cleared, the interval boundary already advanced (no immediate re-trigger),
   **no chunk, and the raw material still unobserved** — it is pending for the next trigger
   (a later boundary, the threshold, or turn end). Nothing from the failure reaches the agent
   turn. This is the shape tau must reproduce to keep an exhausted cycle out of the
   `calls`/`completed` reconciliation (the #82 invariant).
6. **Activation.** Promotion of buffered chunks is a pure storage swap (`swapBufferedToActive`)
   with retention-floor math (chunk boundary closest to `pending − floor`, biased over, 95%-of-
   floor overshoot guard, `min(1000, floor)` minimum remainder; `blockAfter` forces
   max-activation). It runs at step 0, in the threshold-observation loop, on idle-TTL /
   provider-change, and at `finalize()`; an in-flight async cycle is awaited (30 s / 60 s
   timeouts) before the swap.
7. **Tau mapping.** Today tau has the *sync* strategy only (verbatim port): one turn-end
   `settle_turn` pass with a `Reflect → Observe → Buffer → Done` plan, in-memory chunks, no
   retries, and the OM call made through the session's `ForwardingProvider`. Parity requires,
   in order of risk: a token-boundary trigger **inside the `run_turn` loop**, background
   cycles under the `with_store` re-lock discipline, **exemption of background calls from the
   `calls`/`completed` reconciliation**, persisted buffered chunks, and a per-trigger-point
   configurable retry count. The turn-end pass retains activation/promote, a final sweep, and
   the reflect decision.

---

## 1. Trigger cadence — `buffering-coordinator.ts`

### 1.1 The enable condition

`buffering-coordinator.ts`, `BufferingCoordinator.isAsyncObservationEnabled`:

```ts
isAsyncObservationEnabled(): boolean {
  return this.observationConfig.bufferTokens !== undefined && this.observationConfig.bufferTokens > 0;
}
```

`bufferTokens` defaults to `0.2` (20% of `messageTokens`) — `constants.ts`:

```ts
// Async buffering defaults (enabled by default)
bufferTokens: 0.2 as number | undefined, // Buffer every 20% of messageTokens
bufferActivation: 0.8 as number | undefined, // Activate to retain 20% of threshold
```

`0.2` is a *ratio of the message threshold*, resolved in `thresholds.ts`,
`resolveBufferTokens`:

```ts
export function resolveBufferTokens(
  bufferTokens: number | false | undefined,
  messageTokens: number | ThresholdRange,
): number | undefined {
  if (bufferTokens === false) return undefined;
  if (bufferTokens === undefined) return undefined;
  if (bufferTokens > 0 && bufferTokens < 1) {
    return Math.round(getMaxThreshold(messageTokens) * bufferTokens);
  }
  return bufferTokens;
}
```

So with the 30k default threshold the interval is 6k tokens.

### 1.2 The interval-boundary math and the ramp (verbatim)

`buffering-coordinator.ts`, `BufferingCoordinator.shouldTriggerAsyncObservation` — the whole
trigger decision, quoted (the trailing `omDebug` call is elided):

```ts
shouldTriggerAsyncObservation(
  currentTokens: number,
  lockKey: string,
  record: ObservationalMemoryRecord,
  storage?: { setBufferingObservationFlag(id: string, flag: boolean): Promise<void> },
  messageTokensThreshold?: number,
): boolean {
  if (!this.isAsyncObservationEnabled()) return false;

  if (record.isBufferingObservation) {
    if (isOpActiveInProcess(record.id, 'bufferingObservation')) return false;
    omDebug(`[OM:shouldTriggerAsyncObs] isBufferingObservation=true but stale, clearing`);
    storage?.setBufferingObservationFlag(record.id, false)?.catch(() => {});
  }

  const bufferKey = this.getObservationBufferKey(lockKey);
  if (this.isAsyncBufferingInProgress(bufferKey)) return false;

  const bufferTokens = this.observationConfig.bufferTokens!;
  const dbBoundary = record.lastBufferedAtTokens ?? 0;
  const memBoundary = BufferingCoordinator.lastBufferedBoundary.get(bufferKey) ?? 0;
  const lastBoundary = Math.max(dbBoundary, memBoundary);

  const rampPoint = messageTokensThreshold ? messageTokensThreshold - bufferTokens * 1.1 : Infinity;
  const effectiveBufferTokens = currentTokens >= rampPoint ? bufferTokens / 2 : bufferTokens;

  const currentInterval = Math.floor(currentTokens / effectiveBufferTokens);
  const lastInterval = Math.floor(lastBoundary / effectiveBufferTokens);

  const shouldTrigger = currentInterval > lastInterval;
  …
  return shouldTrigger;
}
```

Reading the math:

- **Interval boundary, not threshold.** `floor(currentTokens / effectiveBufferTokens) >
  floor(lastBoundary / effectiveBufferTokens)` — the trigger is *crossing into a new
  `bufferTokens` interval since the last buffered boundary*, so a 262k-token episode produces
  ~43 buffer cycles at the default 6k interval, each observing only the delta since the
  previous boundary.
- **The ramp.** `rampPoint = threshold − 1.1·bufferTokens`; at or above it the interval
  *halves* (`bufferTokens / 2`), so the final ~1.1 intervals before the sync threshold are
  buffered at double cadence — the last of the raw window gets observed before it can pile up
  against the threshold. With the defaults: ramp at 30k − 6.6k = 23.4k, then 3k-token
  intervals.
- **Boundary = max(DB, memory).** `lastBoundary = Math.max(dbBoundary, memBoundary)`: the DB
  value survives restarts, the in-memory value is fresher within a process.
- **Guards first.** A live in-process op, or a DB flag with a live op, short-circuits to
  `false` before any boundary math; a DB flag *without* a live in-process op is stale (crashed
  writer) and is cleared inline (see §3.4).

### 1.3 Where the check is called, and how sync and async partition the range

`observational-memory.ts`, `getStatus` — the buffer trigger only runs **below** the sync
threshold:

```ts
// Should buffer? Check interval boundary using DB-backed state
const asyncObservationEnabled = this.buffering.isAsyncObservationEnabled();
let shouldBuffer = false;
if (asyncObservationEnabled && pendingTokens < threshold) {
  const lockKey = this.buffering.getLockKey(threadId, resourceId);
  shouldBuffer = this.buffering.shouldTriggerAsyncObservation(
    pendingTokens,
    lockKey,
    record,
    this.storage,
    threshold,
  );
}

// Should observe?
const shouldObserve = pendingTokens >= threshold;
```

**When both are configured, they partition the token range**: below `threshold`, only the
interval-boundary async buffer can fire; at/above `threshold`, the sync path owns the step.
The sync path is never suppressed by async buffering — `observation-turn/step.ts`,
`runThresholdObservation` documents the interplay:

```ts
/**
 * Run the full threshold observation pipeline:
 * waitForBuffering → re-check → activate buffered chunks → reflect → observe
 * (sync fallback when pending tokens are still at or above the threshold)
 */
```

and, after the activation loop:

```ts
// Sync observation — we've waited for buffering and activated what we could;
// we're still above threshold, so observe the remaining messages synchronously.
```

i.e. at the threshold, mastra first activates whatever buffered chunks exist (cheap, no LLM
call), and runs the blocking sync observer only on the *remainder* that activation did not
bring back under threshold. The user docs state the same contract
(`observational-memory.mdx`, "Async buffering"):

> A synchronous observation runs when the `messageTokens` threshold is reached and activating
> buffered chunks doesn't bring pending tokens back under it. For example, this happens when a
> large tool result arrives after buffering has stopped.

## 2. Per-step lifecycle — `processor.ts`, `observation-turn/`

### 2.1 Where in the agent loop the check runs

The OM engine is wired to the agent as a *processor pair*: `processor.ts`,
`ObservationalMemoryProcessor.processInputStep` runs before every LLM call and
`processOutputResult` after the turn's final output. The turn is created once on the first
step and stashed in the shared processor-state map:

```ts
// ── Create turn on first step (or when state is reset) ──
// The turn is stashed in customState so that the output processor instance
// (which is a separate ObservationalMemoryProcessor) can retrieve it in
// processOutputResult. In production, getInputProcessors() and
// getOutputProcessors() each call createOMProcessor(), producing two
// different instances that share only the processorStates map.
```

Each step then prepares through the Turn/Step handles:

```ts
// ── Run step preparation (activation, threshold, observation, filtering) ──
{
  const step = this.turn.step(stepNumber);
  let ctx;
  try {
    ctx = await step.prepare();
  } catch (error) {
    …
  }
  // Inject system messages (one per cache-stable chunk) + continuation
  injectObservationContextMessages({ … });
  …
}
```

`observation-turn/turn.ts` documents the step/turn split:

```ts
/**
 * Represents a single turn in the agent conversation — one user message → agent response cycle.
 *
 * The turn manages record caching, context loading, and step lifecycle.
 * Create via `om.beginTurn(...)`, then call `start()` to load context,
 * `step(n)` to create steps, and `end()` to finalize.
 *
 * @example
 * ```ts
 * const turn = om.beginTurn({ threadId, resourceId, messageList });
 * await turn.start(memory);
 *
 * const step0 = turn.step(0);
 * const ctx = await step0.prepare();
 * // ... agent generates ...
 *
 * const step1 = turn.step(1);  // finalizes step 0
 * const ctx1 = await step1.prepare();
 * // ... agent generates ...
 *
 * await turn.end();  // finalizes last step, cleanup
 * ```
 */
```

So in mastra's agent loop the OM check point is **once per step** (a step = one
model-generate round inside the multi-step tool loop), *before* the model generates —
`step.prepare()` decides activate / buffer / observe and rewrites the message list before the
prompt is built.

### 2.2 What exactly happens each step — `observation-turn/step.ts`, `ObservationStep.prepare`

The per-step sequence (abridged to the OM-relevant decisions; all quoted from `prepare()`):

1. **Step 0 only — activation + sync reflection check:**

   ```ts
   // ── Step 0: Activate buffered chunks ──────────────────────
   if (this.stepNumber === 0) {
     const step0Messages = getObservableMessages(messageList);
     const activation = await om.activate({ … checkThreshold: true, … });
     …
     // Check if reflection is needed (whether or not activation happened).
     // maybeReflect handles both sync (above full threshold) and async buffered
     // reflection (above bufferActivation point but below full threshold).
     …
     await om.reflector.maybeReflect({ … trigger: 'turn-sync' });
   ```

2. **Pending-tool-call check** — threshold (sync) observation must not run while a tool call
   is still `state: 'call'` on the newest message; mid-loop buffering is *not* blocked, it
   admits only the safe completed prefix:

   ```ts
   // Tool calls (provider- or client-executed) may still be in state:'call'
   // while the agent loop continues. Threshold observation must not run until
   // they complete. Mid-loop buffering is NOT blocked here — it admits only
   // the safe completed prefix via selectSafeBufferPrefix below.
   ```

3. **All steps — status snapshot + interval-boundary buffer trigger (fire-and-forget):**

   ```ts
   // Trigger buffering if interval boundary crossed (fire-and-forget, all steps).
   // A pending tool call on the newest message doesn't block the whole batch:
   // admit only the safe prefix before it — the same policy idle buffering
   // applies at turn end (see selectSafeBufferPrefix).
   if (statusSnapshot.shouldBuffer) {
     const allMessages = getObservableMessages(messageList);
     const unobservedMessages = om.getUnobservedMessages(allMessages, statusSnapshot.record);
     …
     const candidates = om.getUnobservedMessages(unobservedMessages, statusSnapshot.record, {
       excludeBuffered: true,
     });
     const safeCandidates = selectSafeBufferPrefix(candidates);
     // Deferred = there were candidates but none can be buffered yet. Skip buffer()
     // entirely so the interval boundary isn't advanced and the next step retries.
     const deferred = candidates.length > 0 && safeCandidates.length === 0;
     if (safeCandidates.length > 0) {
       om.sealMessagesForBuffering(safeCandidates);
       …
       if (this.turn.memory) {
         await this.turn.memory.persistMessages(safeCandidates);
       }
       …
     }

     if (!deferred) {
       void om.trackBackgroundWork(
         om
           .buffer({
             threadId: this.threadId,
             resourceId: this.resourceId,
             messages: safeCandidates,
             pendingTokens: statusSnapshot.pendingTokens,
             record: statusSnapshot.record,
             …
           })
           .catch((err: Error) => {
             omDebug(`[OM:buffer] fire-and-forget buffer failed: ${err?.message}`);
           }),
       );
       buffered = true;
     }
   }
   ```

   Note the ordering contract in the comment above it: sealing/rotation/persistence of the
   buffer candidates happens **synchronously** before the fire-and-forget `buffer()` call, so
   the in-flight response message can't be merged into sealed history.

4. **All steps (and step 0 when observation is imminent) — sync threshold observation:**

   ```ts
   const willObserveNow = statusSnapshot.shouldObserve && !hasIncompleteToolCalls;
   …
   // Threshold observation (skip if tool calls pending)
   if (willObserveNow) {
     const preObsGeneration = this.turn.record.generationCount;
     const obsResult = await this.runThresholdObservation();
     …
   }
   ```

### 2.3 Turn end — `observation-turn/turn.ts`, `ObservationTurn.end`

`end()` persists unsaved messages and, with `bufferOnIdle`, starts one final background
buffer for anything unobserved, so short idle turns are observed proactively:

```ts
// When the agent goes idle, start buffering any unobserved messages in the background.
// This ensures messages accumulated during the turn are observed proactively
// rather than waiting for the next turn's step.prepare() to trigger buffering.
const asyncObservationEnabled = this.om.buffering.isAsyncObservationEnabled();
const bufferOnIdle = this.om.getObservationConfig().bufferOnIdle;
if (asyncObservationEnabled && bufferOnIdle) {
  const allMessages = getObservableMessages(this.messageList);
  const record = this._record!;
  const unobservedMessages = this.om.getUnobservedMessages(allMessages, record);
  // Buffer only the safe prefix before a tool call still pending on the newest
  // message; defer when the cut before it is unsafe (see selectSafeBufferPrefix).
  const idleMessages = selectSafeBufferPrefix(unobservedMessages);
  if (idleMessages.length > 0) {
    void this.om.trackBackgroundWork(
      this.om
        .buffer({ … skipMinimumTokenCheck: true })
        .catch((err: Error) => {
          omDebug(`[OM:turn.end] idle buffer failed: ${err?.message}`);
        }),
    );
  }
}
```

`bufferOnIdle` is **off by default** (`observational-memory.ts` constructor:
`bufferOnIdle: config.observation?.bufferOnIdle ?? false`); the docs:

> `bufferOnIdle` is off by default. It's separate from `bufferTokens`: `bufferTokens`
> controls step-time async buffering, while `bufferOnIdle` controls end-of-turn buffering for
> idle turns.

### 2.4 "Multiple OM instances per agent loop step" — what it implies for state sharing

`buffering-coordinator.ts`, class doc:

```ts
/**
 * Manages the static buffering state machine for async observation and reflection.
 *
 * Static maps are shared across all ObservationalMemory instances in a process.
 * This is critical because multiple OM instances are created per agent loop step,
 * and they need to share knowledge of in-flight operations.
 */
export class BufferingCoordinator {
  /**
   * Track in-flight async buffering operations per resource/thread.
   * Key format: "obs:{lockKey}" or "refl:{lockKey}"
   * Value: Promise that resolves when buffering completes
   */
  static asyncBufferingOps = new Map<string, Promise<void>>();

  /**
   * Track the last token boundary at which we started buffering.
   * Key format: "obs:{lockKey}" or "refl:{lockKey}"
   */
  static lastBufferedBoundary = new Map<string, number>();
  …
```

The consequence: **any per-thread in-flight knowledge that must survive the creation of a
fresh OM instance per step must live in process-level (static) state, not instance state** —
and the durable half (the boundary, the flag) must also be in the DB record, because a new
*process* (restart, gateway) only sees the DB. This is the three-tier state discipline:
in-memory static map (fast, same process) → DB record field (survives restart) → in-process
op registry (crash-staleness detection). Tau's equivalent must make the same three-way split
(§9.2).

## 3. Coordinator semantics

### 3.1 In-flight dedup — `asyncBufferingOps` with mutex-wait chaining

`observational-memory.ts`, `buffer()` — a new trigger while a cycle is in flight **waits for
the in-flight one** rather than spawning in parallel:

```ts
// Check if another process is already buffering (and the op is genuinely active)
if (record.isBufferingObservation && isOpActiveInProcess(record.id, 'bufferingObservation')) {
  return { buffered: false, record };
}
…
// Set lastBufferedBoundary IMMEDIATELY (before ANY async work) to prevent
// shouldTriggerAsyncObservation from triggering again on the next step.
// This MUST happen before the first await when buffer() is called fire-and-forget.
BufferingCoordinator.lastBufferedBoundary.set(bufferKey, currentTokens);

// Clear stale flag if it was set by a crashed process (non-blocking)
if (record.isBufferingObservation) {
  await this.storage.setBufferingObservationFlag(record.id, false).catch(() => {});
}

// Wait for any existing buffering operation to complete first (mutex behavior).
// IMPORTANT: read the existing op BEFORE overwriting the map entry.
const existingOp = BufferingCoordinator.asyncBufferingOps.get(bufferKey);
if (existingOp) {
  try {
    await existingOp;
  } catch {
    // Previous op failed, continue with new one
  }
}

// Set persistent flag and register op
registerOp(record.id, 'bufferingObservation');
inMemoryRecord.isBufferingObservation = true;
inMemoryRecord.lastBufferedAtTokens = currentTokens;
this.storage.setBufferingObservationFlag(record.id, true, currentTokens).catch(err => {
  omError('[OM] Failed to set buffering observation flag', err);
});

// Register in asyncBufferingOps so callers (and tests) can await completion
let resolveOp: () => void;
const opPromise = new Promise<void>(resolve => {
  resolveOp = resolve;
});
BufferingCoordinator.asyncBufferingOps.set(bufferKey, opPromise);
```

The `finally` unwinds all three:

```ts
} finally {
  unregisterOp(record.id, 'bufferingObservation');
  BufferingCoordinator.asyncBufferingOps.delete(bufferKey);
  resolveOp!();
  // Only clear the flag if the success path didn't already clear it (with token count)
  if (!flagCleared) {
    setBufferingState(false);
    await this.storage.setBufferingObservationFlag(record.id, false).catch(() => {});
  }
}
```

So at most **one in-flight async observation cycle per thread**, and triggers during that
window are dropped (the boundary was already advanced, so they wouldn't re-fire anyway).

### 3.2 Boundary state in DB vs memory

Two cursors, each two-tier:

- **Token boundary** (`lastBufferedAtTokens` in the record / `lastBufferedBoundary` static
  map) — drives `shouldTriggerAsyncObservation` (§1.2). The DB copy is written with the flag
  (`setBufferingObservationFlag(record.id, true, currentTokens)`); the memory copy is written
  *immediately at trigger* so a same-step re-check can't double-fire.
- **Time cursor** (`lastBufferedAtTime` in the record / `lastBufferedAtTime` static map) —
  drives *candidate selection* so cycles never re-buffer the same messages.
  `buffer()`:

  ```ts
  let bufferCursor = BufferingCoordinator.lastBufferedAtTime.get(bufferKey) ?? record.lastBufferedAtTime ?? null;

  // Advance the cursor if lastObservedAt is newer (e.g. sync observation ran after the last buffer)
  if (freshRecord.lastObservedAt) {
    const lastObserved = new Date(freshRecord.lastObservedAt);
    if (!bufferCursor || lastObserved > bufferCursor) {
      bufferCursor = lastObserved;
    }
  }

  // Filter messages to only those newer than the buffer cursor.
  // This prevents re-buffering messages that were already included in a previous chunk.
  let candidateMessages = this.getUnobservedMessages(unobservedMessages, freshRecord, {
    excludeBuffered: true,
  });
  …
  if (bufferCursor) {
    candidateMessages = candidateMessages.filter(msg => {
      if (!msg.createdAt) return true; // include messages without timestamps
      return new Date(msg.createdAt) > bufferCursor;
    });
  }
  ```

  and on success the cursor advances past the buffered batch
  (`runAsyncBufferedObservation`):

  ```ts
  if (result?.observed) {
    // Update the buffer cursor so the next buffer only sees messages newer than this one.
    const maxTs = this.getMaxMessageTimestamp(messagesToBuffer);
    const cursor = new Date(maxTs.getTime() + 1);
    BufferingCoordinator.lastBufferedAtTime.set(bufferKey, cursor);
  }
  ```

  (tau's existing `buffer_cursor` in `om_integration.rs` is the direct port of this:
  "The last entry a buffered run covered (mastra's buffer cursor, the port's
  `lastBufferedAtTokens`)".)

### 3.3 The operation registry

`operation-registry.ts`, in full (36 lines) — a process-level ref-count of which ops are
*actually running in this process*:

```ts
/**
 * Process-level operation registry for Observational Memory.
 *
 * Tracks which operations (reflecting, observing, buffering) are actively running
 * in THIS process. Used to detect stale DB flags left by crashed processes.
 *
 * Key format: `${recordId}:${operationType}`
 */
export type OmOperationName = 'reflecting' | 'observing' | 'bufferingObservation' | 'bufferingReflection';

const activeOps = new Map<string, number>();
…
export function isOpActiveInProcess(recordId: string, op: OmOperationName): boolean {
  return (activeOps.get(opKey(recordId, op)) ?? 0) > 0;
}
```

`registerOp`/`unregisterOp` are ref-counted (`+1` / decrement-and-delete) so concurrent
cycles on the same record don't un-register each other.

### 3.4 Stale-flag cleanup and crash recovery

The crash-recovery contract, in the three places it is enforced:

1. **At trigger** — `shouldTriggerAsyncObservation` (§1.2): DB flag set, op not in-process ⇒
   stale, clear it and continue:

   ```ts
   if (record.isBufferingObservation) {
     if (isOpActiveInProcess(record.id, 'bufferingObservation')) return false;
     omDebug(`[OM:shouldTriggerAsyncObs] isBufferingObservation=true but stale, clearing`);
     storage?.setBufferingObservationFlag(record.id, false)?.catch(() => {});
   }
   ```

2. **At cycle start** — `buffer()` (§3.1): the same stale check, clearing non-blocking before
   waiting on any in-process op.

3. **At activation** — `activate()` treats a foreign (other-replica) flag as "proceed with
   whatever chunks exist":

   ```ts
   // Wait for any in-progress buffering to complete (check DB flag)
   if (record.isBufferingObservation) {
     // If the op is active in this process, wait for it
     const lockKey = this.buffering.getLockKey(threadId, resourceId);
     const bufferKey = this.buffering.getObservationBufferKey(lockKey);
     const asyncOp = BufferingCoordinator.asyncBufferingOps.get(bufferKey);
     if (asyncOp) {
       try {
         await Promise.race([
           asyncOp,
           new Promise((_, reject) => setTimeout(() => reject(new Error('Timeout')), 60_000)),
         ]);
       } catch {
         // Timeout or error — proceed with what we have
       }
     }
     // If not in this process, the flag might be stale or from another replica.
     // Proceed with activation of whatever chunks exist.
   }
   ```

After activation, `resetBufferingState` zeroes the boundary and clears the flag so the next
buffer interval starts fresh (`observational-memory.ts`):

```ts
async resetBufferingState(opts: {
  threadId: string;
  resourceId?: string;
  recordId: string;
  activatedMessageIds?: string[];
}): Promise<void> {
  const { threadId, resourceId, recordId, activatedMessageIds } = opts;
  const lockKey = this.buffering.getLockKey(threadId, resourceId);
  const bufKey = this.buffering.getObservationBufferKey(lockKey);

  BufferingCoordinator.lastBufferedBoundary.set(bufKey, 0);
  await this.storage.setBufferingObservationFlag(recordId, false, 0).catch(() => {});

  if (activatedMessageIds && activatedMessageIds.length > 0) {
    this.buffering.cleanupStaticMaps(threadId, resourceId, activatedMessageIds);
  }
}
```

`cleanupStaticMaps` (buffering-coordinator.ts) distinguishes partial (post-activation: clear
only the boundary/time state "so the next buffer cycle isn't suppressed") from full cleanup
(thread teardown: clear ops, boundary, time, and reflection cycle ids).

**Crash-recovery summary.** A process that dies mid-cycle leaves the DB flag set and the DB
boundary advanced. The next process: (a) sees flag + no in-process op ⇒ stale ⇒ clears it and
may start a new cycle; (b) the advanced DB boundary means the *killed* interval is not
re-triggered — the unobserved material it was supposed to cover is simply pending for the next
interval/threshold; (c) no chunk was written (persist happens after the observer call), so
there is no partial-chunk state to repair.

## 4. The async observation cycle

### 4.1 What a cycle observes

Candidate selection (`buffer()`, quoted in §3.2) is: unobserved messages, minus messages
already covered by a buffered chunk (`excludeBuffered: true`), minus everything at/before the
time cursor, with a **minimum content check** so a boundary crossing on a trickle doesn't burn
an LLM call:

```ts
// Check minimum token threshold
const bufferTokens = this.observationConfig.bufferTokens ?? 5000;
const minNewTokens = bufferTokens / 2;
const newTokens = await this.tokenCounter.countMessagesAsync(candidateMessages);

if (candidateMessages.length === 0 || (!opts.skipMinimumTokenCheck && newTokens < minNewTokens)) {
  setBufferingState(false);
  return { buffered: false, record };
}
```

(`skipMinimumTokenCheck` is the `bufferOnIdle` escape hatch, which "observe any non-empty
candidate set".) Before the observer runs, the candidates are **sealed and persisted
synchronously** — `runAsyncBufferedObservation`:

```ts
// Seal the messages being buffered to prevent new parts from being added.
// This ensures that any streaming content after this point goes to new messages,
// preserving the boundary of what we're buffering.
this.sealMessagesForBuffering(messagesToBuffer);

// CRITICAL: Persist the sealed messages to storage immediately.
// This ensures that:
// 1.  The seal metadata (sealedAt on last part) is saved to the database
// 2.  When MessageList creates new messages for streaming content after the seal,
//     those new messages have their own IDs and don't overwrite the sealed messages
// 3.  The sealed messages remain intact with their content at the time of buffering
try {
  await this.messageHistory.persistMessages({
    messages: messagesToBuffer,
    threadId,
    resourceId: freshRecord.resourceId ?? undefined,
  });
} catch (err) {
  omError('[OM] Failed to persist sealed messages before buffering — skipping observation cycle', err);
  return;
}
```

### 4.2 Where the output lands

`observation-strategies/async-buffer.ts`, `AsyncBufferObservationStrategy.persist` — the
result is a **chunk in `bufferedObservationChunks`**, not the active log; the write itself is
retried:

```ts
const messageTokens = await this.tokenCounter.countMessagesAsync(messages);
await withRetry(
  () =>
    this.storage.updateBufferedObservations({
      id: record.id,
      chunk: {
        cycleId: this.cycleId,
        observations: processed.observations,
        tokenCount: processed.observationTokens,
        messageIds: processed.observedMessageIds,
        messageTokens,
        lastObservedAt: processed.lastObservedAt,
        …
      },
      lastBufferedAtTime: processed.lastObservedAt,
    }),
  { label: 'persist-buffered-observations', abortSignal: this.opts.abortSignal },
);
```

Empty observer output is a no-op that stores nothing:

```ts
if (!output.observations) {
  omDebug(`[OM:asyncBuffer] empty observations returned, skipping buffer storage`);
  return {
    observations: '',
    observationTokens: 0,
    cycleObservationTokens: 0,
    observedMessageIds: [],
    lastObservedAt: new Date(),
  };
}
```

### 4.3 The strategy flags and their consequences

`observation-strategies/async-buffer.ts`:

```ts
get needsLock() {
  return false;
}
get needsReflection() {
  return false;
}
get rethrowOnFailure() {
  return false;
}
```

against the sync strategy (`observation-strategies/sync.ts`), which has all three `true`.
The base runner (`observation-strategies/base.ts`, `ObservationStrategy.run`) interprets them:

- `needsLock: false` — the cycle skips the freshness guard that sync uses
  (`if (this.needsLock) { … if (fresh.lastObservedAt > record.lastObservedAt) return
  { observed: false }; }`): a buffered cycle is *expected* to run against a record that other
  activity has moved on from; correctness comes from the cursor/excludeBuffered filtering,
  not a lock.
- `needsReflection: false` — no `reflector.maybeReflect` at the end of the cycle; reflection
  on buffered material happens only in the async-reflection lane (§5) or at activation.
- `rethrowOnFailure: false` — from `base.ts`, `run()`:

  ```ts
  } catch (error) {
    await this.emitFailedMarkers(cycleId, error);

    if (!this.rethrowOnFailure) {
      const failedMarkerForStorage = {
        type: 'data-om-observation-failed',
        data: {
          cycleId,
          operationType: 'observation',
          startedAt: new Date().toISOString(),
          error: formatOmError(error),
          …
          recordId: record.id,
          threadId,
        },
      };
      await this.persistMarkerToStorage(failedMarkerForStorage, threadId, this.opts.resourceId).catch(() => {});
      if (abortSignal?.aborted) throw error;
      omError('[OM] Observation failed', error);
      return { observed: false, error: error instanceof Error ? error : new Error(String(error)) };
    }
    …
    throw error;
  }
  ```

  **Consequence: a failed background cycle is a dead end by design** — it emits a failed
  marker, logs, and returns `{ observed: false }`; the caller (`buffer()`) then runs its
  `finally` (flag cleared, op unregistered) and the unobserved material simply stays pending.
  Nothing propagates to the agent turn. (Contrast: the sync strategy rethrows, and
  `failurePolicy: 'continue'` is the only sync path that swallows — see §6.5.)

### 4.4 How cycles chain across steps

The chain is exactly the two cursors: each *successful* cycle (a) advances the time cursor
past its batch (§3.2), (b) writes the new token boundary to the DB
(`setBufferingObservationFlag(record.id, false, newTokens)` — flag off, boundary = the tokens
just buffered), and (c) appends a chunk with a `lastObservedAt`/`messageIds` stamp. The next
boundary crossing selects only messages newer than the cursor. A *failed* cycle advances
neither (the boundary was set at trigger, so the interval is consumed, but the time cursor and
`lastObservedAt` are untouched — the material remains unobserved and re-eligible).

## 5. Async reflection — `reflector-runner.ts`

### 5.1 Enablement and trigger point

`buffering-coordinator.ts`:

```ts
isAsyncReflectionEnabled(): boolean {
  return this.reflectionConfig.bufferActivation !== undefined && this.reflectionConfig.bufferActivation > 0;
}
```

(default `reflection.bufferActivation: 0.5`, §constants). The trigger, in
`reflector-runner.ts`, `ReflectorRunner.maybeReflect` — reflection buffers *early*, at a
fraction of the full reflection threshold:

```ts
// ════════════════════════════════════════════════════════════════════════════
// ASYNC BUFFERING: Trigger background reflection at bufferActivation ratio
// ════════════════════════════════════════════════════════════════════════════
if (this.buffering.isAsyncReflectionEnabled() && observationTokens < reflectThreshold) {
  const shouldTrigger = (() => {
    if (!this.buffering.isAsyncReflectionEnabled()) return false;
    if (record.isBufferingReflection) {
      if (isOpActiveInProcess(record.id, 'bufferingReflection')) return false;
      omDebug(`[OM:shouldTriggerAsyncRefl] isBufferingReflection=true but stale, clearing`);
      this.storage.setBufferingReflectionFlag(record.id, false).catch(() => {});
    }
    const bufferKey = this.buffering.getReflectionBufferKey(lockKey);
    if (this.buffering.isAsyncBufferingInProgress(bufferKey)) return false;
    if (BufferingCoordinator.lastBufferedBoundary.has(bufferKey)) return false;
    if (record.bufferedReflection) return false;
    const activationPoint = reflectThreshold * this.reflectionConfig.bufferActivation!;
    return observationTokens >= activationPoint;
  })();
  if (shouldTrigger) {
    this.startAsyncBufferedReflection( … );
  }
}
```

Note the asymmetry with observation: reflection buffering has **no interval math** — it is a
one-shot-per-threshold-window trigger (suppressed while a boundary is set, an op is in
flight, or a `bufferedReflection` already exists), at `observationTokens >= 0.5 ·
reflectThreshold`.

### 5.2 What gets buffered

`doAsyncBufferedReflection` reflects only the *oldest prefix* of the active observations, up
to the activation point, and stores it as `bufferedReflection` — **not** a new generation,
**not** the active log:

```ts
/**
 * Perform async buffered reflection — reflects observations and stores to bufferedReflection.
 * Does NOT create a new generation or update activeObservations.
 */
…
const bufferActivation = this.reflectionConfig.bufferActivation ?? 0.5;
…
const fullObservations = currentRecord.activeObservations ?? '';
const allLines = fullObservations.split('\n');
const totalLines = allLines.length;

const avgTokensPerLine = totalLines > 0 ? observationTokens / totalLines : 0;
const activationPointTokens = reflectThreshold * bufferActivation;
const linesToReflect =
  avgTokensPerLine > 0 ? Math.min(Math.floor(activationPointTokens / avgTokensPerLine), totalLines) : totalLines;
```

`startAsyncBufferedReflection` follows the same coordination pattern as observation:
boundary set immediately, `registerOp(record.id, 'bufferingReflection')` +
`setBufferingReflectionFlag(record.id, true)`, op registered in `asyncBufferingOps`, and in
the `finally` the flag/op are cleared — with one distinctive failure rule:

```ts
omError('[OM] Async buffered reflection failed', error);
// Clear the boundary so a failed reflection doesn't permanently block
// future async reflection attempts.
BufferingCoordinator.lastBufferedBoundary.delete(bufferKey);
```

(a failed *reflection* deletes the boundary so the next turn retries; a failed
*observation* keeps the advanced boundary so the interval is not re-fought — both are
deliberate.)

### 5.3 Buffered vs committed — `tryActivateBufferedReflection`

When the threshold (or TTL / provider change) is reached, `maybeReflect` first tries to
*activate* the buffered reflection before running any sync reflection:

```ts
// ════════════════════════════════════════════════════════════════════════════
// ASYNC ACTIVATION: Try to activate buffered reflection first
// ════════════════════════════════════════════════════════════════════════════
if (this.buffering.isAsyncReflectionEnabled()) {
  const activationResult = await this.tryActivateBufferedReflection( … );
  if (activationResult.status === 'activated') {
    return;
  }
  …
  if (activationResult.status === 'suppressed') { … return; }
  …
}
```

Activation composes `bufferedReflection` with the *unreflected tail* and swaps in one shot
(`swapBufferedReflectionToActive`), with early-trigger guards so a TTL fire right after a
buffer was written can't collapse the log to "almost just the reflection" (a composition
floor — unreflected tail ≥ buffered reflection — and a size floor at 75% of the regular
activation target, else `suppressed`). An in-flight buffering op is waited on with a **5 s**
timeout for threshold triggers, and *not* waited on for TTL/provider triggers:

```ts
// TTL and provider-change triggers should not block on in-progress
// reflection buffering. The async op will finish in the background
// and the buffered result will be available for activation on the next turn.
```

### 5.4 `reflectionBufferCycleIds`

`BufferingCoordinator.reflectionBufferCycleIds` (keyed `refl:{lockKey}`) records the cycle id
of the in-flight buffered reflection at start (`doAsyncBufferedReflection`:
`BufferingCoordinator.reflectionBufferCycleIds.set(_bufferKey, cycleId)`), deleted at
activation. Its sole job is marker identity: the activation marker emitted at commit reuses
the *original buffering* cycle id so the UI can correlate the activation with the buffered
cycle that produced it.

## 6. Retry and failure policy — `retry.ts`

### 6.1 The schedule (verbatim)

`retry.ts`, `RETRY_CONFIG` and its doc:

```ts
/**
 * Retry knobs for the internal OM transport-error retry wrapper.
 * Exported as a mutable object so tests can shrink the backoff schedule
 * without changing public API.
 *
 * With the defaults the per-retry pre-jitter backoff schedule is:
 *   1s, 2s, 4s, 8s, 16s, 32s, 64s, 120s (cap)
 * giving 8 retries / 9 total attempts and ~247s (~4 minutes) of waiting
 * before the final attempt fails. Designed to ride out short provider /
 * network blips without holding the actor turn for much longer than that.
 *
 * @internal
 */
export const RETRY_CONFIG = {
  /** Maximum number of retry *attempts* (total tries = maxRetries + 1). */
  maxRetries: 8,
  /** Initial backoff delay in milliseconds. */
  initialDelayMs: 1_000,
  /** Multiplier applied to the delay after each failed attempt. */
  backoffFactor: 2,
  /** Cap on per-attempt delay (ms). */
  maxDelayMs: 120_000,
  /** Random jitter as a fraction of the computed delay (e.g. 0.2 = ±20%). */
  jitter: 0.2,
};
```

Delay computation:

```ts
export function computeDelay(attempt: number): number {
  const base = RETRY_CONFIG.initialDelayMs * Math.pow(RETRY_CONFIG.backoffFactor, attempt);
  const capped = Math.min(base, RETRY_CONFIG.maxDelayMs);
  if (RETRY_CONFIG.jitter <= 0) return capped;
  const jitterRange = capped * RETRY_CONFIG.jitter;
  // Symmetric jitter in [-jitterRange, +jitterRange].
  const offset = (Math.random() * 2 - 1) * jitterRange;
  return Math.max(0, Math.round(capped + offset));
}
```

Per-stage override already exists in the config (`types.ts`, `ObservationConfig`):

```ts
/** Number of retries after the initial Observer model call. @default 8 */
maxRetries?: number;

/** Terminal policy after Observer model retries are exhausted. @default 'abort' */
failurePolicy?: 'abort' | 'continue';
```

and the constructor resolves `maxRetries: config.observation?.maxRetries ?? RETRY_CONFIG.maxRetries`
(same for reflection) — so **mastra's retry count is already a per-stage (per-trigger-point)
configurable knob**; the user decision for tau (per-trigger-point configurable count) adopts
the same shape, extended to all four of tau's trigger points.

### 6.2 Which errors are retried

`retry.ts` — transient classification walks the `cause`/`error` wrapper chain:

```ts
const TRANSIENT_MESSAGE_SUBSTRINGS = [
  'terminated',
  'fetch failed',
  'econnreset',
  'econnrefused',
  'enotfound',
  'eai_again',
  'socket hang up',
  'network error',
  'request timed out',
  'request timeout',
  'connection reset',
  'connection closed',
  // Core raises this when a stream closes with finishReason 'other' before any output.
  'finished with finishreason "other"',
];

const INCOMPLETE_FINISH_REASONS = new Set(['other', 'unknown']);
```

```ts
function isRetryableHttpStatus(status: number): boolean {
  if (status === 408 || status === 425 || status === 429) return true;
  return status >= 500 && status <= 599;
}
```

```ts
/**
 * Returns true when the given error looks like a transient transport-class
 * failure that's worth retrying — undici `terminated`, `fetch failed`,
 * `UND_ERR_*` codes, AI SDK `APICallError` with `isRetryable: true`, and
 * common HTTP 408/425/429/5xx statuses. Walks the `error.cause` chain so
 * wrapper errors don't hide the real failure.
 *
 * Never retries on user-initiated aborts.
 *
 * @internal
 */
export function isTransientLLMError(error: unknown): boolean {
  if (hasAbortInChain(error)) return false;
  …
}
```

Plus a distinctive class: a *single-step OM call whose stream ended early* is made
retryable, so a truncated observer response is re-run from a clean prompt instead of being
saved as a half-observation:

```ts
/**
 * A model reply that ended before the model finished. Carries the AI SDK's
 * `isRetryable` flag, which `isTransientLLMError` and core's error processors
 * already recognize.
 *
 * @internal
 */
export class OmIncompleteResponseError extends Error {
  readonly isRetryable = true;
  …
}

/**
 * OM calls are single-step (`maxSteps: 1`). A step that ends with `other` or
 * `unknown` means the stream closed before the model finished, so its text is
 * partial. Throw a retryable error so `withRetry` re-runs the whole call
 * instead of saving it.
 *
 * @internal
 */
export function assertCompleteModelResponse<T extends { finishReason?: string }>(output: T, label: string): T {
  if (output.finishReason && INCOMPLETE_FINISH_REASONS.has(output.finishReason)) {
    throw new OmIncompleteResponseError(label, output.finishReason);
  }
  return output;
}
```

### 6.3 Where `withRetry` is applied

- The observer model call — `observer-runner.ts` (twice: single-thread and multi-thread
  paths), and the reflector model call — `reflector-runner.ts` — with provider-level retries
  deliberately zeroed so there is one retry ladder:

  ```ts
  const agent = new Agent({
    id: isMultiThread ? 'multi-thread-observer' : 'observational-memory-observer',
    name: isMultiThread ? 'multi-thread-observer' : 'Observer',
    maxRetries: 0,
    // withRetry owns retries and restarts each attempt from a clean prompt.
    // Processor retries would continue from the failed attempt instead.
    errorProcessorDefaults: false,
    …
  ```

- The buffered-chunk DB write (`async-buffer.ts` `persist`, label
  `persist-buffered-observations`).
- Observation-group indexing (`base.ts` `indexObservationGroups`, label `index-observations`).

The loop itself:

```ts
export async function withRetry<T>(fn: () => Promise<T>, opts: WithRetryOptions): Promise<T> {
  const { label, abortSignal, maxRetries = RETRY_CONFIG.maxRetries } = opts;
  let attempt = 0;
  // total tries = maxRetries + 1 (the initial attempt isn't a "retry")
  while (true) {
    if (abortSignal?.aborted) {
      throw new Error('The operation was aborted.');
    }
    try {
      return await fn();
    } catch (error) {
      if (hasAbortInChain(error) || abortSignal?.aborted) throw error;
      if (attempt >= maxRetries || !isTransientLLMError(error)) {
        if (attempt > 0) {
          omDebug(
            `[OM:retry:${label}] giving up after ${attempt} retry/retries: …`,
          );
        }
        throw error;
      }
      const delay = computeDelay(attempt);
      attempt++;
      …
      await sleep(delay, abortSignal);
    }
  }
}
```

Non-transient errors (auth, validation, schema) are rethrown on the *first* failure; aborts
are rethrown without delay.

### 6.4 What an exhausted async cycle leaves behind

Tracing an async buffer cycle that exhausts all retries (all calls in this section
previously quoted):

1. **A persisted failure marker** — `base.ts` catch → `emitFailedMarkers` →
   `async-buffer.ts` persists `createBufferingFailedMarker({ cycleId, operationType:
   'observation', startedAt, tokensAttempted, error, failurePolicy, recordId, threadId })`
   to the last assistant message (and streams it transiently to the UI).
2. **Flag cleared, op unregistered** — `buffer()` `finally`
   (`setBufferingObservationFlag(record.id, false)`, `unregisterOp`,
   `asyncBufferingOps.delete`): the record is not "stuck buffering".
3. **The interval boundary stays advanced** — it was written *immediately at trigger*,
   before any async work (§3.1), and the success-only write
   (`setBufferingObservationFlag(record.id, false, newTokens)`) never happens. So the
   consumed interval does not re-trigger; the next trigger is the next boundary crossing.
4. **No chunk** — persist is the last step; nothing partial is in
   `bufferedObservationChunks`.
5. **The raw material remains unobserved** — the time cursor and `lastObservedAt` are
   untouched, so the entries are in the next cycle's candidate set (next interval, the
   threshold, or turn end).

**Dead-letter / debug surfacing:** the failed marker (durable, per-cycle, carries the error
and the configured `failurePolicy`) plus `omError('[OM] buffer() failed', error)` /
`[OM:retry:{label}] giving up after N retry/retries` logs. There is no queue, no
replay — abandonment is the policy, and the material's survival is guaranteed by the cursor
discipline.

### 6.5 `failurePolicy` for the sync path

The user docs (`observational-memory.mdx`, "Retries and failure policy"):

> With `failurePolicy: 'continue'`, Mastra retries as configured, reports the failure through
> the existing OM diagnostics, keeps the failed input pending for a later cycle, and lets the
> main agent turn continue. It doesn't advance observation boundaries or discard unobserved
> messages. A Reflector failure under `'continue'` leaves any already-persisted observations
> committed and defers reflection to the next threshold crossing.

> This policy applies only to Observer and Reflector model and provider failures in
> synchronous, resource-scoped, and buffered observation. Persistence, indexing, transform,
> locking, invariant, and explicit abort failures remain fatal.

`base.ts` implements exactly this: under `failurePolicy: 'continue'` an
`observer-model` execution failure returns `{ observed: false, error }` instead of throwing.
tau's turn-end pass today has **no** retry and **no** policy — `turn_end.rs`
`settle_turn` propagates the first provider error as `OmError::Provider`, and
`agent/turn.rs` `run_turn` propagates that as `AgentError::Om`, killing the turn. That is
the second of the two 2026-10-06 2.1 run failures (the idle-timeout killing a 68k-event
run).

## 7. Activation / swap

### 7.1 When promotion runs

Four call sites, all through `activate()`:

1. **Step 0 of every turn** (`step.ts`): `om.activate({ … checkThreshold: true … })` — the
   threshold-guarded promotion that makes "30k reached → buffered observations activate
   instantly with no pause" true.
2. **The threshold-observation loop** (`step.ts`, `runThresholdObservation`) — while
   `shouldObserve && canActivate`, activate repeatedly (every chunk the sync observer would
   otherwise re-observe):

   ```ts
   // Activate buffered chunks first. Buffering stops once pending tokens reach the
   // threshold, so the content that crossed it is never in a chunk and activation
   // alone may not bring the context back under the threshold. Keep activating while
   // chunks remain — every message a chunk owns must be activated before the sync
   // observer runs, or it would observe them a second time.
   let pendingMessages = observableMessages;
   const activatedMessageIds: string[] = [];
   let activated = false;
   while (status.shouldObserve && status.canActivate) {
     const activation = await om.activate({ … });
     this.turn.setRecord(activation.record);
     if (!activation.activated) break;
     …
   }
   ```

   and it is preceded by the in-flight join:

   ```ts
   // Wait for any in-flight buffering to settle, then refresh the turn cache once.
   await om.waitForBuffering(threadId, resourceId);
   await this.turn.refreshRecord();
   ```

3. **Idle TTL / provider change** — inside `activate()` itself
   (`activationTriggeredBy: 'threshold' | 'ttl' | 'provider_change'`), where
   `checkThreshold` is the union: provider-change and TTL-expired bypass the token check,
   otherwise `status.pendingTokens < status.threshold` skips activation.
4. **`finalize()`** — the terminal sweep:

   ```ts
   // Wait for any in-flight buffer operations to complete
   await BufferingCoordinator.awaitBuffering(threadId, resourceId ?? null, this.scope);

   // Activate any remaining buffered chunks
   const preStatus = await this.getStatus({ threadId, resourceId, messages });
   if (preStatus.canActivate) {
     const actResult = await this.activate({ threadId, resourceId, messages });
     activated = actResult.activated;
   }

   // Observe if threshold is crossed (advances the cursor)
   const postStatus = await this.getStatus({ threadId, resourceId, messages });
   if (postStatus.shouldObserve) {
     const obsResult = await this.observe({ threadId, resourceId, messages });
     observed = obsResult.observed;
   }

   // Reflect if observation tokens exceed reflection threshold
   const reflectStatus = await this.getStatus({ threadId, resourceId });
   if (reflectStatus.shouldReflect) {
     const refResult = await this.reflect(threadId, resourceId);
     reflected = refResult.reflected;
   }
   ```

### 7.2 The retention-floor boundary math

`thresholds.ts`:

```ts
/**
 * Convert bufferActivation to an absolute retention floor (tokens to keep after activation).
 * When bufferActivation >= 1000, it's an absolute retention target.
 * Otherwise it's a ratio: retentionFloor = threshold * (1 - ratio).
 */
export function resolveRetentionFloor(bufferActivation: number, messageTokensThreshold: number): number {
  if (bufferActivation >= 1000) return bufferActivation;
  const ratio = Math.max(0, Math.min(1, bufferActivation));
  return messageTokensThreshold * (1 - ratio);
}
```

The chunk-boundary selection (`calculateProjectedMessageRemoval`) — the same math tau already
ports as `projected_message_removal` in `om.rs` — quoted for the ramp of its guards:

```ts
// Find the closest chunk boundary to the target, biased over (prefer removing
// slightly more than the target so remaining context lands at or below retentionFloor).
// Track both best-over and best-under boundaries so we can fall back to under
// if the over boundary would overshoot by too much.
…
// Safeguard: if the over boundary would eat into more than 95% of the
// retention floor, fall back to the best under boundary instead.
// This prevents edge cases where a large chunk overshoots dramatically.
// Additionally, never bias over if it would leave fewer than the smaller of
// 1000 tokens or the retention floor — at that level the agent may lose
// all meaningful context.
const maxOvershoot = retentionFloor * 0.95;
```

and `blockAfter` flips the swap to force-max:

```ts
const forceMaxActivation = !!(
  this.observationConfig.blockAfter && currentPendingTokens >= this.observationConfig.blockAfter
);
```

```ts
const activationResult = await this.storage.swapBufferedToActive({
  id: freshRecord.id,
  activationRatio,
  messageTokensThreshold,
  currentPendingTokens,
  forceMaxActivation,
  bufferedChunks: freshChunks,
});
```

`blockAfter` defaults to `1.2 × messageTokens` when async buffering is enabled
(§8); docs: "Above `blockAfter` (default `1.2`, ~36k tokens) activation is allowed to
overshoot the retention target instead of activating fewer chunks."

### 7.3 In-flight async cycles at activation

Two joins, both bounded:

- `waitForBuffering` → `BufferingCoordinator.awaitBuffering` with
  `timeoutMs = 30000` — a `Promise.race` between `Promise.allSettled(promises)` and a timeout
  (used by `runThresholdObservation` and `finalize`).
- The 60 s `Promise.race` inside `activate()` itself (§3.4), for the case where the DB flag
  is set and the op is in-process.

Both are *best-effort*: on timeout the activation proceeds with whatever chunks exist. An
in-flight cycle that finishes after the swap simply appends a new chunk that the next
activation promotes — the swap is idempotent per chunk set, and the boundary reset
(`resetBufferingState`) starts the next interval from zero.

## 8. Config surface

Every async-relevant knob, its default, and where it is resolved
(`observational-memory.ts` constructor + `thresholds.ts` + `activation-ttl.ts` +
`model-by-input-tokens.ts`; user-facing table in `observational-memory.mdx` "Async buffering
→ Settings"):

| Knob | Default | Resolved by | Notes |
| - | - | - | - |
| `observation.bufferTokens` | `0.2` (ratio of `messageTokens`) → 6k at 30k | `resolveBufferTokens` | `false` disables async buffering entirely (observation *and* reflection); must be `< messageTokens` (`validateBufferConfig`); "buffer every 20% of `messageTokens`" |
| `observation.bufferOnIdle` | `false` | constructor `?? false` | end-of-turn idle buffering, separate from `bufferTokens` |
| `observation.bufferActivation` | `0.8` | `resolveRetentionFloor` / `resolveActivationRatio` | ratio `(0,1]` or absolute `≥ 1000`; values in `(1, 1000)` are a config error; "retain ~20% of messageTokens as raw messages" |
| `observation.blockAfter` | `1.2` (when buffering enabled) | `resolveBlockAfter` | multiplier `[1, 100)` of `messageTokens`, or absolute `≥ 100`; requires `bufferTokens` set; "synchronous observation is never used when async buffering is enabled" if unset |
| `observation.maxRetries` | `8` | constructor `?? RETRY_CONFIG.maxRetries` | per-stage retry count (§6.1) |
| `observation.failurePolicy` | `'abort'` | constructor `?? 'abort'` | terminal policy after exhaustion (§6.5) |
| `activateAfterIdle` / `observation.activateAfterIdle` | none | `parseActivationTTLConfig` + `resolveActivationTTL` | ms, duration string, `"auto"` (provider-aware: 5 min Anthropic/xAI/OpenRouter/unknown, 1 h OpenAI/DeepSeek, 2 h Groq, 24 h Gemini — `activation-ttl.ts`), or per-provider map |
| `activateOnProviderChange` | `false` | constructor `?? false` | force-activate on model/provider switch |
| `reflection.bufferActivation` | `0.5` | `maxRetries`-stage config | must be in `(0, 1]`; starts buffered reflection at 50% of `observationTokens` |
| `reflection.activateAfterIdle` / `activateOnProviderChange` | none / `false` | same as above | reflections do **not** inherit the top-level values |
| `reflection.blockAfter` | `1.2` (when enabled) | `resolveBlockAfter` | above it, reflection runs synchronously |
| `model` (per stage) | `google/gemini-2.5-flash` | `ModelByInputTokens.resolve` when tiered | token-tiered model selection: |

`model-by-input-tokens.ts` (tiered model selection by input token count):

```ts
resolve(inputTokens: number): AgentConfig['model'] {
  for (const { limit, model } of this.thresholds) {
    if (inputTokens <= limit) {
      return model;
    }
  }

  const maxLimit = this.thresholds[this.thresholds.length - 1]!.limit;
  throw new Error(
    `ModelByInputTokens: input token count (${inputTokens}) exceeds the largest configured threshold (${maxLimit}). ` +
      `Please configure a higher threshold or use a larger model.`,
  );
}
```

Validation constraints (`validateBufferConfig`, all thrown at construction):
`bufferTokens > 0` and `< messageTokens`; `bufferActivation ≤ 1` (ratio) or `≥ 1000`
(absolute, and then `< messageTokens`); `blockAfter ≥ messageTokens` and requires
`bufferTokens`; `reflection.bufferActivation ∈ (0, 1]`; `reflection.blockAfter ≥
observationTokens` and requires `reflection.bufferActivation`; async buffering unsupported
with `scope: 'resource'` (deprecated); `shareTokenBudget` requires async buffering disabled
(temporary limitation).

## 9. Tau mapping

### 9.1 Mechanism-by-mechanism table

| # | Mastra mechanism | Tau equivalent today (source) | Status |
| - | - | - | - |
| 1 | Interval-boundary trigger + ramp (`shouldTriggerAsyncObservation`) | `plan()` Buffer arm: `pending_tokens >= buffer_increment` — absolute, no boundary, no ramp, evaluated **once per turn at turn end** (`turn_end.rs`) | **New** — per-iteration boundary check |
| 2 | Per-step check in `processInputStep`/`step.prepare()` | `om_turn_end()` only when `result.calls.is_empty()` in the `run_turn` loop (`agent/turn.rs`); killed turns skip the pass entirely | **New** — mid-loop trigger point in `run_turn` |
| 3 | Turn-end idle buffer (`bufferOnIdle`, `turn.end()`) | The whole `settle_turn` pass (turn-end only) | Partly retained (final sweep, §10) |
| 4 | `BufferingCoordinator` static maps + `operation-registry` + DB flag | None — `OmState` is a per-session in-memory clone; `buffered: Vec<BufferedChunk>` dies with the process ("A restart loses the buffer; the threshold path then re-observes the range" — `om_integration.rs`) | **New** — process-level registry + persisted flag/boundary |
| 5 | Async buffer cycle → `bufferedObservationChunks` (persisted) | `apply_buffer` — synchronous, in the turn-end pass, in-memory chunk | **New** — background task + persisted chunks |
| 6 | Async reflection (`bufferActivation` 0.5, `bufferedReflection`) | Reflect only at `reflect_threshold`, turn end, synchronous (`plan()` first arm) | **New** (or deferred — open decision) |
| 7 | `withRetry` (8×, exp backoff, transient-only) | None — `provider.call(...).await.map_err(OmError::Provider)?` in `settle_turn`; failure kills the turn (`agent/turn.rs` `om_turn_end` → `AgentError::Om`) | **New** — per-trigger-point configurable count |
| 8 | Activation: `swapBufferedToActive` + retention floor + `blockAfter` + `waitForBuffering` | `promote()` + `projected_message_removal` (ported math) — fires at turn end on threshold **or** fixed 60 s idle (`IDLE_ACTIVATION_SECS`); no `blockAfter`, no in-flight join (no in-flight cycles exist) | Retained + extended |
| 9 | Config surface (§8) | `config.rs` `Om { om_model, observe_threshold: 30_000, reflect_threshold: 40_000, buffer_increment: 6_000 }` | Extended |

### 9.2 Conflict (a): turn flag / session lock discipline

tau's existing background-safe pattern is the `with_store` re-lock. `turn_end.rs`,
`settle_turn` doc:

> The state owns the sequence; `with_store` is the caller's re-lock — each sync store phase
> runs under the session lock and releases it before the LLM round-trips (a std guard cannot
> cross an await in a spawned future), so the pass never holds the lock across a provider
> call.

and `agent/turn.rs`, `om_turn_end`:

> The state runs on a clone and is written back at the end; the store is handed to it phase
> by phase through the re-lock, so the session lock is never held across a provider call (a
> std guard cannot cross an await in a spawned future).

A mastra-parity background cycle is *exactly* a spawned future that must (1) never hold the
session lock across its LLM round-trip, and (2) not mutate the turn's in-flight `OmState`
clone. The state-clone write-back that works for the awaited turn-end pass is **unsound for a
concurrent background cycle**: the turn's loop assembles context from `inner.om` on every
iteration, so a background task writing back a stale clone would clobber the turn's state
(lost update). The design consequence: a background cycle's durable output must go straight
to the session file as an append (the record is appended, not maintained in place —
`om_integration.rs` `save`), with the in-memory `OmState` re-loaded/merged under a short
lock — i.e. the re-lock pattern, but with file-append as the commit point instead of
clone-writeback.

### 9.3 Conflict (b): the `calls`/`completed` reconciliation

Every `LiveSession`'s provider is a `ForwardingProvider` (`harness/state.rs`:
`provider: Arc<ForwardingProvider>`), whose `ForwardSink` records **every** call it mirrors:

> ```rust
> // harness/forwarding.rs, ForwardSink::event
> if !self.started {
>   self.started = true;
>   self.calls.lock().expect("…").push(self.call_id.clone());
>   self.send(Event::StreamStart { … });
> }
> ```

and notes completion:

> ```rust
> // A stop cuts the stream at the next delta (the loop records the partial as
> // interrupted, spec §7). Snapshot it before the completed note: …
> let stopped = self.stop.load(Ordering::SeqCst);
> if let TurnEvent::Completed(_) = &event && !stopped {
>   self.completed.lock().expect("…").insert(self.call_id.clone(), true);
> }
> ```

The post-turn reconciliation then diffs the turn's calls against assistant entries, and
**every call that produced no entry gets a `StreamEnd` whose `interrupted` flag is the
negation of the `completed` note** (`harness/turn.rs`, empty-partial loop):

> ```rust
> let new_calls = calls.len() - calls_before;
> for i in assistant_index..new_calls {
>   let call_id = calls[calls_before + i].clone();
>   let completed = completed.get(&call_id).copied().unwrap_or(false);
>   core.emit(Event::StreamEnd {
>     …,
>     interrupted: !completed,
>     usage: None,
>   });
> }
> ```

The #82 fix (`63e62d4`) exists precisely because today's *synchronous, awaited* OM observer
call goes through this seam and its response never becomes a journal entry:

> Pre-fix, the empty-partial arm treated every entry-less call as cut — including the OM
> observer call, whose response never becomes a journal entry — emitting a spurious
> interrupted StreamEnd that made the ACP settle rule answer Cancelled for completed runs.

The fix covered the *successful* case (Completed frame ⇒ clean end). It does **not** cover a
background OM cycle that *fails*: the call is registered in `calls` at its first stream
event, no `Completed` frame ever arrives, and the reconciliation deterministically emits
`StreamEnd { interrupted: true }` for it. Downstream, `tau-acp/src/pump.rs` turns that into
a cancelled trial:

> ```rust
> fn queue_settle(s: &crate::sessions::SessionState, queue_empty: bool) -> Option<Outcome> {
>   if s.saw_stream_end && queue_empty {
>     Some(if s.saw_interrupted || s.cancel_active {
>       Outcome::Cancelled
>     } else {
>       Outcome::EndTurn
>     })
>   }
>   …
> }
> ```

So the hard constraint for parity: **a background maintenance call must never be visible to
the reconciliation** — neither as a `calls` entry (no `StreamStart` mirroring, no
empty-partial `StreamEnd`) nor as a dangling id. Two shapes satisfy this; the plan must pick
one: route background cycles through the *inner* provider (the `ForwardingProvider.inner`
`TurnProviderRef`, which the OM turn-end path can already reach — it is a plain
`TurnProviderRef` in `agent/turn.rs` `om_turn_end`) so no `ForwardSink` is in the chain at
all; or keep the forwarding seam and tag maintenance calls so the reconciliation skips them.
The former is structurally cleaner (a background OM call is not a turn stream — it has no
GUI bubble to open or close) and is the recommendation carried into §10. A second, narrower
instance of the same conflict: a background cycle that is *still in flight* when the turn
settles would have its `calls` entry fall inside the settling turn's `calls_before` diff
window — the exemption must apply to in-flight background calls, not only failed ones.

### 9.4 Conflict (c): one-episode-per-turn headless structure

In the eval rig a Terminal-Bench trial is **one long turn**: the episode loop's continuation
prompts ride the follow-up/steering lanes of the same session, and the tool loop
(`run_turn`'s `loop { … run_tools … }`) runs the whole trial. There are no intermediate turn
ends, so the turn-end-only pass runs once per trial — which is how a turn whose unobserved
history reached ~262k tokens produced a single observer prompt one token over the 256k window
(the 2026-10-06 2.1 run's deterministic 400). Mastra's structure dissolves this: its
"steps" are per-LLM-call, and the boundary trigger fires mid-episode. The tau equivalent of
"one step" is **one iteration of the `run_turn` loop** (one `provider.call` + one
`run_tools` batch). The mid-loop trigger check therefore belongs in `run_turn` — after
`run_tools` appends the tool results (the moment new unobserved tokens exist) and/or before
the next `provider.call` — computing pending tokens from the store under a short lock and,
on a boundary crossing, spawning the background cycle. The turn-end pass then shrinks to
what mastra's `turn.end()` + `finalize()` do: promote buffered chunks, sweep the remainder,
decide reflection.

### 9.5 Conflict (d): the headless episode loop settle path

`pump.rs` settles a turn only on `StreamEnd` + empty `Queue` (`saw_stream_end`), and the
post-turn OM pass currently runs *inside* `agent.process()` — i.e. **before** the
reconciliation emits the `StreamEnd` — which is why the current design is safe: the settle
path never observes the OM call. Background cycles break that ordering in two ways the
design must neutralize:

1. An in-flight background cycle at settle time must not contribute a `StreamEnd` (covered
   by the §9.3 exemption — the settle path only sees what the reconciliation emits).
2. A background cycle that *completes* during the next episode's turn must not land in that
   turn's `calls_before` diff either — the exemption is per-call (by provider routing), so
   it holds across turn boundaries automatically; a `completed`-map-based scoping would
   need the same care.

### 9.6 What an exhausted cycle must leave behind (tau spec)

Carrying the user design constraint (per-trigger-point configurable retry count) through to
the abandonment case, a tau background cycle that has retried N times and failed must leave:

1. **A failure record** — a log line with the trigger point, cycle id, and error (and, if the
   marker surface is ported, a session entry; at minimum the log — mastra's durable marker
   is a UI feature, not a correctness one).
2. **No dangling call in `calls`** — guaranteed structurally by routing the cycle through
   the inner provider (§9.3), so the reconciliation cannot see it at all; the next turn's
   empty-partial loop sees exactly the turn's own calls.
3. **The observation cursor unadvanced** — the unobserved entries remain unobserved and are
   re-eligible for the next trigger (mastra's guarantee that abandonment is data-preserving).
4. **The interval boundary advanced** — written before the cycle starts, so the consumed
   interval does not re-trigger a tight retry loop of failing cycles; the next trigger is the
   next genuine boundary crossing (or the threshold, which the turn-end pass still owns).
5. **The in-flight flag cleared** — the next cycle can start; a crash mid-cycle leaves the
   same recoverable state mastra's has (§3.4).

## 10. Synthesis — the shape of parity for tau

No implementation here; the implementation plan is written from this section.

1. **Trigger points.** Two, mirroring mastra's partition: (a) a *mid-loop boundary trigger*
   in the `run_turn` loop — per iteration, `floor(pending / bufferTokens)` boundary crossed
   with the ramp, only while `pending < observe_threshold`; (b) the existing *turn-end*
   pass, which keeps the threshold observe, the promote, and the reflect decision. The
   boundary state is the max of a persisted value (in the session file, with the OM record)
   and an in-process value, exactly mastra's two-tier discipline — with a per-session
   in-process registry (mastra's `operation-registry` + `asyncBufferingOps`) so at most one
   background cycle runs per session and a stale flag is detected by "flag set but no
   in-process op".
2. **Background cycles as re-lock tasks.** A triggered cycle is a spawned task that clones
   the OM state, builds the transcript under a short lock, makes its LLM call on the **inner
   provider** (out of the forwarding seam), and commits its chunk by **appending to the
   session file** under a short lock — the `with_store` discipline, with file-append as the
   commit point (no clone write-back, §9.2).
3. **Persisted buffered chunks.** The in-memory `Vec<BufferedChunk>` becomes durable (session
   file), so a restart doesn't force a full re-observation — mastra's chunks survive
   restarts; tau's currently do not.
4. **Per-trigger-point retries.** Adopt `withRetry`'s policy *shape* — exponential backoff
   with cap and jitter, transient-class-only, aborts never retried — with a **configurable
   count per trigger point** (buffer trigger, turn-end observe, turn-end reflect; the
   reflect path's existing level-escalation ladder is a separate, orthogonal mechanism).
   The "incomplete response" class (stream ended with a partial finish reason) is retryable,
   so a truncated observer output is re-run rather than saved.
5. **Exhaustion is silent to the turn.** Per §9.6: failure record, no reconciliation
   visibility, cursor unadvanced, boundary advanced, flag cleared.
6. **Activation unchanged in math, extended in reach.** `promote()` keeps the ported
   `projected_message_removal`; add the `blockAfter` force-max; the 60 s fixed idle
   activation stands in for mastra's `activateAfterIdle` (v0's documented choice).
   `waitForBuffering` becomes a bounded join (the 30 s/60 s races) before any activation
   that could race an in-flight cycle.

## Open design decisions

1. **Configurable retry count per trigger point.** One shared `om.retries` value, or
   per-trigger-point values (mid-loop buffer / turn-end observe / turn-end reflect)? Mastra
   is per-stage (observation vs reflection); tau has four trigger points. Also: is the
   backoff schedule fixed (mastra's 1 s → 120 s, ~4 min total) or does the count scale it?
2. **Reconciliation scoping for exhausted (and in-flight) cycles.** Route background cycles
   through the inner provider (recommended, §9.3), or keep the forwarding seam and tag/skip
   maintenance calls in the `calls`/`completed` reconciliation? The former changes what the
   GUI sees for OM activity (no stream bubbles for background cycles — which it doesn't see
   today either, since the turn-end pass's call is an entry-less call the #82 fix silences
   only on success).
3. **Where the mid-loop trigger check lives in the turn driver.** After `run_tools`
   (new tokens exist, batch granularity) vs before `provider.call` (mastra's per-step
   position, one extra store read per iteration) vs both with the boundary math making the
   second a no-op? The check needs pending tokens, which means a store read under the
   session lock per iteration — the cost is one `entries_range` + token count over the
   unobserved tail.
4. **Interaction with the reflect path.** Does async buffering get a reflection lane
   (mastra's 0.5-threshold buffered reflection), or does reflect stay turn-end-only in v0
   (the buffered-observation chunks raise `observation_tokens` at activation, so the
   turn-end `should_reflect` check sees the post-activation count — at what point in the
   turn-end sequence does that check run relative to promote)?
5. **What the turn-end pass retains.** Recommended: promote (activation, no LLM) + final
   sweep observe of the unbuffered remainder (mastra's `finalize`) + the reflect decision.
   Does the turn-end pass also await in-flight background cycles first (mastra's
   `finalize` does `awaitBuffering` before activating), and with what timeout?
6. **Buffered-chunk persistence shape.** Chunks ride the existing `om` record entry (record
   is appended per change — a chunk append is a new `om` entry), or get their own entry
   kind? Affects crash semantics: a cycle that appends its chunk then dies before clearing
   the flag must leave a state the next process can recover (mastra recovers via the
   flag/registry split, §3.4).
7. **Minimum chunk size.** Mastra skips a triggered cycle with fewer than `bufferTokens/2`
   *new* tokens. Adopt the same floor for tau's background cycles (a boundary crossing on a
   trickle of tool results shouldn't burn an observer call)?
