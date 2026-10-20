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
mod tests {
    use super::*;
    use rusqlite::params;

    fn store(dir: &Path, policy: GcPolicy) -> PinStore {
        PinStore::new(dir, policy)
    }

    fn gc(policy: GcPolicy) -> GcPolicy {
        policy
    }

    fn no_gc() -> GcPolicy {
        GcPolicy {
            retention_days: 0,
            gc_on_start: false,
        }
    }

    fn pins(path: &str, anchors: &[&str], sums: &[&str]) -> FilePins {
        FilePins {
            path: path.into(),
            checksum: file_checksum(&sums.iter().map(ToString::to_string).collect::<Vec<_>>()),
            line_count: sums.len(),
            pins: anchors
                .iter()
                .zip(sums.iter())
                .map(|(a, s)| Pin {
                    anchor: a.to_string(),
                    checksum: s.to_string(),
                    tombstone: false,
                })
                .collect(),
            crc: None,
        }
    }

    fn ws(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pin-store-{name}-{}", std::process::id()));
        p
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = ws("rt");
        cleanup(&dir);
        let s = store(&dir, no_gc());
        let fp = pins("a.rs", &["ABCD", "EFGH"], &["c1", "c2"]);
        s.save(&fp).expect("save");
        let loaded = s.load("a.rs").expect("load");
        assert_eq!(loaded.path, "a.rs");
        assert_eq!(loaded.line_count, 2);
        assert_eq!(loaded.pins, fp.pins);
        assert_eq!(loaded.checksum, fp.checksum);
        // A fresh store on the same file sees the persisted row.
        let again = store(&dir, no_gc());
        assert_eq!(again.load("a.rs").expect("load"), loaded);
        cleanup(&dir);
    }

    #[test]
    fn load_missing_is_none_not_err() {
        let dir = ws("missing");
        cleanup(&dir);
        let s = store(&dir, no_gc());
        assert!(s.load("never-seen.rs").is_none());
        cleanup(&dir);
    }

    #[test]
    fn remove_drops_the_record() {
        let dir = ws("rm");
        cleanup(&dir);
        let s = store(&dir, no_gc());
        s.save(&pins("gone.rs", &["ABCD"], &["c1"])).expect("save");
        s.remove("gone.rs");
        assert!(s.load("gone.rs").is_none());
        assert!(s.paths().is_empty());
        cleanup(&dir);
    }

    #[test]
    fn file_path_is_the_workspace_sqlite_file() {
        let dir = ws("fp");
        cleanup(&dir);
        let s = store(&dir, no_gc());
        assert_eq!(
            s.file_path("anything.rs"),
            dir.join("anchors.sqlite"),
            "the db file is workspace-scoped, not per-path"
        );
        cleanup(&dir);
    }

