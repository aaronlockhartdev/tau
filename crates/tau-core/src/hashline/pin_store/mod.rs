//! The persistent anchor pinning store (spec §5.4, #207): the step up from
//! session-scoped, in-memory anchor state to *persistent* state. The
//! in-memory registry (item 2) is gone with the process; this store persists
//! a file's pinned anchor assignments across sessions, so a line's anchor
//! stays stable while its content is unchanged — even when neighbors change
//! or the file is reformatted. That is what stateless `line_hashes` cannot
//! do: its allocation depends on the whole preceding file, so one change
//! churns every anchor after it.
//!
//! **Store technology (see `docs/research/anchor-store-db-migration.md`).**
//! One SQLite file per workspace at `{workspace}/.tau/anchors.sqlite`
//! (`rusqlite`, bundled): WAL + `synchronous=NORMAL` for crash safety,
//! `secure_delete=ON`, `auto_vacuum=INCREMENTAL` with an opportunistic
//! `incremental_vacuum` when the freelist grows, `PRAGMA quick_check`-gated
//! corruption quarantine (rename db + `-wal` + `-shm` aside, rebuild), and a
//! `meta` version row with nuke-on-mismatch. The JSONL sidecar and its
//! per-record CRC32 are retired with this migration.
//!
//! **Pin identity (the last churn source, #207 §2).** A pin is
//! *content-addressed*: a line's anchor is tied to its line checksum
//! (xxh64 of the canon), not to its position. Re-anchoring after an edit
//! matches the new file's lines against the previous pins **by checksum, in
//! order** (the same LCS-over-checksums the registry uses, item 6): an
//! unchanged line keeps its exact anchor no matter where it moved; a changed
//! line gets a fresh anchor; a deleted line's slot is tombstoned, not
//! recycled. Inserting or deleting a line shifts the *positions* after it,
//! but the surviving pins do so by content, so the shift does not churn them.
//!
//! **Garbage collection (#225).** At session start the store sweeps rows
//! whose file no longer exists on disk (always on) and, when configured,
//! rows untouched for longer than the retention window. Best-effort: a
//! sweep error is logged and the session proceeds.
//!
//! **Placement:** one database file per workspace (`{workspace}/.tau/
//! anchors.sqlite`), the workspace scope the session store uses
//! (`session.rs`, spec §3). One synchronous connection for the store's
//! lifetime; a row is a file's whole pin record (`pins` a JSON blob column).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

/// One pinned anchor: the 4-letter anchor, the checksum of the line it was
/// minted for (the pin's content identity), and a tombstone flag — a freed
/// slot is kept (not recycled) for the session/file lifetime (#207 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub anchor: String,
    pub checksum: String,
    /// The slot was freed (its line deleted) and must not be re-minted.
    #[serde(default)]
    pub tombstone: bool,
}

/// A file's pinned anchor set: one record in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePins {
    /// Path (as the tool layer keys it) the pins belong to.
    pub path: String,
    /// Checksum of the whole file content the pins were computed for; a
    /// mismatch means the file drifted and the pins must re-anchor.
    pub checksum: String,
    pub line_count: usize,
    pub pins: Vec<Pin>,
    /// The JSONL sidecar's CRC32 field; kept so the serde shape is stable
    /// for the in-memory flow — the SQLite row carries no integrity field
    /// (WAL + atomic page writes replace the per-record CRC).
    #[serde(default)]
    pub crc: Option<String>,
}

impl FilePins {
    /// The per-line checksums in file order — the re-anchor base.
    #[must_use]
    pub fn checksums(&self) -> Vec<String> {
        self.pins.iter().map(|p| p.checksum.clone()).collect()
    }

    /// The per-line anchors in file order.
    #[must_use]
    pub fn anchors(&self) -> Vec<String> {
        self.pins.iter().map(|p| p.anchor.clone()).collect()
    }
}

/// The GC policy a store runs at session start (#225, the `[hashline]`
/// config section): `gc_on_start` gates both sweeps, `retention_days` the
/// age sweep (0 disables it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcPolicy {
    pub retention_days: u32,
    pub gc_on_start: bool,
}

impl Default for GcPolicy {
    fn default() -> Self {
        Self {
            retention_days: 180,
            gc_on_start: true,
        }
    }
}

/// A pin-store error: the store is best-effort, so callers degrade to the
/// in-memory registry path (fresh allocation) rather than failing the tool.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StoreError {}

