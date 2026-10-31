# OM Observer overflow diagnosis (2026-10-31)

**Reported symptom.** "I recently got a 400 error that the output exceeds the safe
limit, but there's nothing left to compact."

That sentence is `AgentError::ContextOverflow`'s own display string —
`crates/tau-core/src/agent/mod.rs:86`: *"context overflow: prompt ~{prompt} tokens
exceeds the safe budget of {budget}; nothing left to compact"*. It is not the
gateway's wording (the gateway says *"leaves no room to answer"*), so the report is
about a **pre-wire refusal the core raised itself**, not a provider rejection.

**Status: diagnosed, no fix shipped.** The red loop is in the tree (below).

## Scope and baseline

Evidence read, not guessed: the 1.5 MB `/tmp/tau.log`, and the durable diagnostic
records in `.tau/sessions/*.jsonl` (ADR-0034 — records, not log lines). Working
tree at `71ed1eb`. Gate: `cargo nextest -p tau-core --lib om_integration::tests::plan`.

## Defect A — the Observer's request is unbounded (this is the reported failure)

### The records

| ts | session | record |
| --- | --- | --- |
| 1793406623204 | `5746321196af` | `om_failed` observe: 400 `prompt (217419 tokens) leaves no room to answer in the context (204800)` |
| 1793406623251 | `5746321196af` | `om_stalled { pending: 182094, activation: 30000 }` |
| 1793425421025 | `5746321196af` | `context_overflow { prompt: 204172, budget: 191808 }` |
| 1793412717075 | `cde8f236e720` | `om_failed` observe: 400, prompt 239212 / window 204800 |
| 1793412717108 | `cde8f236e720` | `om_stalled { pending: 198131, activation: 30000 }` |

`/tmp/tau.log` carries five `tau_core::agent::turn::om_pass` WARNs — *"om: turn-end
pass failed: provider: provider returned 400"* — at prompts **226 404 / 231 767 /
305 740 / 341 883 / 365 402** against a **204 800** window, and one
`om_integration::background` WARN (a buffer cycle) at 143 357 against 131 072.
`om_stalled` sits in six session files: pending 107 435, 182 094, 198 131,
199 382, 310 884, 321 593 — every one far past the 30 000 activation threshold.

### The mechanism

The endpoint finding (next section) explains *which* ceiling the Observer hit. The
mechanism below is endpoint-independent: no OM request path reads a window at all.
1. **The Observer call is never gated.** `OmState::plan`
   (`crates/tau-core/src/om_integration/turn_end/mod.rs:320-327`) builds the
   observe action from `transcript(unobserved)` — the *entire* unobserved tail, no
   slice, no cap. `observer_request` (`:275-284`) then builds a `ResponseRequest`
   carrying no `max_output_tokens` and no window. The context gate
   (`agent/turn/gate.rs`) fences the **main** request only; the OM round-trip is
   built on a path that never reads `context_window`. `OmState::config`
   (`om/mod.rs:20-38`) holds no window field, so the OM side *cannot* price its
   own prompt.
2. **The 400 is permanent, so it stalls OM.** `om_integration/retry.rs`
   `is_transient` classifies 400 as permanent → one attempt → `Err` out of
   `run_actions` → `om_turn_end` (`agent/turn/om_pass.rs:100`) records
   `om_failed` (stage `observe`, latched once per stage per run).
3. **The stall is durable.** The cursor never advanced (the pass died before
   `commit`), so `record_om_stall` (`om_pass.rs:248`) sees `pending ≥ activation`
   and records `om_stalled` 47 ms after the `om_failed` — the pair is visible in
   both session files above.
4. **The gate ladder then has no rungs.** Prompt over budget → `gate_cap` →
   `ladder` (`gate.rs:120`): `om_promote` is a no-op (nothing buffered);
   `om_turn_end` re-plans the same observe, dies on the same 400, and `om_failed`
   is latched so it records nothing further; `om_overflow_compact` reflects
   `active_observations`, which is empty — there is nothing to compress because
   the Observer never ran. `om_fingerprint()` (cursor, `observation_tokens`,
   `prefix_demoted`, `buffered.len()`) is unchanged, so `rung_fits` earns no
   re-pricing, and the ladder ends in `refuse_over_budget` →
   `context_overflow` + `AgentError::ContextOverflow` — *"nothing left to
   compact"*, true, because OM's own failure destroyed the compaction.

