# The `entry_upsert` live-display protocol — an audit

Date: 2026-10-21 · Branch: `upsert-audit` (cut from main `50f01f3`) · Scope: read-only.

`entry_upsert` is the only live transcript op (ADR-0008). This audit traces the
wire type, every core emitter, and every consumer, and records where the
contract is ambiguous or where the live and paged paths have drifted apart.

## 1. The wire contract

`EntryUpsert` and `ViewEntry` are defined in `tau-protocol`:

- `crates/tau-protocol/src/events.rs` — `Event::EntryUpsert { workspace, session, entry: ViewEntry }`,
  documented as "the only live transcript op (ADR-0008)"; the GUI sets
  `map[id] = entry`. There is no delete/tombstone variant: the op is
  idempotent-cumulative, full-state replacement per id.
- `crates/tau-protocol/src/snapshot.rs:100-109` — `ViewEntry { id, parent,
  kind, timestamp, payload: Value, blob: Option<BlobRef>, first_kept:
  Option<String> }`. The comment on `blob` cites ADR-0005 (sidecar pointer
  when the payload lives out-of-band).

The design principle: each event carries the *full* `ViewEntry`, never a
delta, so a lost event self-heals on the next snapshot of the same id.

## 2. Emitter sites (tau-core)

All emitters funnel through two agent hooks, which `launch.rs` wires to the
*identical* `Event::EntryUpsert` over one channel:

- `crates/tau-core/src/harness/launch.rs:537-541` — `set_entry_upsert_hook`
  (wire-only re-sends) → `tx.try_send(Event::EntryUpsert …)`.
- `crates/tau-core/src/harness/launch.rs:543-549` — `set_entry_event_hook`
  (entry append / persistence) → same event, same channel.
- Hook type: `crates/tau-core/src/session.rs:51` —
  `EntryEventHook = Arc<dyn Fn(&Entry) + Send + Sync>`.