/// The persistent pin store: one SQLite file per workspace, one row per file.
#[derive(Debug)]
pub struct PinStore {
    root: PathBuf,
    conn: std::sync::Mutex<Option<Connection>>,
}

/// Epoch ms since the Unix epoch (0 on clock failure).
fn now_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).map_or(0, |s| s * 1000),
        Err(_) => 0,
    }
}

impl PinStore {
    /// Open (creating) the store at `root` (the workspace's `.tau` dir) and
    /// run the session-start GC sweeps per `policy` (best-effort).
    pub fn new(root: &Path, policy: GcPolicy) -> Self {
        let root = root.to_path_buf();
        let store = Self {
            root: root.clone(),
            conn: std::sync::Mutex::new(None),
        };
        {
            let mut slot = store
                .conn
                .lock()
                .expect("the mutex is not poisoned: a store is single-threaded");
            if let Ok(c) = Self::open_store(&root) {
                *slot = Some(c);
            }
        }
        store.run_gc(policy);
        store
    }

    /// The db file: `{root}/anchors.sqlite` (sibling of the retired
    /// `anchors/` directory; the raw path is not a safe file name, so one
    /// file for the whole workspace).
    pub fn file_path(&self, _path: &str) -> PathBuf {
        Self::db_path(&self.root)
    }

    fn db_path(root: &Path) -> PathBuf {
        root.join("anchors.sqlite")
    }

    /// Open the database: pragmas, the schema, the corruption quarantine,
    /// and the version check.
    fn open_store(root: &Path) -> Result<Connection, StoreError> {
        fs::create_dir_all(root).map_err(|e| StoreError(e.to_string()))?;
        let db = Self::db_path(root);
        if db.exists() && !Self::is_healthy(&db) {
            // A quick_check failure is the corruption mode the old per-file
            // CRC32 structurally could not cover (a torn database file):
            // quarantine and rebuild. The record is a cache — re-minting is
            // a valid degradation, as a CRC-failed JSONL record was.
            Self::quarantine(root, &db);
        }
        let conn = Connection::open(&db).map_err(|e| StoreError(e.to_string()))?;
        Self::apply_pragmas(&conn);
        Self::ensure_schema(&conn);
        Self::check_version(&conn);
        Ok(conn)
    }

    /// `PRAGMA quick_check` reports `ok` per row; any other line is a
    /// corruption finding ("file is not a database", corrupt page, …).
    fn is_healthy(db: &Path) -> bool {
        let Ok(conn) = Connection::open(db) else {
            return false;
        };
        conn.prepare("PRAGMA quick_check")
            .and_then(|mut m| {
                m.query_map(params![], |r| r.get::<_, String>(0))
                    .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
            })
            .is_ok_and(|rows| rows.iter().all(|l| l == "ok"))
    }

    /// Rename the db (and its WAL sidecars) to
    /// `anchors.sqlite.corrupt-{ts}-{pid}-{rand}` so a corrupted file is
    /// inspectable and cannot be opened again.
    fn quarantine(root: &Path, db: &Path) {
        let base = db
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let ts = now_ms();
        let stem = format!(
            "{base}.corrupt-{ts}-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        );
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::rename(
                root.join(format!("{base}{suffix}")),
                root.join(format!("{stem}{suffix}")),
            );
        }
    }

    /// `auto_vacuum` only takes effect before the first write, so it goes in
    /// before the schema DDL.
    fn apply_pragmas(conn: &Connection) {
        let _ = conn.pragma_update(None, "auto_vacuum", "INCREMENTAL");
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let _ = conn.pragma_update(None, "secure_delete", "ON");
        let _ = conn.busy_timeout(std::time::Duration::from_millis(2000));
    }

    fn ensure_schema(conn: &Connection) {
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS anchors (
                path        TEXT PRIMARY KEY,
                checksum    TEXT NOT NULL,
                line_count  INTEGER NOT NULL,
                pins        TEXT NOT NULL,
                updated_at  INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             INSERT OR IGNORE INTO meta (key, value) VALUES ('version', '1');",
        );
    }

