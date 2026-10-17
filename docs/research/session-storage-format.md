# What session storage format should tau use?

Research for [aaronlockhartdev/tau#3](https://github.com/aaronlockhartdev/tau/issues/3) (child of the v0 envelope, #1).
Date: 2026-09-15. Method: primary sources (pi install docs + shipped source, SQLite/LevelDB/Kafka/MessagePack/CBOR/FlatBuffers specs) plus direct measurement of real pi session files (script + data in §1.2).

## Brief (from ticket #3)

Requirements the format must satisfy:

- Incremental, crash-safe writes while a turn streams (writes cheap; kill at any point never corrupts).
- A tree of entries (id + parent id) with in-place branching (no file rewrite on branch).
- Compaction records that replace/annotate a stretch of history — richer than pi's summary string (OM records).
- Concurrent writers: multiple sessions writing at once (at minimum per-session concurrency).
- Read patterns: GUI loads the full tree; incremental tail for live streaming; branch re-reads after compaction.

Dimensions: write cost during streaming; read cost (full tree vs tail); branching cost; compaction-rewrite cost; on-disk size; crash safety; tooling/debuggability ("human-readable files have real value in a dev tool").

**User's decision rule:** "use it if there's a more efficient format" than pi's JSONL — i.e., deviate from the JSONL baseline only if a candidate is more efficient across these dimensions. The choice is delegated; the user ratifies at close.

## 1. Baseline: pi's JSONL

### 1.1 Format and mechanics

From pi's docs (`session-format.md`, `sessions.md`, `compaction.md` in the local install at `/Users/aaron/.local/share/nvm/v26.7.0/lib/node_modules/@earendil-works/pi-coding-agent/docs/`) and its shipped source ([`session-manager.ts`](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/src/core/session-manager.ts), verified against the installed `dist/core/session-manager.js`):

- **One file per session**: `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`. JSON Lines: first line is a header `{"type":"session","version":3,id,timestamp,cwd}`; each subsequent line is one entry `{"type","id","parentId","timestamp",...}`.
- **Tree via `id`/`parentId`**: entries form a tree; the "leaf" is the current position. **Branching = appending a new child line** pointing at any earlier entry — no file rewrite (docs: "enabling in-place branching without creating new files").
- **Entry types**: `message` (user/assistant/toolResult/bashExecution/custom), `compaction`, `branch_summary`, `custom`, `custom_message`, `model_change`, `thinking_level_change`, `label`, `session_info`.
- **Compaction is append-only**: a `compaction` entry stores `summary`, `firstKeptEntryId`, `tokensBefore`, plus an extension `details` field and optional `usage`; newer harness compactions embed `retainedTail` making the entry a self-contained checkpoint. Pre-compaction entries **remain on disk**; context rebuild simply starts at the kept boundary. No in-place rewrite ever.
- **Write path**: each entry is one `appendFileSync(file, JSON.stringify(entry) + "\n")`. The file is created exclusively (`openSync(..., "wx")`) and flushed when the first *assistant* message lands (entries before that are buffered in memory). **No `fsync` anywhere** in the module's fs imports.
- **Crash safety (process-kill)**: on load, `loadEntriesFromFile` parses line-by-line and **skips malformed lines**; if the file ends without a newline (torn last write) it appends `"\n"` to terminate the partial line, which is then skipped as malformed. A kill at any point loses at most the in-flight entry and never corrupts earlier entries. (Not power-loss durable, since nothing is fsync'd — the ticket's requirement is process-kill safety, which pi meets.)
- **Whole-file rewrite** happens only during version migration (v1→v3) and when forking/copying to a new file (`forkFrom`).

### 1.2 Measured size profile

Measured 2026-09-15 against real pi session files under `~/.pi/agent/sessions` (280 files, 1.1 KB → 85 MB, ≈1 GB total). For sample files, every entry was re-encoded as (a) stored JSONL, (b) JSONL + per-line CRC field, (c) per-entry MessagePack, (d) per-entry definite-length CBOR (RFC 8949 minimal encoder); sizes in MB, encode times best-of-3:

| Session (entries) | JSONL | JSONL+CRC | MessagePack | CBOR | JSON enc | MP enc | CBOR enc |
|---|---|---|---|---|---|---|---|
| 0.49 MB (60) | 0.49 | 0.49 (+0.2%) | 0.42 (−14.1%) | 0.42 (−13.8%) | 1 ms | 1 ms | 4 ms |
| 5.59 MB (1,253) | 5.57 | 5.59 (+0.3%) | 4.99 (−10.4%) | 4.99 (−10.4%) | 8 ms | 11 ms | 36 ms |
| 40.88 MB (4,602) | 40.73 | 40.80 (+0.2%) | 37.99 (−6.7%) | 38.01 (−6.7%) | 38 ms | 71 ms | 191 ms |

Composition of the 40.88 MB file (by entry type / role):

- `message` 88.7%: **toolResult 26.8 MB**, assistant 9.3 MB, user 0.02 MB.
- `compaction` 2.74 MB (14 entries: 1.41 MB `details` + 1.33 MB `summary`), `custom` 1.81 MB (362 entries).
- Content: **base64 images 16.5 MB = 40.4%** (40 images); thinking 3.1 MB; tool-result text 2.2 MB; toolCall args 1.0 MB; user+assistant text 0.29 MB.
- Line lengths: p50 1.8 KB, p99 210 KB, p99.9 837 KB, **max 1,049,788 bytes (one 1 MB base64 image on a single line)**; 83 lines > 100 KB.

Interpretation:

1. **The encoding is not where the bytes live.** Payload (base64 images + tool output + thinking) dominates; JSON overhead (key names, escaping) is only the 4–14% gap measured above, and the gap shrinks as sessions get payload-heavy (6.7% at 41 MB). A binary format cannot compress or remove the payload.
2. **The real size levers are content policies** (raw-bytes sidecars instead of inline base64, output truncation), which are orthogonal to the line format — pi already externalizes large bash output via `fullOutputPath`, but images stay inline.
3. The per-line CRC hardening costs only +0.2–0.3% of file size.

### 1.3 Baseline vs the ticket's dimensions

- **Streaming writes**: one small `appendFileSync` per completed entry (pi writes complete entries, not partial deltas); cheap; deferred-flush until first assistant message.
- **Full-tree read**: sequential scan, one `JSON.parse` per line (38 ms to *encode* 4,602 entries in the 41 MB file; parse is of the same order — trivial at session scale).
- **Tail read**: seek to end, parse the last line(s); the 1 MB image line is the worst case (parse cost bounded, but it makes "tail" reads occasionally expensive).
- **Branching**: O(1) append, zero rewrites.
- **Compaction**: O(1) append; the compacted stretch stays on disk (audit-friendly; no space reclaim, which at session scale is fine).
- **Crash safety**: process-kill safe by construction (line framing + load-time repair); a mid-line power loss is not recovered (no fsync, no checksum) — a silent-corruption hole (a bit-flip that still parses is undetectable).
- **Human readability**: best possible — grep/jq/vim/diff work out of the box; sessions are a first-class debugging artifact in a dev tool.

## 2. Candidates

### 2.1 Segmented append-only (rotated segments + index)

Primary sources: LevelDB log format — "a sequence of 32KB blocks", each record a 7-byte header `checksum: uint32 // crc32c of type and data[]` + `length: uint16` + `type: uint8` (FULL/FIRST/MIDDLE/LAST so records larger than a block fragment across blocks), zero-filled trailer, "if there is a corruption, skip to the next block" (block-boundary resyncing) ([google/leveldb `doc/log_format.md`](https://github.com/google/leveldb/blob/main/doc/log_format.md), [design doc](https://github.com/google/leveldb/blob/main/doc/design.md): log rotates at ~4 MB, converted to sorted tables); Kafka — partition = append-only log of segment files with sparse offset index; compaction is a background **recopy** of rolled segments that "does not block reads", and "the active segment is never compacted" ([apache/kafka design doc](https://kafka.apache.org/documentation/#design), [source](https://github.com/apache/kafka/blob/trunk/docs/design/design.md)).

- **Streaming writes**: same per-record append, plus block/segment bookkeeping and rotation at size boundaries (LevelDB ~4 MB, Kafka `log.segment.bytes`).
- **Full-tree read**: scan with optional index; block-aligned parsing avoids per-line resync heuristics.
- **Tail read**: open the active segment, read to EOF.
- **Branching**: append (a record may straddle a block boundary → FIRST/MIDDLE/LAST fragmentation, i.e., more bookkeeping than a plain line).
- **Compaction-rewrite**: this is where the pattern earns its keep — background recopy/compaction reclaims space without blocking readers (Kafka log cleaner). At ≤85 MB observed session sizes, that machinery is unneeded overhead.
- **Size**: no payload savings; +2.2% structural overhead (7 B per 32 KB) on top of whatever encoding the records use.
- **Crash safety**: the strongest *framing* of the file-based candidates — per-record CRC32C detects corruption (including silent mid-record bit flips) and truncation at a block boundary is exact.
- **Readability**: binary blocks; strictly worse than JSONL, though a CRC-checked JSONL (see §4) recovers most of the safety benefit for ~0.2% cost.

### 2.2 SQLite (WAL) as the session store

Primary source: [sqlite.org/wal.html](https://www.sqlite.org/wal.html) — changes are appended to a `-wal` file before the database file; "readers do not block writers and a writer does not block readers"; "since there is only one WAL file, there can only be one writer at a time"; automatic checkpoint by default when the WAL reaches 1000 pages; crash recovery runs when the database is next opened; WAL "will not work on a network filesystem" (shared-memory requirement).

- **Streaming writes**: each entry = a transaction (WAL append + index/B-tree maintenance); durable commits cost an fsync per commit at `synchronous=FULL` (or per-checkpoint at `NORMAL`, which trades power-loss durability). Heavier per write than a raw append.
- **Full-tree read**: SQL scan of the entries table + tree rebuild in memory; fine at session scale.
- **Tail read**: no native follow-the-file stream; a live tail means polling `max(id)`/rowid — workable but a protocol, not a file property.
- **Branching**: one row insert (`parent_id` FK) — O(1), no rewrite.
- **Compaction-rewrite**: append-only marker rows work exactly like pi's `firstKeptEntryId` (no rewrite); real space reclamation needs `VACUUM` (whole-file rewrite) — the one place this format forces a big rewrite.
- **Size**: 4 KB pages + B-tree overhead; text stored unescaped (no JSON key/escape overhead) — roughly JSONL-class, ±.
- **Crash safety**: strongest of all candidates (WAL + automatic recovery; ACID across power loss).
- **Readability**: opaque binary; good tooling (sqlite3 CLI, `.dump`, GUIs) but no grep/vim workflow, and a schema to manage/migrate.
- **Concurrency note**: with one DB file per session (tau's model), the multi-process advantage never materializes; if multiple writers share one DB, WAL serializes them (one writer at a time) — it *mediates* concurrency rather than enabling it.

### 2.3 Compact binary encodings (MessagePack / CBOR / FlatBuffers)

Primary sources: [MessagePack spec](https://github.com/msgpack/msgpack/blob/master/spec.md) — "an object serialization specification like JSON"; length-prefixed, self-delimiting, no whitespace/quotes/key-escaping overhead. [CBOR, RFC 8949](https://datatracker.ietf.org/doc/html/rfc8949) — "design goals include the possibility of extremely small code size, fairly small message size, and extensibility without the need for version negotiation"; indefinite-length (streaming) encodings; deterministic (canonical) encoding. [FlatBuffers](https://github.com/google/flatbuffers) — zero-copy access "without parsing/unpacking", but schema-driven (`flatc` codegen) and "the current implementation constructs these buffers backwards (starting at the highest memory address of the buffer)" ([internals](https://github.com/google/flatbuffers/blob/main/docs/internals.md)) — there is no append-friendly file format; each message is a separate finished buffer.

- **Streaming writes**: one length-prefixed record per entry; a torn tail is detectable by declared length (slightly stronger than JSON parse-or-skip).
- **Reads**: decode is cheaper than `JSON.parse` (no grammar), but every consumer (GUI, CLI, shell scripts, other tools) must link a decoder; no ad-hoc inspection.
- **Branching / compaction**: identical append semantics (id/parentId and span pointers inside the record) — no gain over JSONL.
- **Size**: measured **−4% to −14%** vs JSONL, converging to ~−7% on payload-heavy sessions (§1.2). The one structural win: raw image bytes could replace base64 (removes the 33% base64 expansion on the 40.4%-of-file image content) — but that move makes the file unreadable anyway.
- **Crash safety**: length-prefix framing ≈ JSONL line framing, with better torn-tail detection; no checksum, so silent corruption still undetected.
- **Readability**: worst of all candidates — hex blobs; directly against the ticket's note that human-readable files have real value in a dev tool.

## 3. Comparison

| Dimension (ticket) | pi JSONL | Segmented append-only | SQLite (WAL) | MP/CBOR binary |
|---|---|---|---|---|
| Streaming write cost | 1 small append per entry, no fsync | 1 append + block/segment bookkeeping, rotation | transaction + fsync per durable commit; B-tree/index upkeep | 1 append per entry; no fsync |
| Full-tree read | sequential scan + JSON/line parse; ms at 41 MB (measured) | scan, block-resync, optional index | SQL scan; fine | scan + decode (cheaper than JSON parse) |
| Tail read | seek end, parse last line (1 MB worst case) | active segment → EOF | poll `max(id)` — a protocol, not a file property | same as JSONL |
| Branching cost | O(1) append, zero rewrite | append (may fragment across blocks) | 1 row insert | O(1) append |
| Compaction-rewrite cost | O(1) append; no rewrite; no space reclaim | background recopy machinery (Kafka-style) — heavy | append marker (no reclaim) or `VACUUM` (whole-file rewrite) | O(1) append; no reclaim |
| On-disk size | baseline | +~2% structure, no payload savings | ~baseline (pages/B-tree) | **−4…14% measured (→ ~7% on real sessions)** |
| Crash safety | kill-safe (line framing + load repair); silent mid-line corruption undetectable; no power-loss durability | best framing: per-record CRC32C, exact block-boundary truncation | strongest: automatic WAL recovery, ACID incl. power loss | kill-safe via length framing; silent corruption undetectable |
| Human readability / debuggability | **best** (grep/jq/vim/diff, editor-friendly) | poor (binary blocks) | poor (opaque; CLI tooling only) | poor (hex; decoder required everywhere) |
| Concurrent writers (per-session files) | trivially satisfied (one file per session) | same | no gain (one writer per DB anyway) | same |

## 4. Recommendation (applying the user's rule)

**Rule**: "use it if there's a more efficient format" — deviate from pi's JSONL only if a candidate is more efficient *across the ticket's dimensions*.

**Findings against the rule:**

- **MessagePack/CBOR**: more efficient on *size alone* (−4…14% measured, → ~7% on payload-dominated real sessions) but loses the one dimension the ticket explicitly weights for a dev tool — human readability/debuggability — and buys no branching/compaction/concurrency improvement. The residual size gap is payload, not encoding: no encoding change fixes base64 images (40.4% of the largest session measured) or 1 MB single lines.
- **Segmented append-only**: no size gain (payload + ~2% structure); its signature win (CRC32C per record) can be borrowed into JSONL at +0.2% measured cost; its rotation/compaction machinery only pays off at scales (GBs, reader fleets) sessions do not reach (max observed 85 MB).
- **SQLite (WAL)**: the strongest crash-recovery story, but per-commit fsync cost, opaque files, `VACUUM`-only space reclamation, and zero concurrency gain for per-session files — a net loss on write cost, readability, and simplicity at v0 scale.
- **FlatBuffers**: excluded — buffers are built backwards, schema-bound, and there is no append-friendly file format.

No candidate is more efficient *across the dimensions as a whole*, so the rule's condition is not met.

**Recommendation: keep the pi-style JSONL family as tau's v0 session store format** — one file per session; header line; append-only entries `{type, id, parentId, timestamp, ...}`; the tree via `parentId` (branch = new child line, never a rewrite); compaction/OM records as **appended entries that reference the replaced span** (pi's `firstKeptEntryId` pattern, extended with tau's richer OM payload in the entry body/`details` — append-only, audit-friendly, no in-place rewrite, exactly matching the ticket's compaction requirement).

**Two hardenings to adopt** (borrowed from the candidates, no format change):

1. **Per-line integrity field** (LevelDB log-format idea): an optional `crc` field per entry (measured +0.2–0.3% of file size) to detect silent mid-line corruption that parse-or-skip cannot.
2. **Out-of-band sidecar blobs for oversized payloads**: store payloads above a threshold (measured: 83 lines >100 KB, max 1 MB — mostly base64 images, 40.4% of the largest session) as separate raw-byte files referenced by id/path. This is the *real* size lever (raw bytes replace base64's 33% expansion), shrinks the worst-case single write (smaller torn-write blast radius), and keeps tail reads cheap — while the session file itself stays human-readable.

**Revisit triggers** (documented for the future): sessions routinely >~100 MB, multiple processes needing concurrent writes to *one* session file, or structured queries across many sessions — at which point SQLite (WAL) or a LevelDB-style segmented log become worth revisiting.

The choice is delegated per the ticket; the user ratifies at close.

## Sources

1. pi docs, local install: `/Users/aaron/.local/share/nvm/v26.7.0/lib/node_modules/@earendil-works/pi-coding-agent/docs/` — `session-format.md` (format, versions, entry types, file location), `sessions.md` (branching/`/tree`/fork), `compaction.md` (compaction/branch-summary entry structure, `firstKeptEntryId`, `details`, `retainedTail`).
2. pi source: [`earendil-works/pi-mono` `packages/coding-agent/src/core/session-manager.ts`](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/src/core/session-manager.ts), verified against the installed `dist/core/session-manager.js`: per-entry `appendFileSync`, `"wx"` exclusive-create on first flush, malformed-line skip + `"\n"` repair on load, no `fsync` in fs imports.
3. SQLite WAL: <https://www.sqlite.org/wal.html> (WAL append, reader/writer concurrency, one-writer-at-a-time, 1000-page auto-checkpoint, crash recovery, network-filesystem limitation).
4. LevelDB: [`doc/log_format.md`](https://github.com/google/leveldb/blob/main/doc/log_format.md) (32 KB blocks; `crc32c`+`uint16 len`+`uint8 type` record header; FULL/FIRST/MIDDLE/LAST; block-boundary resync on corruption) and [`doc/design.md`](https://github.com/google/leveldb/blob/main/doc/design.md) (~4 MB log rotation → sorted tables).
5. Kafka: design doc (<https://kafka.apache.org/documentation/#design>, [source](https://github.com/apache/kafka/blob/trunk/docs/design/design.md)) — append-only log of segments, sparse offset index, background log-cleaner recopy that doesn't block reads, active segment never compacted.
6. MessagePack spec: <https://github.com/msgpack/msgpack/blob/master/spec.md>.
7. CBOR: RFC 8949, <https://datatracker.ietf.org/doc/html/rfc8949> (goals; indefinite-length "streaming"; deterministic encoding).
8. FlatBuffers: <https://github.com/google/flatbuffers> (README: zero-copy without parsing; [internals](https://github.com/google/flatbuffers/blob/main/docs/internals.md): buffers constructed backwards).
9. Local measurement (this research): re-encoding of real pi session files (280-file corpus under `~/.pi/agent/sessions`; three sample files across sizes) — script at `/tmp/research/measure/measure.mjs` (re-runnable; `@msgpack/msgpack` + a minimal RFC 8949 definite-length CBOR encoder); encode timings are indicative (encoder-specific), sizes are exact.