The underlying channel is **lossy**: `crates/tau-core/src/harness/state.rs:200`
opens it as `mpsc::channel(1024)` and every emitter uses `try_send` (drop on
full), so any event can silently vanish. The pump is a plain forward with no
batching: `crates/tau-core/src/harness/pump.rs:8-11` (comment: frame
alignment is the turn loop's job, ADR-0008).

Individually:

| # | Site | What fires | Wire-only? |
|---|------|-----------|-----------|
| 1 | `session/file.rs:272` — `append_entry` → `entry_event_hook` | entry appended, file line written | no |
| 2 | `session/file.rs:281` — `upsert_event` → `entry_event_hook` | pre-minted id, no file line yet (#148) | **yes** |
| 3 | `harness/turn.rs:177-182` | post-turn re-emit of `KIND_TOOL` entries — "a lossy-channel dedup, idempotent by construction" | no (re-send of the file line) |
| 4 | `agent/turn/sink.rs` — `StreamEntrySink::emit_snapshot` (`SNAPSHOT_INTERVAL = 16ms`, line 26) | growing assistant snapshots | yes |
| 5 | `tools/progress.rs:74-109` (`report`), `:111+` (`report_detail`) | growing tool output / `ui_detail`, 16 ms coalescing | yes |

Hook cloning into the tool path: `agent/turn.rs:159` and
`agent/turn/tools.rs:130` clone `entry_upsert_hook` into `ToolProgress`.
`upsert_event` callers: `agent/queue.rs:95` (steering item shown mid-turn)
and `session/file.rs:430` (test).

Blob extraction happens *before* the hook fires:
`session/file.rs:225-245` (`extract_blob_if_needed`, threshold
`DEFAULT_BLOB_THRESHOLD = 100_000` bytes, `session.rs:27`) rewrites
`entry.payload = Value::Null` and sets `entry.blob = Some(BlobRef)` on the
in-memory entry — so the wire event that follows the file write carries the
null payload + pointer, not the content. `append_entry` (ADR-0008,
`session/file.rs:260-274`) supports pre-minted ids so "the wire's upsert and
the file's line are one object"; the view is built by
`harness/sessions.rs:404-418` (`entry_to_view`).

## 3. Consumers

**Tauri app.** `app/src-tauri/src/main.rs:147-149` pumps
`pump_events(core, |batch| handle.emit("tau://event", batch))`.
The frontend event loop: `app/svelte/lib/store.svelte:917-919` —

```ts
case 'entry_upsert': {
  upsertEntry(s.entries, ev.entry);
```

`upsertEntry` is a direct map-set with decode:
`app/svelte/lib/entries.ts:173-176`. **No blob resolution on this path.**
The paged read, by contrast, *does* resolve before decoding:
`store.svelte:763-778` (`fetchWindow` → `resolveBlobs(…)` → `upsertEntry`).
`resolveBlobs` is `entries.ts:153-166` (a fetch failure keeps the pointer —
one unreadable blob must not sink the window read).

**Snapshot/live merge.** `app/svelte/lib/sessions.ts:261-273` (#248):
snapshot entries are file-backed *metadata skeletons* (id/kind/`preview`
text only — `sessions.ts:79-96`); the merge keeps prior live entries whose
id is absent from the snapshot, then `Object.assign`s the snapshot values
over the live map.

**Render.** `app/svelte/components/Transcript.svelte:52, 70-74` — the
transcript is `Object.values(entries)` in creation-id order; the virtualizer
owns windowing, and the paged-hydration policy
(`app/svelte/lib/windowing.ts`: `scrollFetches`, `openFetches`,
`turnEndFetches`) drives `fetchWindow` through it.

**ACP (headless).** `crates/tau-acp/src/pump.rs:44-58` consumes the same
`Event::EntryUpsert` and feeds `Mapper::on_entry`
(`crates/tau-acp/src/mapper.rs:33-43`), which re-derives deltas with its own
state (`in_flight`, `text_sent`, `tool_created`, `tool_result_sent`,
`mapper.rs:16-26`): char-suffix deltas for assistant text, one `tool_call`
per id, one `tool_call_update` per id. The post-turn re-emit (#3) is a no-op
for the mapper only because of that dedup.

**Blob fetch (paged path only).** The frontend resolves via the
`blob_read` command (`app/svelte/lib/protocol.ts:322`); the core side is
`session/file.rs:23-35` (`read_blob` / `resolve_blob`).

## 4. Ambiguities and messiness findings

**F1 — The live path never resolves blobs (drift from the paged path).**
The live `entry_upsert` handler (`store.svelte:917-919`) decodes the raw
`ViewEntry`; the paged `fetchWindow` path (`store.svelte:763-778`) resolves
blobs first. Because `extract_blob_if_needed` fires *before* the hook
(`session/file.rs:272`), the post-turn re-emission of a tool result
(>100 KB output) arrives with `payload: null` + `blob: Some(…)` — and the
live path stores it as-is. `decodeEntry` then flags it
(`entries.ts:110`: `unavailable: v.payload == null && v.blob != null`),
which `presentation.ts:120` renders as
"content unavailable — sidecar blob missing". A correct, freshly persisted
entry is therefore displayed as *missing* until a paged re-read
(`windowing.ts` `turnEndFetches`) replaces it with the resolved value. The
two paths for the same wire op disagree about what decoding requires.

**F2 — Two semantically distinct hooks share one indistinguishable event.**
`entry_upsert_hook` (wire-only: streaming assistant #4, tool progress #5,
queue echo #2) and `entry_event_hook` (persistence: #1, and the post-turn
re-emit #3) map to the byte-identical `Event::EntryUpsert` at
`launch.rs:537/543`. No field marks a snapshot as provisional vs.
authoritative, and the channel is lossy (`state.rs:200` + `try_send`
everywhere). The "self-healing" claim therefore rests entirely on *some*
later event for the same id existing — guaranteed for the tool/assistant
final line, but nothing in the protocol enforces it. A consumer that cannot
distinguish a wire-only re-send from a persisted line (exactly the
distinction the snapshot/live merge at `sessions.ts:261-273` has to guess
at, via id presence in the file-backed snapshot) is forced to reconstruct
semantics the wire never stated.

**F3 — "Full state, never a delta" is true of the bytes and false of the
meaning.** The assistant stream re-emits the *whole growing text* at 16 ms
(`agent/turn/sink.rs:60-80`); the ACP mapper must re-derive the delta as a
char suffix (`mapper.rs:71-77`) because the protocol offers none. The tool
path's "final result wins" invariant (`tools/progress.rs:70-74` comment)
holds for the file line but is enforced out-of-band: the wire carries
provisional snapshots with `status: None` (decoded as *running*), and the
mapper's `tool_result_sent` dedup (`mapper.rs:24, 117-120`) is what keeps a
dropped mid-stream snapshot from being re-interpreted. Three consumers
(Tauri store, ACP mapper, and the snapshot merge) each implement a private
theory of which upsert is final.

**F4 — Same shape, three different content contracts, in one map.**
The store's `entries` map holds (a) metadata skeletons from the session
snapshot (`sessions.ts:79-96`: id/kind/`preview`), (b) fully decoded live
entries, and (c) the live-but-unresolved blob pointers of F1. The #248 merge
(`sessions.ts:261-273`) and the Transcript render
(`Transcript.svelte:52, 70-74`) treat all three as interchangeable rows. A
card can therefore legitimately render "content unavailable" for an entry
whose content exists and is fetchable — the distinction is invisible in the
type (`protocol.ts:114` `unavailable?: boolean` is a decode artifact, not a
wire field).

## 5. What is solid

- One op, one id-keyed map, no moves: creation-order slot semantics are
  consistent across wire (`snapshot.rs:100`), file
  (`session/file.rs:260-274` pre-minted ids), and render
  (`Transcript.svelte:70-74`).
- The 16 ms coalescing lives at the source (`sink.rs:26`,
  `progress.rs:18`), not in a central coalescer — the pump is a plain
  forward (`pump.rs:8-11`), which keeps the lossy channel's behavior
  predictable (drops are whole events, never partials).
- Positive/negative coverage exists for the ACP mapper's delta logic
  (`mapper.rs:201-223`) and for the protocol roundtrip
  (`crates/tau-protocol/src/events.rs:234, 278`).
