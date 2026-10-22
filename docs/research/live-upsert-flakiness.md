# Live upsert flakiness: consumer-side diagnosis

Date: 2026-10-21. Companion to `entry-upsert-audit.md` (PR #264). Question: the wire
stream is consistent (no drops, no inversions — measured in harbor, see #266), so where
does the live-view flakiness come from?

## TL;DR

The flakiness is not in the stream. It is in the **store's three-writer merge** and,
above all, in the **blob boundary**: the live `entry_upsert` path never resolves
sidecar blobs, so any tool result ≥ 100 KB renders "content unavailable — sidecar
blob missing" until a *viewport-triggered* paged fetch happens to cover that entry.
The self-heal is real but position-dependent — which is exactly what "flaky" means.

## Ruled out first (with evidence)

**Transport loss and reordering.** Instrumented harbor run (3 terminal-bench tasks,
yolo, `qwen3.8-27b`): 1,596 upserts fired at the hooks (1,598 `[measure]` lines
total — the other 2 are per-episode `dropped=` settlement summaries), 0 pipe
drops, 0 inversions (no wire-only upsert landed after a persisted one for the
same `(sid, eid)`; the 55 persisted entries show no wire-only marker after
their persisted one — the 235:1 amplification (streaming deltas vs the single
persisted final) was measured directly: top entry 235 wire-only vs 1 persisted). The caveat carried in #266 stands: the `PipeCounters` drop counter does not cover the two upsert `try_send` sites (raw `try_send`, `let _ =`), so "0 drops" is a marker/ordering result, not a counter reading.

That leaves the consumer. The GUI's entry map has three writers:

1. live `entry_upsert` — `store.svelte:918` → `upsertEntry` (last-write-wins per id);
2. snapshot-on-open — `sessions.ts:268-280` (the #248 merge: wire-only ids survive,
   snapshot values win on collision);
3. window fetch — `store.svelte:741-796` → `resolveBlobs` → `upsertEntry`.

Any two racing clobbers the map exactly like a dropped final would — with a perfectly
clean wire. #248 (merged 2026-10-21) already fixed one instance of this class:
`openSession` was preserving the stub over the snapshot. The fix existing is evidence
this is the live bug class.

## The dominant cause, end to end (verified in code)

1. **Core writes big tool outputs to a sidecar blob.** `session/file.rs`
   `extract_blob_if_needed` (line 225): payload > `DEFAULT_BLOB_THRESHOLD`
   (100,000 bytes, `session/mod.rs:27`) → zstd-compressed to `.tau/blobs/`,
   `entry.payload = Value::Null`, `entry.blob = Some(BlobRef)`.
2. **The persistence hook fires after that mutation.** `append_entry`
   (`file.rs:264`) = `append_line` (which does the extraction) then
   `hook(&entry)` (line 272) — the hook receives the *blobless* view.
3. **The wire carries the blobless view.** `entry_to_view`
   (`harness/sessions.rs:478`) clones `payload` (`null`) and `blob` (pointer)
   verbatim into `Event::EntryUpsert`.
4. **The store decodes it as unavailable.** `decodeEntry`'s tool arm
   (`app/svelte/lib/entries.ts:110`, in the tool arm starting :93):
   `unavailable: v.payload == null && v.blob != null ? true : undefined`.
5. **The live path never resolves the blob.** `store.svelte:918` calls
   `upsertEntry(s.entries, ev.entry)` directly — `resolveBlobs` is only called in
   `fetchWindow` (`store.svelte:763`). So the card renders "content unavailable —
   sidecar blob missing" (`presentation.ts:120`) the moment the tool result lands.
6. **Self-heal is viewport-dependent.** `fetchWindow` is triggered only by
   `Transcript.svelte`'s windowing policy (lines 206/235/245 — viewport +
   turn-end fetches). An entry in the visible window gets re-fetched, blob-resolved,
   and its map slot replaced with the decoded content; an entry outside it stays
   "content unavailable" until it scrolls into range.

That last point is the flakiness: identical workloads differ by *where the big tool
result landed in the viewport timeline*. In-view → heals in milliseconds; out-of-view
→ broken for the rest of the session. No wire anomaly involved.
**The blob boundary is hit in real GUI workloads (log-backed):** the local
workspace's durable session files show 63 blob-extracted entries across three
sessions — 61 `tool` + 2 `om` — every one with `payload: null` on the file
line (33 in one long session, 28 in another; originals 100 KB – 522 KB,
zstd sidecars 4 KB – 365 KB on disk). These are exactly the entries whose
live upserts carry the blobless view.

## Secondary: the window/live race (predicted — not directly observed)

The sequence below is reconstructed from the code paths; no captured session
was found in which a resolved page was clobbered back by a later blobless
re-emit. It remains the residual last-write-wins hazard and is what the
`authoritative` merge discipline should close.

`fetchWindow` captures its `session_entries` read at issue time and merges on
resolve (`store.svelte:747-778`, with an explicit re-open guard at 754-761). But the
page read predates any live upsert that lands while it is in flight:

1. big tool result `T` lands live (blobless) at time t₀;
2. a window page covering `T` was issued at t₀−ε (e.g. the post-open height-settling
   recompute storm the `windowsInFlight` dedupe was built for) and resolves at t₁;
3. the page's blob-resolved `T` (full content) upserts at t₁;
4. the *persisted* re-emit of `T` (the turn-settlement `entry_event_hook`, also
   blobless) lands at t₂ > t₁ — last-write-wins clobbers the resolved copy back to
   "content unavailable".

Last-write-wins with two sources of stale-ness (the blobless live path, the
pre-issued page) has no monotonicity: whichever writer fires last wins, resolved or
not. The #248 merge has the same shape at open time; it just picked the safer side
(snapshot wins) by construction.

## Blast radius

- **GUI**: as above — the user-visible flakiness.
- **ACP/headless**: the same blobless `Event::EntryUpsert` feeds `tau-acp`'s
  `agent_message_chunk` mapping, so a headless client's live view of a big tool
  result is equally empty until it re-reads the session. **Log-backed and worse
  than "empty"** — the mapper (`tau-acp/src/mapper.rs`, `on_tool`) derives
  `has_result` from `payload.output`, which is absent when the payload was
  extracted to a blob: the result `tool_call_update` is never owed, so the
  headless client's tool card stays `in_progress` for the rest of the session.
  The harbor `acp-events.jsonl` confirms it: across all three measurement jobs
  every `tool_call` is `in_progress`, zero `tool_call_update` events, zero
  `rawOutput` fields. The runs never surfaced this because the evaluator reads
  the durable session file, not the live stream.

## Fix direction

- **Root fix (the #265 proposal, now sharper):** core-side blob inlining at the
  upsert hook boundary — when `entry_to_view`-shaping an entry whose payload was
  extracted to a blob, read the sidecar and inline the payload (at least for the
  wire path). Then every consumer sees one shape: full payload, no client-side
  resolution, no race, GUI and ACP fixed at once. The blob sidecar stays the
  durable storage; inlining is a read at the already-file-boundary hook.
- **Store discipline (defense in depth, #266-adjacent):** a map slot should only
  be downgraded by a writer that carries the `authoritative` marker — i.e. the
  #266 wire field, whose real value is this merge discipline, not the transport
  (which measured clean). Until then, last-write-wins is the vulnerability.
- **The `unavailable` flag's semantics** deserve a rename or a second state: today
  it conflates "blob deleted from disk" (a real, rare condition the flag's comment
  describes) with "blob not yet fetched" (the common, self-healing one). One
  boolean, two meanings, is what made this hard to see.

## What this is *not*

- Not the ACP chunk coalescing (a different unit — 499 chunks for 1,596 upserts is
  delta re-encoding, not loss).
- Not the drop counter (blind to the upsert path — #266 prerequisite).
- Not the configure-git-webserver harbor hang (evaluator-phase, post-agent-settle;
  a harness-side issue, separate ticket territory).