### The arithmetic checks out

The 01:43 refusal: `budget 191808` = `200000 − 4096 (CONTEXT_GATE_MARGIN) − 4096
(MIN_ANSWER_ROOM)` for the main model's declared window (`prompt_budget`,
`provider/request.rs:472`) — the main request on `qwen3.8-flash-next-iq3_s`, whose window
the config declares at 200 000. Its prompt is 204 172 because the prompt carries the **raw
window**, the whole unobserved tail: the record one tick earlier (05:43:40.993, against the
refusal's 05:43:41.025) reads `pending: 200188`, `observation_tokens: 764`. Nothing was
compacted because the Observer never ran — 182 k of raw tail, 764 tokens of observations.
`~33 282` in the user's copy is the same shape on a smaller session; the numbers vary, the
mechanism does not.

### The route: the Observer carries the local model's name to the gateway

The declared `context_window = 262144` for `qwen3.6-35b-a3b` is **correct** and is not
the cause. Measured live against the config's two endpoints (2026-10-31):

- `GET localhost:8000/v1/models` → `qwen3.6-35b-a3b … context_length: 262144` (kapitsa).
  An over-window request to it refuses in its own words: *"Your input exceeds the context
  window of this model (360010 tokens > 262142 maximum)"*.
- **No 400 in `/tmp/tau.log` has that shape.** Every OM 400 (6 of 7; the seventh, Oct 25,
  names 131 072) reads *"prompt (N tokens) leaves no room to answer in the context
  (204800)"* — the gateway's wording and the gateway's number.
- `GET llms.aaronlockhart.dev/v1/models` lists exactly one model, `swift-1.5-iq3_s`
  (`meta.n_ctx: 204800`). A request carrying `"model": "qwen3.6-35b-a3b"` is served by it
  and answers `"model": "swift-1.5-iq3_s"`. So 204 800 is the real ceiling of an OM
  request that names the local model; the config's 262 144 never reaches the wire.

The seam is a split between name and endpoint, in the tree as it stood when this
was written: `resolve_om_model` supplied the **name** (the config's `om_model`)
and `om_provider` (`agent/turn/om_pass.rs:19`) supplied the **endpoint**, which
is `ForwardingProvider::route` (`harness/forwarding.rs:21`) — a `Mutex` rebound
only by `set_route` (`harness/dispatch_sessions.rs:140`), which fires when the
**session's** model changes. No `Config::route_model` / `route_child` call
(`config/mod.rs:580,606`) was applied to `om_model` anywhere in the tree. #336
made a *main* request follow its model id; the OM pass kept borrowing the
session's endpoint. #348 closed that half: `om_address`
(`agent/turn/om_pass.rs:39`) is the resolver now, addressing the pass at the
provider `om_model` names and recording a refusal when it names no endpoint.

Both incidents show the switch doing it. Each appends `model_changed`
`qwen3.6-35b-a3b → yunyun/qwen3.8-flash-next-iq3_s` at 20:55Z Oct 30, and their `om`
records keep writing `om_model: "qwen3.6-35b-a3b"` (display-only, written from the
resolved name at `turn_end/mod.rs:267`) for hours afterwards — the name stayed local, the
endpoint moved to yunyun. The Observer then ran, successfully, against a 204 800-token
gateway that answers any model id, until the unobserved tail crossed it. The first 400 is
the last, because `unobserved` only grows: `pending` ran 182 094 → 200 188
(`5746321196af`) and 198 131 → 207 161 (`cde8f236e720`), every later pass re-planning a
transcript that route cannot carry. `om_stalled` is the durable form of that.

So: OM's request is not addressed at the 262144-token server. Two independent defects
compose — the borrowed endpoint (this section) and the unbounded transcript (the section
above). A borrowed endpoint is survivable while histories are small; an unbounded
transcript is survivable on a server that honours its own window to a request that can be
sliced. Together they stall, permanently, on the first crossing.

## Red loop

`crates/tau-core/src/om_integration/tests/plan.rs::the_observe_transcript_is_bounded_by_the_om_room`
— pure over `plan()`: no provider, no LLM, no network.

```
$ cargo nextest run -p tau-core --lib \
    'om_integration::tests::plan::the_observe_transcript_is_bounded_by_the_om_room'
FAIL  the observe transcript outgrows the OM room: 341365 tokens over a 126976-token room
Summary [0.161s] 1 test run: 0 passed, 1 failed
```

0.16 s per run (3.9 s with the incremental build). 341 365 is the incident's
341 883 rounded; the room is the contract's default OM route
(`Provider::DEFAULT_WINDOW` 131 072 − 4 096), the same default an OM route with no
declared facts is addressed by. The test asserts the fence the fix must install —
it is red today because **no OM request path reads a window at all**.

## Defect B — the `gate_unavailable` refusals (a separate, closed defect)

Ten `gate_unavailable` records sit in the Oct 29–30 window, *before* the fixes that
landed after them:

- `missing: ["context_window","max_tokens"]` for model `yunyun/qwen3.8-flash-next-iq3_s`
  (`6b82e236fb90`, `e637475c0fc8`) — a menu-qualified id.
- `missing: ["context_room"]` for `qwen3.8-flash-next-iq3_s` and `qwen3.6-35b-a3b`
  (`007d852ce976`, `38397efeab68`, `4199c8c862c4`, `2c599fc0fa40`, `e637475c0fc8`, `00c8886b653d`).

Both shapes are what #342 (`10-30 10:28`, "gate the prompt against the window …
`max_tokens` gates on the default cap, it does not refuse") and #343 (`10-30 13:10`,
"gate the prompt on the window, not the wish") set out to remove: a gate that armed
from `min(max_tokens, …)` refused a model configured with `max_tokens ==
context_window`, which is exactly this user's config. `00c8886b653d`'s refusal at
`10-30 10:32` is 4 minutes after #342 landed — plausibly the pre-fix build. **These
records are not the reported message** (they are `gate_unavailable`, not
`context_overflow`), and the fix is already in the tree. Worth one dogfood pass on a
current build to confirm the class is closed; nothing to change today.

## What the fix must do (not yet authorized)

Defect A needs a v0 §4 decision, so it is the user's call. Four moves, in the order the
causal chain runs:

1. **Route the OM request by `om_model`, not by the session's model.** #336's rule — a
   model's endpoint is the provider its id names (`Config::route_model`, `config/mod.rs:519`)
   — is applied to the main request (`harness/sessions.rs:369`, `dispatch_sessions.rs:140`)
   and never to `om_model`. `om_provider` (`agent/turn/om_pass.rs:30`) returns the
   forwarding seam's current route, which tracks the *session*. The OM name and the OM
   endpoint must be resolved together, at the same boundary the config reload uses.
2. **Give the OM route a window.** `OmState::config` carries no `context_window`;
   `om_address` (`agent/turn/om_pass.rs:39`, #348) names the endpoint and the id, so the
   Observer's room is uncomputable as written. Routed facts (`ModelDef.context_window` of
   the provider `om_model` names) are the source — the config declares 262 144 correctly;
   nothing today reads it for OM.
3. **Bound the observe transcript.** Slice `plan()`'s transcript to
   `window − margin − headroom`, oldest entries first, at entry boundaries. The
   cursor is entry-based and `apply_observation` (`turn_end/commit.rs:96-124`) advances it
   to the last entry observed — so the action must carry the observed range, or a slice
   marks unobserved entries observed (silent data loss). With that carried, a slice commits
   cleanly and the remainder is picked up by the next pass: each pass shrinks `pending`,
   the opposite of today's zero-progress stall.
4. **Re-price the ceiling.** `observe_threshold_for` expands with the observation log under
   `share_token_budget` (off by default), so a 321 k stall is reachable at the default
   30 k threshold. A threshold that can outrun the route's window is the precondition;
   bounding the transcript is the enforcement.

The red loop at `plan()` is the seam: it is deterministic, sub-second, and needs
no provider. A routing test belongs at the harness seam (`tests_child_route.rs` is the
nearest shape: a model id, its route, and the facts that follow it).
