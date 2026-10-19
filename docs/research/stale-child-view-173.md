# #173 — Stale child session view + composer stop/send reflecting the parent

**Status: both defects are already fixed in `64bff48` (not in `origin/main`). No new code needed — the work is to land that commit and re-verify.**

Date: 2026-12-11. Diagnosis from a read-only trace at `origin/main` (`c70c6da`).
The issue's earlier diagnosis comment (2026-10-18) was **high-confidence and correct**; this
document re-verifies it against current main, pins every link in the chain, and adds the
one thing the earlier comment missed: *why the fix never reached main*.

## Symptom recap

1. A child (subagent) session's transcript does not update live — it looks frozen while
   the child is working.
2. The composer's stop/send button always reflects the **parent** session's running state.

## Defect 1 — the child's entries never reached the frontend

The child's `AgentSession` had **no entry hook wired**. The chain:

- The child's live entries are emitted only through the agent's `entry_upsert_hook`
  (wired by `wire_events`, `crates/tau-core/src/harness/launch.rs:454`).
- `wire_events` was called only from the **root** session path
  (`launch.rs:446`). The child launch path wired nothing.
- The bridge's `spawned` (`crates/tau-core/src/harness/forwarding.rs`, `SessionSubagentBridge::spawned`)
  registered the child as a live session (so the GUI *can* open it, ADR-0006) and emitted
  the parent-keyed `SubagentEvent`, but never wired the child's hooks.
- The post-turn reconciliation (`crates/tau-core/src/harness/turn.rs:169-180`) re-emits
  only `KIND_TOOL` entries — it relies on the live hook having done assistant entries,
  which is true for roots and **false for children**. So children's assistant text was
  written to the session file but never emitted as `Event::EntryUpsert`.

Frontend routing was never the problem: `applyEvents`
(`app/svelte/lib/store.svelte:856`, drop guard at `:889`) routes by `ev.session`, and the
child's stub always exists before the child's stream events arrive — the parent-keyed
`subagent_event(spawned)` creates the stub via `touchChild`
(`app/svelte/lib/sessions.ts:143`, called from `applySubagentEvent` `:296/:315/:320`),
which precedes the child's own `stream_start`. So a child-keyed `entry_upsert` would have
applied; it simply never arrived.

## Defect 2 — composer "reflects the parent"

**The premise is not what the code does.** The composer keys off the **in-view** session:

- `Composer.svelte:47-48` — `cur = $derived(currentSession())`,
  `running = $derived(cur?.turn === 'running' || cur?.turn === 'starting')`.
- `currentSession()` (`app/svelte/lib/store.svelte:216`) reads
  `store.sessions[store.current]` — whichever session is open, parent or child.
- `stop()` (`:814`) and `send()` (`:788`) likewise use `store.current` as the session id.

So when viewing a child, the button reflects the **child's** `turn` — and that `turn` was
**stuck at `running`** because the child's `StreamEnd` was never emitted (defect 2's real
cause, tracked as #175):

- `stream_start` set `turn: 'running'` (`store.svelte` `applyEvents` `:889-895`).
- `stream_end` is the only thing that resets it to `idle` (`:903-907`).
- The child's `LiveSession` was built with a **different** `ForwardingProvider` than the
  one its agent ran through (`forwarding.rs`, `SessionSubagentBridge::spawned`): the
  child's agent used the factory's provider, whose `calls` vector recorded the call id,
  but the `LiveSession` got a fresh provider with an **empty** `calls`. The
  post-turn reconciliation derives the `StreamEnd`'s `call_id` from
  `live.provider.calls` (`turn.rs:177`), found nothing, and emitted no `StreamEnd` —
  neither for the child's call nor at turn settlement. `turn` never left `running`,
  so the composer showed "stop" forever, and the parent (which *did* get its `StreamEnd`
  through its own correctly-shared root provider) looked like the "real" running
  session. To a user watching the child tab, that read as "the button follows the parent."

## The fix — already written, in the wrong branch

**`64bff48` "core: child sessions stream to the GUI and stop cleanly; bash gets a real
PATH"** (2026-10-18, the same day as the diagnosis comment) fixes both defects in one
commit:

1. **One provider per child, shared** — `ForwardingChildFactory` now stashes the
   provider it hands a child's agent (`create`, `forwarding.rs:~330`), and the bridge's
   `spawned` **reuses that exact `Arc`** for the child's `LiveSession`
   (`forwarding.rs`, `spawned`, `take_provider`). The reconciliation then finds the
   child's call id and emits its `StreamEnd` (+ assistant `EntryUpsert`).
2. **`wire_events` for children** — `spawned` now calls
   `super::launch::wire_events(&self.core, &self.workspace, &n.child, &agent)`
   (`forwarding.rs:362-364`), so the child's streaming entries, queue projection,
   om-status, and plan hooks all reach the shared event channel live.