    #[test]
    fn gc_missing_file_sweep_prunes_rows_for_absent_files() {
        let dir = ws("gc-missing");
        cleanup(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        // Both files exist while their rows are written.
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        fs::write(&a, "one\n").expect("write a");
        fs::write(&b, "two\n").expect("write b");
        let s = store(&dir, no_gc());
        s.save(&pins(a.to_str().expect("utf8"), &["AAAA"], &["c1"]))
            .expect("save a");
        s.save(&pins(b.to_str().expect("utf8"), &["BBBB"], &["c2"]))
            .expect("save b");
        // b.rs is deleted from disk between sessions; the session-start sweep
        // prunes its row. a.rs exists and survives.
        fs::remove_file(&b).expect("rm b");
        drop(s);
        let s = store(
            &dir,
            gc(GcPolicy {
                retention_days: 0,
                gc_on_start: true,
            }),
        );
        let paths = s.paths();
        assert_eq!(paths, vec![a.to_str().expect("utf8").to_owned()]);
        assert!(
            s.load(a.to_str().expect("utf8")).is_some(),
            "existing file keeps its pins"
        );
        cleanup(&dir);
    }

    #[test]
    fn gc_disabled_does_not_prune() {
        let dir = ws("gc-off");
        cleanup(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let b = dir.join("b.rs");
        fs::write(&b, "two\n").expect("write b");
        let s = store(&dir, no_gc());
        s.save(&pins(b.to_str().expect("utf8"), &["BBBB"], &["c2"]))
            .expect("save");
        fs::remove_file(&b).expect("rm b");
        drop(s);
        // gc_on_start = false: the sweep does not run, the row stays.
        let s = store(&dir, no_gc());
        assert_eq!(s.paths(), vec![b.to_str().expect("utf8").to_owned()]);
        cleanup(&dir);
    }

    #[test]
    fn gc_age_expiry_drops_stale_records() {
        let dir = ws("gc-age");
        cleanup(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let a = dir.join("a.rs");
        fs::write(&a, "one\n").expect("write a");
        let s = store(&dir, no_gc());
        s.save(&pins(a.to_str().expect("utf8"), &["AAAA"], &["c1"]))
            .expect("save");
        drop(s);
        // Backdate the row three days; a 1-day retention must drop it.
        let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
        db.execute(
            "UPDATE anchors SET updated_at = ?1",
            params![now_ms() - 3 * 24 * 60 * 60 * 1000],
        )
        .expect("backdate");
        drop(db);
        let s = store(
            &dir,
            gc(GcPolicy {
                retention_days: 1,
                gc_on_start: true,
            }),
        );
        assert!(
            s.paths().is_empty(),
            "a record older than retention_days is pruned"
        );
        cleanup(&dir);
    }

    #[test]
    fn gc_age_expiry_disabled_at_zero() {
        let dir = ws("gc-age-off");
        cleanup(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let a = dir.join("a.rs");
        fs::write(&a, "one\n").expect("write a");
        let s = store(&dir, no_gc());
        s.save(&pins(a.to_str().expect("utf8"), &["AAAA"], &["c1"]))
            .expect("save");
        drop(s);
        let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
        db.execute(
            "UPDATE anchors SET updated_at = ?1",
            params![now_ms() - 300 * 24 * 60 * 60 * 1000],
        )
        .expect("backdate");
        drop(db);
        // retention_days = 0: the age sweep is off (the missing-file sweep
        // still runs, but a.rs exists, so nothing is pruned).
        let s = store(
            &dir,
            gc(GcPolicy {
                retention_days: 0,
                gc_on_start: true,
            }),
        );
        assert_eq!(s.paths(), vec![a.to_str().expect("utf8").to_owned()]);
        cleanup(&dir);
    }

    #[test]
    fn version_mismatch_nukes_the_anchors_table() {
        let dir = ws("ver");
        cleanup(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let s = store(&dir, no_gc());
        s.save(&pins("a.rs", &["AAAA"], &["c1"])).expect("save");
        drop(s);
        // Simulate a future schema bump landing in this file.
        let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
        db.execute(
            "UPDATE meta SET value = '2' WHERE key = 'version'",
            params![],
        )
        .expect("bump version");
        drop(db);
        let s = store(&dir, no_gc());
        assert!(
            s.paths().is_empty(),
            "a version mismatch drops every row (the store is a cache: re-mint)"
        );
        // And the store is still usable after the nuke.
        s.save(&pins("b.rs", &["BBBB"], &["c2"]))
            .expect("save after nuke");
        assert_eq!(s.paths(), vec!["b.rs".to_owned()]);
        cleanup(&dir);
    }

    #[test]
    fn corrupt_db_is_quarantined_and_rebuilt() {
        let dir = ws("quar");
        cleanup(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let s = store(&dir, no_gc());
        s.save(&pins("a.rs", &["AAAA"], &["c1"])).expect("save");
        drop(s);
        // Overwrite the db file with garbage so quick_check fails.
        fs::write(
            dir.join("anchors.sqlite"),
            b"this is not a sqlite database at all\n",
        )
        .expect("corrupt");
        let s = store(&dir, no_gc());
        // The corrupt file is quarantined (renamed, original gone) and a
        // fresh db is in its place.
        let quarantined: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("anchors.sqlite.corrupt"))
            })
            .collect();
        assert_eq!(quarantined.len(), 1, "the corrupt db was renamed aside");
        assert!(!dir.join("anchors.sqlite").exists() || quarantined.len() == 1);
        // The store is usable again after the rebuild.
        s.save(&pins("b.rs", &["BBBB"], &["c2"]))
            .expect("save after quarantine");
        assert_eq!(s.paths(), vec!["b.rs".to_owned()]);
        cleanup(&dir);
    }

    #[test]
    fn reanchor_matches_by_checksum_in_order() {
        let prev = pins("a", &["A", "B", "C"], &["s1", "s2", "s3"]);
        // New file: s4 at the front, then s1 and s2 in their original
        // relative order (s3 deleted). Re-anchoring is an in-order LCS: s1
        // and s2 keep their pins, s4 is fresh, s3's slot is tombstoned
        // downstream.
        let new = vec!["s4".to_owned(), "s1".to_owned(), "s2".to_owned()];
        let (matched, fresh) = reanchor(&prev, &new);
        assert!(fresh[0], "s4 is new");
        assert!(matched[1], "s1 keeps its pin");
        assert!(matched[2], "s2 keeps its pin");
        assert_eq!(fresh.iter().filter(|b| **b).count(), 1);
    }

    #[test]
    fn reanchor_reverse_order_is_not_matched() {
        // A three-way reversal: any *pair* of the survivors is in the
        // opposite relative order, so the in-order LCS matches only one of
        // the three and re-mints the other two. (A two-element reversal has
        // LCS 1 — a single element is trivially in-order — so the negative
        // needs three.)
        let prev = pins("a", &["A", "B", "C"], &["s1", "s2", "s3"]);
        let new = vec!["s3".to_owned(), "s2".to_owned(), "s1".to_owned()];
        let (matched, fresh) = reanchor(&prev, &new);
        assert_eq!(
            matched.iter().filter(|b| **b).count(),
            1,
            "a full reversal matches only one survivor"
        );
        assert_eq!(fresh.iter().filter(|b| **b).count(), 2);
    }

    #[test]
    fn next_pins_tombstones_deleted_and_keeps_survivors() {
        let prev = pins("a", &["A", "B"], &["s1", "s2"]);
        // Delete s2 (anchor B), keep s1, add s3.
        let new = vec!["s1".to_owned(), "s3".to_owned()];
        let (matched, _fresh) = reanchor(&prev, &new);
        let minted = vec!["A".to_owned(), "ZZZZ".to_owned()];
        let next = next_pins(&prev, &matched, &minted, &new);
        // s1 survives with anchor A; s3 is minted ZZZZ; s2's pin B is
        // tombstoned (present, flagged), not recycled.
        let live: Vec<&Pin> = next.pins.iter().filter(|p| !p.tombstone).collect();
        assert_eq!(live.len(), 2);
        assert!(live.iter().any(|p| p.anchor == "A" && p.checksum == "s1"));
        assert!(
            live.iter()
                .any(|p| p.anchor == "ZZZZ" && p.checksum == "s3")
        );
        let tomb: Vec<&Pin> = next.pins.iter().filter(|p| p.tombstone).collect();
        assert_eq!(tomb.len(), 1);
        assert_eq!(tomb[0].anchor, "B", "the deleted line's slot is tombstoned");
    }

    #[test]
    fn file_checksum_is_content_and_order_sensitive() {
        let a = file_checksum(&["x".into(), "y".into()]);
        let same = file_checksum(&["x".into(), "y".into()]);
        let reordered = file_checksum(&["y".into(), "x".into()]);
        let changed = file_checksum(&["x".into(), "z".into()]);
        assert_eq!(a, same);
        assert_ne!(a, reordered);
        assert_ne!(a, changed);
    }
}