    /// A `meta` version the store does not recognize drops every row: the
    /// store is a cache, so re-minting is the degradation.
    fn check_version(conn: &Connection) {
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'version'",
                rusqlite::params![],
                |r| r.get(0),
            )
            .ok();
        if version.as_deref() != Some("1") {
            let _ = conn.execute("DELETE FROM anchors", rusqlite::params![]);
        }
    }

    /// The session-start sweeps (research doc §4): prune rows whose file is
    /// gone, then rows older than the retention window, then vacuum if the
    /// freelist has grown. Best-effort throughout.
    fn run_gc(&self, policy: GcPolicy) {
        if !policy.gc_on_start {
            return;
        }
        let guard = self
            .conn
            .lock()
            .expect("the mutex is not poisoned: a store is single-threaded");
        let Some(conn) = guard.as_ref() else {
            return;
        };
        // 1. Missing-file sweep (always on): stat the rows' paths in batches
        //    of 64 and delete the rows whose file is gone.
        let paths: Vec<String> = conn
            .prepare("SELECT path FROM anchors")
            .and_then(|mut m| {
                m.query_map(params![], |r| r.get(0))
                    .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
            })
            .unwrap_or_default();
        let mut missing = Vec::new();
        for chunk in paths.chunks(64) {
            for path in chunk {
                if !Path::new(path).exists() {
                    missing.push(path.clone());
                }
            }
        }
        if !missing.is_empty() {
            let sql = format!(
                "DELETE FROM anchors WHERE path IN ({})",
                missing
                    .iter()
                    .map(|_| "?".to_owned())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if let Ok(n) = conn.execute(&sql, rusqlite::params_from_iter(missing.iter())) {
                tracing::info!(count = n, "pin store gc: pruned missing files");
            }
        }
        // 2. Age expiry (config-gated; 0 disables).
        if policy.retention_days > 0 {
            let cutoff = now_ms() - i64::from(policy.retention_days) * 24 * 60 * 60 * 1000;
            if let Ok(n) = conn.execute(
                "DELETE FROM anchors WHERE updated_at < ?1",
                rusqlite::params![cutoff],
            ) {
                tracing::info!(count = n, "pin store gc: pruned stale records");
            }
        }
        // 3. Opportunistic vacuum (the fork's rule): the freelist past 50
        //    pages and a fifth of the db → reclaim 50 pages.
        let freelist: i64 = conn
            .prepare("PRAGMA freelist_count")
            .and_then(|mut m| m.query_row(rusqlite::params![], |r| r.get(0)))
            .unwrap_or(0);
        let pages: i64 = conn
            .prepare("PRAGMA page_count")
            .and_then(|mut m| m.query_row(rusqlite::params![], |r| r.get(0)))
            .unwrap_or(0);
        if freelist > 50 && freelist * 5 > pages {
            if conn
                .execute("PRAGMA incremental_vacuum(50)", rusqlite::params![])
                .is_ok()
            {
                tracing::info!(pages = freelist, "pin store gc: incremental vacuum");
            } else {
                let _ = conn.execute("VACUUM", rusqlite::params![]);
            }
        }
    }

    fn open_conn(&self) -> Result<std::sync::MutexGuard<'_, Option<Connection>>, StoreError> {
        let mut guard = self
            .conn
            .lock()
            .expect("the mutex is not poisoned: a store is single-threaded");
        if guard.is_none() {
            *guard = Some(Self::open_store(&self.root)?);
        }
        Ok(guard)
    }

    /// Load `path`'s pins. `None` if the store has no row for it (first
    /// contact → the caller mints fresh) or the store is unavailable (the
    /// caller re-mints). A missing record is not an error.
    #[must_use]
    pub fn load(&self, path: &str) -> Option<FilePins> {
        let guard = self.open_conn().ok()?;
        let conn = guard.as_ref()?;
        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT path, checksum, line_count, pins FROM anchors WHERE path = ?1",
                rusqlite::params![path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok()?;
        let pins: Vec<Pin> = serde_json::from_str(&row.3).ok()?;
        (row.0 == path).then_some(FilePins {
            path: row.0,
            checksum: row.1,
            line_count: usize::try_from(row.2).unwrap_or(0),
            pins,
            crc: None,
        })
    }

    /// Persist `pins` for `path`: an upsert (one row per file, last write
    /// wins) — SQLite's transactional pages are the crash-safety the JSONL
    /// atomic rename used to provide.
    pub fn save(&self, pins: &FilePins) -> Result<(), StoreError> {
        let guard = self.open_conn()?;
        let conn = guard.as_ref().expect("open_conn leaves the connection set");
        let pins_json = serde_json::to_string(&pins.pins).expect("Vec<Pin> is always serializable");
        conn.execute(
            "INSERT INTO anchors (path, checksum, line_count, pins, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                 checksum = excluded.checksum,
                 line_count = excluded.line_count,
                 pins = excluded.pins,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                pins.path,
                pins.checksum,
                i64::try_from(pins.line_count).unwrap_or(i64::MAX),
                pins_json,
                now_ms()
            ],
        )
        .map(|_| ())
        .map_err(|e| StoreError(e.to_string()))
    }

    /// Drop `path`'s row (a `write` voids every anchor).
    pub fn remove(&self, path: &str) {
        let Ok(guard) = self.open_conn() else {
            return;
        };
        let Some(conn) = guard.as_ref() else {
            return;
        };
        let _ = conn.execute(
            "DELETE FROM anchors WHERE path = ?1",
            rusqlite::params![path],
        );
    }

    /// Every path this store has a row for (for tests / tooling).
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        let Ok(guard) = self.open_conn() else {
            return Vec::new();
        };
        let Some(conn) = guard.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let Ok(mut stmt) = conn.prepare("SELECT path FROM anchors ORDER BY path") else {
            return out;
        };
        let Ok(rows) = stmt.query_map(params![], |r| r.get(0)) else {
            return out;
        };
        for p in rows.flatten() {
            out.push(p);
        }
        out
    }
}