3. Side: bash tool gets a real PATH (#172) and honours the session stop flag, killing
   the running child process (#174).

It carries a **dedicated regression test** covering both #173 and #175:
`a_child_reuses_its_agents_provider_and_emits_its_stream_end`
(`crates/tau-core/src/harness/tests_stream.rs:222`) — drives the *default*
`ForwardingChildFactory` (no test-injected factory), and asserts the child's assistant
`EntryUpsert` reaches the stream **and** the child emits a `StreamEnd` with a
non-empty, child-derived `call_id`. **This test passes on `c70c6da`** (verified:
`cargo test -p tau-core --lib harness::tests_stream` → 3 passed).

**Why the issue stayed open despite the fix being in `main`:** `64bff48` was never
merged — `git merge-base --is-ancestor 64bff48 origin/main` → **false**; it exists only
on `origin/edit-array-186` (and two other branches). But the **entire functional
content of `64bff48` reached `main` independently via `bc96aaf`** (edit-array #186,
#192), which re-introduced the same changes as part of a large batch:

- `forwarding.rs` is **byte-identical** between `64bff48` and `c70c6da` (the
  `wire_events`-for-child call at `:362-364` and the provider stash-reuse).
- The regression test is in `main` at `tests_stream.rs:222` and passes on `c70c6da`.
- The bash PATH (#172) / stop-flag kill (#174) changes in `tools/impls.rs` are present
  on `c70c6da` (the only `64bff48→c70c6da` diff there is formatting from the
  `plan_wiring` seam refactor — no behavior).

So `main` today **already contains the complete fix** — only the commit itself (its
message, its lineage) is missing. The issue was diagnosed 2026-10-18 04:46 and fixed at
05:08 that same day in a commit that never landed on `main` through normal PR flow;
nobody re-verified the symptom on a post-`bc96aaf` build.

## Residual gaps — what actually remains

Re-tracing both symptom chains on `c70c6da` with the fix present:

1. **Child transcript live**: child's `entry_upsert` is emitted (hooks wired,
   `forwarding.rs:364`) → parent-keyed `subagent_event(spawned)` creates the stub
   first → `applyEvents` applies the child-keyed upserts. **Closed.**
2. **Composer stop/send**: child's `StreamEnd` is emitted (shared provider,
   `forwarding.rs` `spawned` + `take_provider`) → `turn` returns to `idle`. **Closed.**

The regression test exercises exactly these paths at the core level and passes.

So the only remaining work is **verification, not code**:

- [ ] **Reproduce on a build from `origin/main` (`c70c6da`).** The reporter's repro
      predates the fix reaching main (or ran a build from before it). If a fresh debug
      build from `c70c6da` shows a live child transcript and a child-keyed stop button,
      the issue closes with "fixed in `bc96aaf` (code), diagnosed in #173, regression
      test `a_child_reuses_its_agents_provider_and_emits_its_stream_end`".
- [ ] If it **still reproduces** on `c70c6da`, the gap is in a path the test doesn't
      cover — most likely candidates to re-open:
  - the **test-injected `child_factory`** path (`CoreBuilder::with_child_factory`,
    `state.rs:171`): a custom factory returns a provider the bridge can't `take_provider`
    from, so `spawned` falls back to a **fresh** `ForwardingProvider`
    (`forwarding.rs`, `spawned`, the `unwrap_or_else`) — a LiveSession/agent provider
    mismatch again, i.e. **no child `StreamEnd`**. Production never uses this path
    (no `child_factory` in `app/src-tauri`), so it cannot be the user's bug, but it is
    a latent re-introduction of defect 2 for any future non-forwarding factory.
  - **`pipe_send` overflow** drops (`forwarding.rs` `ForwardSink::send`): a saturated
    channel silently drops a child's `StreamEnd` (counted + one-shot summary). Under
    heavy multi-child load this could transiently stall a child's `turn` at
    `running` — the snapshot self-heals on next `session_open`, so it would read as
    "stale… at least sometimes," matching the issue's hedge.

## Recommendations

1. **Verify on `c70c6da`** (fresh debug build, spawn a child, watch the child tab +
   composer). Expected: fixed.
2. **Close #173** with a pointer to `bc96aaf`/`64bff48` and the regression test.
3. Nothing to port: the bash PATH (#172) / stop-flag kill (#174) changes from
   `64bff48` are already in `main` via `bc96aaf` (verified in `tools/impls.rs` at
   `c70c6da`). If #172/#174 are still open, re-diagnose them on their own merits.
4. Optional hardening (only if a non-forwarding `child_factory` is ever used in
   production): make the `spawned` fallback detect the mismatch instead of silently
   building a fresh provider. Not needed for v0 — `app/src-tauri` uses only the default
   factory.

## Pin index

| What | Where |
|---|---|
| Child hooks never wired (original defect 1) | `launch.rs:446` (root-only) vs `forwarding.rs` `spawned` |
| Fix: `wire_events` for the child | `forwarding.rs:362-364` |
| Fix: one shared provider per child | `forwarding.rs` `create` (stash) / `spawned` + `take_provider` (reuse) |
| `StreamEnd` call_id derivation (defect 2 mechanism) | `turn.rs:177` (from `live.provider.calls`) |
| Composer keys off in-view session | `Composer.svelte:47-48`, `store.svelte:216-218` |
| `turn` lifecycle in the store | `store.svelte` `applyEvents`: `stream_start` → running (`:889`), `stream_end` → idle (`:903`) |
| Child stub precedes child events (no drop) | `sessions.ts:143` `touchChild` via `applySubagentEvent` (`:296/:315/:320`) |
| Regression test | `tests_stream.rs:222` `a_child_reuses_its_agents_provider_and_emits_its_stream_end` |
| Fix commit (unmerged) | `64bff48`; fix code reached main via `bc96aaf` |
