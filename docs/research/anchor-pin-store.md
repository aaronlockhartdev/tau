# What store should the hashline anchor pinning store use?

Research for [aaronlockhartdev/tau#207](https://github.com/aaronlockhartdev/tau/issues/207) (hashline v1: the pinning store).
Date: 2026-10-19. Method: primary sources (the session-storage-format research that fixed tau's JSONL convention, the reference fork's SQLite `hash-store.ts`, SQLite/LevelDB/SQLite-in-Tauri docs) plus the workload's actual shape.

## Brief (from ticket #207 §1)

The hashline anchor system needs *persistent* anchor state: a file's anchors must stay stable across sessions while its lines are unchanged. The in-memory registry (already landed, #207 prerequisite cluster) is session-scoped and gone with the process. The pinning store persists per-file anchor assignments. Requirements:

- **Write pattern:** one small record per file, written after every successful `edit` and after every `read` of a previously-unpinned file. Low frequency (per edit, not per token).
- **Read pattern:** the whole record for one file, loaded on the first `read`/`edit` of that file in a session. Records are KB-range (a 10k-line file ≈ 400 KB of 4-letter anchors + 16-hex checksums).
- **Isolation:** per workspace (`.tau/`), one record per file path. No cross-file queries, no joins.
- **Crash safety:** a torn write must never produce a half-record that a later session trusts.
- **Debuggability:** "human-readable files have real value in a dev tool" (the session-storage research rule) — an anchor file a developer can open and read.
- **No new dependency:** the bar set by the session store (JSONL + CRC32 + zstd, std + xxhash only).

The question: JSONL sidecar, SQLite, or something else?

## 1. Baseline: the session-storage convention

`docs/research/session-storage-format.md` (for #3) already ran this exact analysis for session transcripts and settled on **JSONL + per-line CRC32 + atomic temp-file+rename writes + zstd sidecar blobs**, with std + `xxhash` + `zstd` as the only dependencies. The decision rule there: deviate from the JSONL baseline only if a candidate is more efficient across the workload's dimensions.

The pinning store's workload is a strict subset of the session store's:

| Dimension | Session store | Pin store |
|---|---|---|
| Write frequency | per entry (many per turn, streaming) | per edit (few per session) |
| Write size | one entry (KB) | one file record (KB–400 KB) |
| Read pattern | full tree + incremental tail | whole record, once per session per file |
| Branching/compaction | yes (the hard part) | no |
| Concurrency | multiple sessions | one process per workspace (the agent) |

Everything that made JSONL the right call for sessions — cheap whole-record rewrites, no server, human-readable, zero new deps, atomic rename for crash safety — holds a fortiori for pins, which are simpler.

## 2. The alternative: SQLite

The reference fork (`/tmp/pi-hashline-edit-pro/src/hash-store.ts`, 671 LOC) uses SQLite (WAL, `synchronous=NORMAL`, `secure_delete=ON`, `auto_vacuum=INCREMENTAL`, a `snapshots` table, an `undo` table, a `meta` table, corruption quarantine via `PRAGMA quick_check`, busy-timeout retry, legacy JSONL migration). That is a substantial machinery, and most of it solves problems tau's workload does not have:

- **Server/process lifetime:** the fork's store outlives the agent process and is shared across invocations in a way that needs connection lifecycle, busy-retry, and WAL. Tau's store is read-modify-write within one process, per file.
- **Cross-file queries:** the fork's `snapshots` table supports listing/searching across files. Tau's access is always "give me file X's record" — a hash-named file is the index.
- **Dependency cost:** `rusqlite` (bundled) or `libsqlite3-sys` pulls a C compiler + ~1 MB of SQLite into every build, and a binary blob that fails the debuggability criterion. The session store's rule was explicit: no new dependency without a demonstrated efficiency gain.

What SQLite *would* buy at this scale: nothing. A 400 KB record read/written a few times per session per file is 400 KB of file I/O either way; the SQL layer adds latency, not removes it.

## 3. Decision

**JSONL sidecar, one CRC32-guarded record per file, atomic temp+rename writes** — the session-store convention, applied at a smaller scale:

- One file per pinned path: `.tau/anchors/{xxh3-64(path)}.jsonl` (the raw path is not a safe file name; the hash is).
- One JSON record per file: `{path, checksum, line_count, pins: [{anchor, checksum, tombstone}], crc}`.
- **Integrity:** an embedded `crc` field = CRC32 (IEEE) of the canonical record with the field empty (the `session/entry.rs` scheme, re-implemented in `pin_store` so the module is self-contained). A load that fails verification drops the record (the caller re-mints) — the store is a cache, not a source of truth.
- **Crash safety:** temp file in the same dir + `fsync` + `rename` (the session store's `writeAtomic`).
- **Best-effort semantics:** a store I/O failure degrades to the in-memory registry path (fresh allocation) — it never fails the `read`/`edit` tool. The store is an optimization for anchor stability, not a correctness dependency.

Reconsider if: the workload grows cross-file queries (a "which files pin this anchor?" index), the record count outgrows directory-listing cost, or a corruption mode appears that CRC32 + atomic rename does not cover. At that point SQLite (or a single `anchors.json` with an in-process index) is the escalation path — but not before.