/// The whole-file checksum a pin set is computed for: xxh3-64 over the
/// per-line checksums in order (content-addressed, position-sensitive — any
/// line change moves it).
#[must_use]
pub fn file_checksum(checksums: &[String]) -> String {
    let joined = checksums.join("\n");
    format!("{:16x}", xxh3_64(joined.as_bytes()))
}

/// Re-anchor a new file's lines against a previous pin set: a line whose
/// checksum matches a previous pin **in order** keeps that pin's anchor
/// (content-addressed, position-independent); every other line is marked for
/// a fresh mint. Returns `(matched, fresh)` index sets over the new lines —
/// the caller does the minting (it owns the global anchor space).
#[must_use]
pub fn reanchor(prev: &FilePins, checksums: &[String]) -> (Vec<bool>, Vec<bool>) {
    use crate::hashline::registry::match_anchors;
    let prev_sums = prev.checksums();
    let matched_idx = match_anchors(&prev_sums, checksums);
    let mut matched = vec![false; checksums.len()];
    let mut fresh = vec![false; checksums.len()];
    for (j, m) in matched_idx.iter().enumerate() {
        match m {
            Some(_) => matched[j] = true,
            None => fresh[j] = true,
        }
    }
    (matched, fresh)
}

/// Build the next `FilePins` from a previous set and the re-anchor result:
/// survivors keep their anchor, fresh lines get `minted[j]`, and a pin whose
/// checksum vanished is tombstoned (kept, not recycled). `minted` must be
/// index-aligned with `checksums` (the caller mints for every line; matched
/// lines ignore their `minted` value and keep the survivor's anchor).
#[must_use]
pub fn next_pins(
    prev: &FilePins,
    matched: &[bool],
    minted: &[String],
    checksums: &[String],
) -> FilePins {
    // Map previous pins to their (old) line index by checksum; a survivor at
    // new index j reuses the pin it matched.
    let prev_sums = prev.checksums();
    let mut by_sum: HashMap<&str, (usize, &Pin)> = HashMap::new();
    for (i, s) in prev_sums.iter().enumerate() {
        by_sum.entry(s.as_str()).or_insert((i, &prev.pins[i]));
    }
    let mut pins = Vec::with_capacity(checksums.len());
    let mut used_prev: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (j, sum) in checksums.iter().enumerate() {
        let pin = if matched[j] {
            let (i, p) = by_sum
                .get(sum.as_str())
                .expect("a match implies a previous pin with this checksum");
            used_prev.insert(*i);
            Pin {
                anchor: p.anchor.clone(),
                checksum: sum.clone(),
                tombstone: false,
            }
        } else {
            Pin {
                anchor: minted[j].clone(),
                checksum: sum.clone(),
                tombstone: false,
            }
        };
        pins.push(pin);
    }
    // Tombstone the pins whose checksum is gone (their line was deleted):
    // kept in the set, flagged, and never re-minted.
    for (i, p) in prev.pins.iter().enumerate() {
        if !used_prev.contains(&i) && !p.tombstone {
            pins.push(Pin {
                anchor: p.anchor.clone(),
                checksum: p.checksum.clone(),
                tombstone: true,
            });
        }
    }
    FilePins {
        path: prev.path.clone(),
        checksum: file_checksum(checksums),
        line_count: checksums.len(),
        pins,
        crc: None,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
