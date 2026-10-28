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
//! **Pin identity, the record, and its generation math** live in `record`:
//! content-addressed pins, re-anchored by checksum in order (#207 §2).
//!
//! **The record is bounded (#310).** Every generation appends the slots it
//! freed, and #304 made that claim last as long as the record does, so
//! `save` caps the graveyard on the way to disk: at most `MAX_TOMBSTONES`
//! retired slots, oldest claims first out. A row refreshed by every write is
//! invisible to the retention sweep, so this cap — not the sweep — is what
//! keeps a file rewritten a thousand times from carrying a million pins.
//! Rows written before the cap keep their size until the file is written
//! again.
//!
//! **Garbage collection (#225).** At session start the store sweeps rows
//! whose file no longer exists on disk (always on) and, when configured,
//! rows untouched for longer than the retention window. A retired record
//! ages out on that sweep: the retire refreshes its row, and a file nobody
//! writes to any more stops refreshing it. Best-effort: a sweep error is
//! logged and the session proceeds.
//!
//! **Placement:** one database file per workspace (`{workspace}/.tau/
//! anchors.sqlite`), the workspace scope the session store uses
//! (`session.rs`, spec §3). One synchronous connection for the store's
//! lifetime; a row is a file's whole pin record (`pins` a JSON blob column).

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

mod record;
pub use record::{FilePins, MAX_TOMBSTONES, Pin, file_checksum, next_pins, reanchor};

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
    ///
    /// The bound lives here, on the way to disk: the retired slots past
    /// `MAX_TOMBSTONES` are dropped as the record is saved, so no caller can
    /// persist an unbounded record (#310).
    pub fn save(&self, pins: &FilePins) -> Result<(), StoreError> {
        let guard = self.open_conn()?;
        let conn = guard.as_ref().expect("open_conn leaves the connection set");
        let mut durable = pins.clone();
        durable.cap_retired();
        let pins_json =
            serde_json::to_string(&durable.pins).expect("Vec<Pin> is always serializable");
        conn.execute(
            "INSERT INTO anchors (path, checksum, line_count, pins, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                 checksum = excluded.checksum,
                 line_count = excluded.line_count,
                 pins = excluded.pins,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                durable.path,
                durable.checksum,
                i64::try_from(durable.line_count).unwrap_or(i64::MAX),
                pins_json,
                now_ms()
            ],
        )
        .map(|_| ())
        .map_err(|e| StoreError(e.to_string()))
    }

    /// Retire `path`'s pins: a `write` voids every anchor, so each slot
    /// becomes a tombstone. The row stays — the record is what keeps those
    /// anchors claimed (#207 §2) — and it stays bounded: `save` caps the
    /// graveyard at `MAX_TOMBSTONES` claims, oldest claims first out (#310).
    pub fn retire(&self, path: &str) {
        let Some(mut pins) = self.load(path) else {
            return;
        };
        for p in &mut pins.pins {
            p.tombstone = true;
        }
        // Best-effort, like every persist here: a save that does not land
        // leaves the durable claim unrecorded, and the seeding session's
        // `freed` set is what keeps the mint off these anchors meanwhile.
        let _ = self.save(&pins);
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

#[cfg(test)]
mod testkit;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
#[cfg(test)]
#[path = "tests_gc.rs"]
mod tests_gc;
