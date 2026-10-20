# Migrating the anchor pin store from JSONL sidecar to a database

Research for the escalation path named in ADR-0031 and `docs/research/anchor-pin-store.md` §3 ("reconsider if … the record count outgrows directory-listing cost, … a corruption mode appears that CRC32 + atomic rename does not cover").
Date: 2026-10-20. Method: primary sources — the current pin store implementation, the reference fork's SQLite `hash-store.ts` (re-read in full), the tau config system, crates.io state of the candidate drivers — plus the workload's actual shape.

## Brief

The hashline anchor pin store (`crates/tau-core/src/hashline/pin_store.rs`) persists per-file anchor pins as one CRC32-guarded JSON record per file in `.tau/anchors/{xxh3-64(path)}.jsonl` (ADR-0031). The decision to move to a database is made; this doc designs the migration:

- **Which database**, and which Rust driver.
- **Schema** for pins.
- **Garbage collection / expiry** of sidecar records (the store today never forgets: a `write` drops one file's record, but records for files deleted from disk, moved, or never again read accumulate forever).
- **Config surface** for the new behavior.

Out of scope: implementing the migration (this is a research + design ticket), the fork's `undo` table (tau has no undo feature), and ADR-0031's tombstone semantics (unchanged).

## 1. The workload (unchanged from the 2026-10-19 analysis)

- **Write pattern:** one small record per file, written after every successful `edit` and after every `read` of a previously-unpinned file. Low frequency (per edit, not per token).
- **Read pattern:** the whole record for one file, loaded on the first `read`/`edit` of that file in a session. Records are KB-range (a 10k-line file ≈ 400 KB of 4-letter anchors + 16-hex checksums).
- **Isolation:** per workspace (`.tau/`), one record per file path. Access is always "give me file X's record"; there are no cross-file queries in the hot path.
- **Crash safety:** a torn write must never produce a half-record that a later session trusts.
- **Concurrency:** one agent process per workspace.

What the JSONL layout costs at scale, which motivates the move:

1. **Directory listing.** `PinStore::paths()` enumerates the `anchors/` subdir — fine at hundreds of records, O(N) `stat`/open churn at tens of thousands. This is ADR-0031's first escalation trigger.
2. **No lifecycle.** Nothing ever reclaims records for files that no longer exist; the directory grows monotonically with every file the agent has ever touched in the workspace.
3. **Per-record integrity plumbing.** The hand-rolled CRC32 + canonical-JSON re-serialization guards each file against partial writes; a database gives the same guarantee (WAL + atomic page writes) for the whole store, plus `PRAGMA quick_check` for the corruption case that per-file CRC does not cover (a torn *directory* state, a half-migrated file).

## 2. Which database

Candidates for a single-process, per-workspace, KV-shaped workload in a Tauri Rust core:

| Candidate | Shape | C dep? | Verdict |
|---|---|---|---|
| **SQLite via `rusqlite` (bundled)** | relational, one file | no (bundled compiles SQLite from source) | **Recommended** |
| `redb` 4.4.0 | embedded KV/B-tree, mmap | no (pure Rust) | Viable alternative |
| JSONL (status quo) | per-file records | no | Retired |
| One big `anchors.json` | single record blob | no | No — serializes every file's write behind one lock and rewrites the whole workspace on every edit (ADR-0031 already rejected this shape) |

**Recommendation: SQLite, via `rusqlite` with the `bundled` feature.**

Reasons:

- **The reference fork's store is SQLite, and its machinery is the design we want to inherit.** The fork's `src/hash-store.ts` (671 LOC, re-read in full for this doc) is battle-tested at exactly this workload shape — per-path snapshot rows, an LRU-bounded in-process cache, WAL mode, `synchronous=NORMAL`, `secure_delete=ON`, `auto_vacuum=INCREMENTAL` + opportunistic `incremental_vacuum`/`VACUUM` when the freelist passes 20% of pages, `busy_timeout` retry, `PRAGMA quick_check`-gated corruption quarantine (rename the db + `-wal` + `-shm` to `*.corrupt-{ts}-{pid}-{uuid}` and rebuild), a `meta` version row with nuke-on-mismatch, and a legacy-file migration on first open. Every one of those is a standard SQLite feature available to `rusqlite` unchanged; the only fork-specific parts are Node's `node:sqlite`/`bun:sqlite` engine selection and the JS glue, which do not port and do not need to.
- **No system dependency.** `bundled` compiles SQLite into the build; no `libsqlite3` on the target machine, no C-compiler requirement beyond what Rust's own toolchain already is. (The 2026-10-19 doc's "C-compiled binary dependency" objection is real for `libsqlite3-sys`'s default non-bundled mode; `bundled` is the mode Tauri apps standardly use, and the binary is already carrying a ~1 MB SQLite that is now well understood.)
- **Crash safety is the point of the move.** WAL gives atomic, crash-consistent writes for the whole store; a torn write leaves a valid last committed state, not a half-record. `quick_check` covers the corruption mode that per-file CRC32 structurally cannot.
- **`redb`** is a legitimate pure-Rust alternative (mmap, no C, small) and would serve the hot path fine — the schema below is a one-table KV, which redb does. The deciding factors are (a) the fork's corruption/quarantine/vacuum story is written against SQLite and porting it to redb's API surface (no SQL, no `quick_check` equivalent, its own durability model) means re-deriving machinery that already exists, and (b) if any future feature wants a real cross-file query ("which files pin this anchor?"), SQL is already there. If dependency-minimalism is re-weighted above mechanism-inheritance, redb is the fallback; the ticket should name the chosen driver, and this doc's recommendation is `rusqlite/bundled`.

## 3. Schema

One database file per workspace: **`.tau/anchors.sqlite`** (sibling of the current `anchors/` directory; same per-workspace isolation).

```sql
PRAGMA journal_mode = WAL;          -- crash-safe, concurrent-read friendly
PRAGMA synchronous = NORMAL;        -- safe under WAL (fork's setting)
PRAGMA secure_delete = ON;          -- no plaintext fragments after DELETE
PRAGMA auto_vacuum = INCREMENTAL;   -- set at create, before any write (fork's setting)

CREATE TABLE IF NOT EXISTS anchors (
    path        TEXT PRIMARY KEY,   -- the file path the pins belong to
    checksum    TEXT NOT NULL,      -- xxh3-64 over the newline-joined per-line checksums (today's `file_checksum`)
    line_count  INTEGER NOT NULL,
    pins        TEXT NOT NULL,      -- JSON array of {anchor, checksum, tombstone}, line order
    updated_at  INTEGER NOT NULL    -- epoch ms
);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- meta: ('version', '1'); on open, a mismatched version → DELETE FROM anchors
-- (the record is a cache: re-minting is always a valid degradation, same as a
-- CRC-failed JSONL record today).
```

Notes:

- **`pins` stays a JSON blob column**, not a per-pin table. The access pattern is whole-record-only (the registry seeds a file's *entire* previous allocation for the LCS re-anchor); there is no per-pin query. This mirrors the fork's `hashes`/`line_checksums` JSON columns and keeps the row a single atomic unit. A per-pin table would only add write amplification.
- **No `snapshots`-style checksum guard column duplication**: the `checksum` + `line_count` pair is what the registry's `seed_from_store` compares against the current file; both are stored so a seed can be rejected without parsing `pins` for the length.
- **The `PinStore` public API is the contract that does not move.** `new(root)`, `file_path(path)`, `load(path) -> Option<FilePins>`, `save(&FilePins)`, `remove(path)`, `paths() -> Vec<String>` all keep their signatures (`file_path` returns the db file path; `paths()` becomes `SELECT path FROM anchors`). `registry.rs` (`seed_from_store`, `persist_pins`, `free`) and `agent.rs:236` (construction) are unchanged; the `FilePins`/`Pin` types are unchanged — only the serialization behind `load`/`save` changes (serde over the `pins` column instead of canonical-JSON + CRC over the whole record).
- **Tombstones are unchanged** (ADR-0031): deleted lines' slots are kept and flagged in the `pins` JSON for the record's life; a `write` still drops the whole row via `remove`.
- **Best-effort semantics unchanged:** any store error (open, query, write) degrades to the in-memory registry path (fresh allocation) and never fails the `read`/`edit` tool. A `StoreError` variant for "db unavailable/corrupt-after-quarantine" replaces the I/O-flavored ones; the registry treats them identically.
- **Single synchronous connection** for the process's lifetime (opened lazily on first store use, closed on session teardown): one process per workspace, writes are per-edit, reads once per session per file — the pool/connection-lifecycle machinery the fork needs across Node invocations is not needed in a long-lived Rust core.

## 4. Garbage collection and expiry

The store today is append-only in effect: records accumulate for every file ever pinned. The DB makes two sweeps cheap, both run **at session start** (the fork runs them at boot, `index.ts:190–192`), best-effort, before any tool call:

1. **Missing-file sweep (always on).** `SELECT path FROM anchors` → `stat` in batches of 64 (the fork's `STAT_BATCH`) → `DELETE FROM anchors WHERE path IN (…)`. For each deleted path, also release the in-memory registry's ownership if the file was already served this session (the fork's `freeAnchorsForPaths` — tau's analogue is `Registry::free`, which today already does both in-memory eviction and store removal). Files deleted from disk between sessions are the target; a moved/renamed file is a new path to the store, so its old record is pruned by the same sweep.
2. **Age expiry (config-gated).** `DELETE FROM anchors WHERE updated_at < ?` with `? = now − retention_days`. This bounds the store for files that still exist on disk but are never touched again (stale pins only cost seed-time comparison work, but the rows and their `pins` blobs accumulate without bound). `retention_days = 0` disables the sweep (missing-file sweep only).

Both sweeps log what they reclaimed (`tracing::info!` with counts/paths, capped like the reclaim warning). Neither may fail the session: a sweep error is logged and the session proceeds (the store is a cache).

Vacuum: on open, if `freelist_count > 50 && freelist * 5 > page_count`, run `incremental_vacuum(50)` (fallback `VACUUM`) — the fork's exact opportunistic rule, cheap, keeps the file from growing monotonically as rows churn.

## 5. Configuration

A new `[hashline]` section in the existing TOML config system (`crates/tau-core/src/config.rs`, spec §12 layering: system `~/.config/tau/config.toml` with project `.tau/config.toml` on top). Follows the existing section pattern exactly: a struct with `#[serde(default, deny_unknown_fields)]`, an `Option` field on `ConfigFile`, a merge in `ConfigFile::merge_into`, and a field on `Config`.

```toml
[hashline]
# Delete pin records not touched within this many days (session-start sweep).
# 0 disables age expiry; the missing-file sweep always runs.
retention_days = 180
# Run the session-start GC sweeps (missing files, age expiry).
gc_on_start = true
```

- `retention_days: u32 = 180` (default) — long enough that a project parked for a sprint keeps its anchors, short enough that a workspace's store converges on the files it is actively working. (The fork's undo-retention default is 30 days; pins are cheaper to re-mint than undo content is to lose, so the pin default is more generous.)
- `gc_on_start: bool = true`.
- Delivery to the session: `SessionParams` (built in `crates/tau-core/src/harness/launch.rs`) is the seam where config already reaches the session (it carries `gates`, `om`, `tool_batch_on_force`); it gains the resolved hashline settings, and `agent.rs:236` passes them to `PinStore` (e.g. `PinStore::new(root, GcPolicy { … })`). The ACP path's `Registry::new` (no store) is untouched.

## 6. Migration from the JSONL sidecar

On first open of `.tau/anchors.sqlite` when a legacy `anchors/` directory exists (fork pattern: migrate only when the db did not pre-exist):

1. Read each `anchors/*.jsonl` file; **CRC-verify** with the existing `pin_store` verification (a corrupt legacy record is skipped, not imported — same drop-and-remint rule as today).
2. Bulk-upsert the valid records into `anchors` in one transaction (`updated_at` = file mtime, falling back to now).
3. Rename the legacy directory to `anchors/legacy-{epoch-ms}` (kept for inspection; the fork renames its legacy file `.bak` — keeping the directory is strictly more conservative and it is a workspace-local dot-dir, not the repo).

A second launch sees no `anchors/` dir and no migration runs. No version flag needed in `meta` for this: presence of the legacy dir is the trigger, and it is consumed by the rename.

## 7. What changes in code (for the implementer's orientation)

- **New:** `rusqlite = { version = "0.40", features = ["bundled"] }` in the workspace + `tau-core` (the only new dependency; `redb` 4.4.0 if that driver wins the review).
- **Rewritten internals, unchanged API:** `hashline/pin_store.rs` — the JSONL read/write/CRC/atomic-rename body becomes open/WAL-PRAGMAs/version-check/quarantine, `load` = `SELECT … WHERE path = ?`, `save` = upsert, `remove` = `DELETE`, `paths()` = `SELECT path`; plus the two GC sweeps and the legacy import. The module's self-contained CRC32 becomes dead code and goes with the JSONL path (it is re-implemented from `session/entry.rs` per ADR-0031; the session store keeps its own copy — no sharing, per the ADR).
- **Untouched:** `hashline/registry.rs` (all store call sites keep their signatures), `hashline/mod.rs`, `agent.rs`, the tools, the ACP path.
- **Config:** the `[hashline]` section as in §5.
- **Tests:** the existing pin-store tests (CRC verification, atomicity, tombstones, reanchor) re-express against the db; new tests for: version-mismatch nuke, quarantine-on-`quick_check`-failure, missing-file sweep (positive + negative: existing files survive), age expiry (positive + `retention_days = 0` negative), legacy import (valid + corrupt-skip), API-shape invariance (a registry seeded through the db store behaves as before). Positive and negative per behavior, per the testing conventions.

## 8. Residual risks

- **A new binary dependency in every build** (the fork's SQLite, ~1 MB). Accepted: it buys the crash-safety and corruption-detection that motivated the move, and `bundled` keeps it self-contained.
- **Debuggability** drops from "open a human-readable JSON file" to "run a sqlite3 CLI on the file" — a real cost against the session-store research rule; the `pins` column keeps the per-record content at least `sqlite3 .dump`-able, and the record is an optimization, not a source of truth.
- **WAL sidecar files** (`-wal`, `-shm`) appear in `.tau/` — the quarantine path already handles all three; `.gitignore`-worthy since they live under the workspace's `.tau/` dot-dir, not in a repo.
- **If the workload never actually outgrows the JSONL layout**, this was a premature escalation of ADR-0031. The missing-file GC is the one piece that is a pure win even at small scale (the store is unbounded today either way); the database is the piece that pays off at scale. The design keeps the blast radius small (one module's internals + one config section) so the cost of that being true is bounded.
