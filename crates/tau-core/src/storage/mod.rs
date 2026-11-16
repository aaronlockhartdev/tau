//! Session storage (`spec/session/storage`). Core owns the store: one write
//! connection per open store in a process (R-session-01), the `blocks` log, and the
//! mutable state tables, a block and its state pointers committed in one transaction
//! (R-session-02).
//!
//! Duties are separated by schema: `blocks` rows are appended and never updated
//! (R-session-31), a block names the request it came from and no request past its
//! terminal produces one (R-session-32), and a block is committed by the transaction
//! that inserts it, so the log carries no marker for a caller to close. The mutable
//! rows — the session row and the `requests` rows that carry usage and a terminal
//! reason — live in `sessions` and `requests` alone (R-session-03). A tool call commits
//! its blocks and their state pointers together, so no fact has two owners. Times are
//! core's: a block carries its event time and the store records that value (R-session-28),
//! and a session's creation time is stamped on the way in (R-session-29).

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

/// A busy store is waited for, not failed on: SQLite keeps one writer at a time.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Two setup refusals are the ones a busy handler cannot settle. The WAL upgrade opens a
/// write transaction and is refused the moment a lock stands. The migrations run in a
/// deferred transaction: SQLite rolls that transaction back when it meets a lock, and the
/// statement answers SQLITE_BUSY even after the busy handler has waited, so a long timeout
/// only spends itself. Both are therefore waited out by the retry below, this many attempts
/// apart by this backoff, with the busy timeout held at SETUP_BUSY_TIMEOUT so that one
/// attempt cannot burn the runtime's BUSY_TIMEOUT.
const SETUP_ATTEMPTS: u32 = 20;
const SETUP_BACKOFF: Duration = Duration::from_millis(50);
/// The setup's own busy timeout: short, because for these two refusals a long one buys
/// nothing, and every other cold-open lock is absorbed by the retry above.
const SETUP_BUSY_TIMEOUT: Duration = Duration::from_millis(100);

/// The refusals this process's setup waited out, as the largest count of any one of its
/// steps, read back by `Store::setup_refusals`.
static SETUP_REFUSALS: AtomicU32 = AtomicU32::new(0);

/// The refusal that means "another writer holds what this step needs".
fn locked(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(failure, _)
        if failure.code == rusqlite::ErrorCode::DatabaseBusy)
}

fn migration_locked(error: &rusqlite_migration::Error) -> bool {
    matches!(error, rusqlite_migration::Error::RusqliteError { err, .. } if locked(err))
}

/// Run a setup step, waiting out a lock refusal. Any other failure is returned as it is,
/// and so is the refusal that ends the last attempt.
fn setup<T, E>(
    mut step: impl FnMut() -> Result<T, E>,
    locked: impl Fn(&E) -> bool,
) -> Result<T, E> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match step() {
            Ok(value) => {
                SETUP_REFUSALS.fetch_max(attempt - 1, Ordering::Relaxed);
                return Ok(value);
            }
            Err(error) if locked(&error) && attempt < SETUP_ATTEMPTS => {
                std::thread::sleep(SETUP_BACKOFF);
            }
            Err(error) => return Err(error),
        }
    }
}

const BLOCK_INSERT: &str = "INSERT INTO blocks \
     (session, request, ts, kind, payload, slot, state_start, state_end) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
const BLOCKS_SELECT: &str = "SELECT seq, request, ts, kind, payload, slot, state_start, state_end \
     FROM blocks WHERE session = ?1 ORDER BY seq";
const BLOCKS_AFTER_SELECT: &str = "SELECT seq, request, ts, kind, payload, slot, state_start, state_end \
     FROM blocks WHERE session = ?1 AND seq > ?2 ORDER BY seq";
const SLOT_SELECT: &str = "SELECT seq, request, ts, kind, payload, slot, state_start, state_end \
     FROM blocks WHERE session = ?1 AND slot = ?2 ORDER BY seq";
const LAST_SEQ_SELECT: &str = "SELECT max(seq) FROM blocks WHERE session = ?1 AND request = ?2";
const REQUEST_SELECT: &str = "SELECT request, provider_id, model, usage_in, usage_cache_read, \
     usage_cache_write, usage_out, reason FROM requests WHERE session = ?1 ORDER BY request DESC LIMIT 1";
const SESSION_SELECT: &str = "SELECT created FROM sessions WHERE id = ?1";
const PARENT_SELECT: &str = "SELECT parent FROM sessions WHERE id = ?1";
/// A request row, opened with null usage and no terminal reason. Its blocks arrive later,
/// and the terminal write stamps the counts and the reason (R-session-40).
const REQUEST_INSERT: &str = "INSERT INTO requests (session, request, provider_id, model) \
     VALUES (?1, ?2, ?3, ?4)";
const REQUEST_STAMP: &str = "UPDATE requests SET usage_in = ?3, usage_cache_read = ?4, \
     usage_cache_write = ?5, usage_out = ?6, reason = ?7 \
     WHERE session = ?1 AND request = ?2 AND reason IS NULL";
/// A search hit: a stored block, located — `(session, seq, ts, kind, payload)`.
pub type Hit = (String, i64, i64, String, String);
/// The index answers, joined back to the log, so a hit is a row a caller can act on and
/// not an index position.
const SEARCH_SELECT: &str = "SELECT b.session, b.seq, b.ts, b.kind, b.payload \
     FROM blocks_fts JOIN blocks b ON b.seq = blocks_fts.rowid \
     WHERE blocks_fts MATCH ?1 ORDER BY b.seq";
const EXPORT_SELECT: &str = "SELECT seq, request, ts, kind, payload, slot, state_start, state_end \
     FROM blocks WHERE session = ?1 ORDER BY seq";

/// One block as the store records it: the request it came from, or none for user input
/// and injections (R-session-38); the context slot it fills, when it fills one
/// (R-session-45); and the state it leaves as pointers into `blocks` (R-session-37).
/// `ts` is the event time (glossary): the caller stamps it, the store records it.
#[derive(Debug)]
pub struct Block<'a> {
    pub session: &'a str,
    pub request: Option<i64>,
    pub ts: i64,
    pub kind: &'a str,
    pub payload: &'a str,
    pub slot: Option<&'a str>,
    /// The state this block leaves, as the first and last sequences of the context it was built
    /// from. Both name blocks the session has already written: the log is append-only, so a
    /// pointer to a later sequence is refused, as is half a pair (R-session-37).
    pub state_start: Option<i64>,
    pub state_end: Option<i64>,
}

/// A block as the store answers it, with the sequence the store assigned.
#[derive(Debug, PartialEq)]
pub struct StoredBlock {
    pub seq: i64,
    pub request: Option<i64>,
    pub ts: i64,
    pub kind: String,
    pub payload: String,
    pub slot: Option<String>,
    pub state_start: Option<i64>,
    pub state_end: Option<i64>,
}

/// One provider round-trip, as the store answers it: its model, its usage counts —
/// null while the request is open, and null for a count the provider never reported
/// (R-session-40) — and its terminal reason, null until the request reaches one.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub request: i64,
    pub provider_id: Option<String>,
    pub model: String,
    pub usage_in: Option<i64>,
    pub usage_cache_read: Option<i64>,
    pub usage_cache_write: Option<i64>,
    pub usage_out: Option<i64>,
    pub reason: Option<String>,
}

/// Why a store write did not happen.
#[derive(Debug)]
pub enum Error {
    /// The store is already open for writing, so a second write connection was
    /// refused (R-session-01).
    AlreadyOpen(PathBuf),
    /// A store panicked while holding the open-store claim.
    Claimed,
    /// A request's terminal is one event, and the stamp found no open request row to land
    /// on: the request was never opened, or its terminal already landed (R-session-40).
    Terminal(String),
    /// The store settled on a journal mode that is not WAL.
    NotWal(String),
    /// `synchronous` did not come back as NORMAL, so every commit would pay the fsync.
    NotNormal(i64),
    /// `foreign_keys` did not come back as ON, so the store would write unchecked rows.
    NotForeignKeys(i64),
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyOpen(path) => write!(f, "{} is already open for writing", path.display()),
            Self::Claimed => f.write_str("the open-store claim is poisoned"),
            Self::Terminal(what) => write!(f, "no open request row to stamp: {what}"),
            Self::NotWal(mode) => write!(f, "journal mode is {mode}, expected wal"),
            Self::NotNormal(sync) => write!(f, "synchronous is {sync}, expected 1 (NORMAL)"),
            Self::NotForeignKeys(fk) => write!(f, "foreign keys are {fk}, expected 1 (ON)"),
            Self::Io(e) => write!(f, "store file: {e}"),
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Migration(e) => write!(f, "migrations: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<rusqlite_migration::Error> for Error {
    fn from(e: rusqlite_migration::Error) -> Self {
        Self::Migration(e)
    }
}

/// The schema, in order. Append a migration; never edit one that shipped. A schema
/// that has not shipped is edited in place (invariant in `spec/index.md`). The crate
/// tracks `user_version` alone, with no checksum, so a store written by the old text
/// keeps its old rows: delete it.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(
        "CREATE TABLE sessions (
             id TEXT PRIMARY KEY,
             parent TEXT REFERENCES sessions(id),
             created INTEGER NOT NULL
         );
         CREATE TABLE requests (
             session TEXT NOT NULL REFERENCES sessions(id),
             request INTEGER NOT NULL,
             provider_id TEXT,
             model TEXT NOT NULL,
             usage_in INTEGER,
             usage_cache_read INTEGER,
             usage_cache_write INTEGER,
             usage_out INTEGER,
             reason TEXT,
             PRIMARY KEY (session, request)
         );
         CREATE TABLE blocks (
             seq INTEGER PRIMARY KEY,
             session TEXT NOT NULL REFERENCES sessions(id),
             request INTEGER,
             ts INTEGER NOT NULL,
             kind TEXT NOT NULL,
             payload TEXT NOT NULL,
             slot TEXT,
             state_start INTEGER,
             state_end INTEGER,
            CHECK ((state_start IS NULL) = (state_end IS NULL)
                   AND (state_start IS NULL OR state_start <= state_end)),
             FOREIGN KEY (session, request) REFERENCES requests (session, request)
         );
         CREATE INDEX blocks_session ON blocks (session, seq);
         -- A state names blocks the session has already written: the log is append-only, so a
         -- pointer to a later block is unwritable, not merely useless.
        CREATE TRIGGER blocks_state_named AFTER INSERT ON blocks
         WHEN NEW.state_start IS NOT NULL AND (
             NOT EXISTS (SELECT 1 FROM blocks b
                         WHERE b.session = NEW.session AND b.seq = NEW.state_start)
             OR NOT EXISTS (SELECT 1 FROM blocks b
                           WHERE b.session = NEW.session AND b.seq = NEW.state_end))
         BEGIN
             SELECT RAISE(ABORT, 'a state pointer names no block of the session');
         END;
         CREATE VIRTUAL TABLE blocks_fts USING fts5(kind, payload, content='blocks',
             content_rowid='seq');
         CREATE TRIGGER blocks_fts_insert AFTER INSERT ON blocks BEGIN
             INSERT INTO blocks_fts (rowid, kind, payload)
             VALUES (NEW.seq, NEW.kind, NEW.payload);
         END;
         CREATE TRIGGER blocks_no_after_terminal
             BEFORE INSERT ON blocks
             WHEN NEW.request IS NOT NULL AND EXISTS (
                 SELECT 1 FROM requests
                 WHERE session = NEW.session AND request = NEW.request AND reason IS NOT NULL)
             BEGIN
             SELECT RAISE(ABORT, 'a terminal request produces no further block');
         END;
         CREATE TRIGGER blocks_no_update
             BEFORE UPDATE ON blocks
             BEGIN
             SELECT RAISE(ABORT, 'the log is append-only');
         END;
         CREATE TRIGGER blocks_no_delete
             BEFORE DELETE ON blocks
             BEGIN
             SELECT RAISE(ABORT, 'the log is append-only');
         END;",
    )])
}

/// Core's clock: whole milliseconds since the Unix epoch, the event time's unit.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

/// The store paths open for writing in this process.
/// shortcut: one write connection is enforced per process; a second process on one
/// project opens its own connection, and SQLite serializes the two writers per
/// transaction. Upgrade when two processes write one project.
fn claims() -> &'static Mutex<HashSet<PathBuf>> {
    static CLAIMS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    CLAIMS.get_or_init(Mutex::default)
}

/// A project's store, open for writing. A `Store` is the only way to reach the
/// file from inside core: it is not `Clone` and hands out no connection, so one open
/// write connection in this process (R-session-01).
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    /// Open the store at `path`, creating it and its parent directory, in WAL mode
    /// with `synchronous = NORMAL`. The journal mode and the synchronous level are read
    /// back, because a declined PRAGMA answers instead of erroring. `foreign_keys` is read
    /// back too: it belongs to the connection, and a connection that skipped it writes
    /// unchecked rows. It is set outside a transaction, where SQLite honours it — inside
    /// one the PRAGMA is a no-op.
    /// A setup that stays locked past the retry budget is refused with SQLite's busy
    /// error: a setup absorbs a foreign writer for ~1 s (the journal upgrade, on a fresh
    /// file) to ~3 s (the migrations, on a file already in WAL, where each attempt burns the
    /// busy timeout), where the 5 s timeout restored after setup absorbs more again.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        let key = path.canonicalize()?;
        // Setup runs at its own short busy timeout: the refusals it meets are the two a
        // busy handler cannot settle, and the retry paces them (SETUP_BUSY_TIMEOUT).
        conn.busy_timeout(SETUP_BUSY_TIMEOUT)?;
        // A declined PRAGMA is not an error: journal_mode answers with the mode it
        // settled on, so the answer is checked. WAL is the design — readers,
        // checkpoint — and a store that stayed in delete mode would not be it.
        let mode = setup(
            || {
                conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
                    row.get::<_, String>(0)
                })
            },
            |error: &rusqlite::Error| locked(error),
        )?;
        if mode != "wal" {
            return Err(Error::NotWal(mode));
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // 1 is NORMAL. A keyword a quoted pragma reads as FULL is the classic silent
        // no-op, so the setting is read back rather than trusted.
        let sync = conn.query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))?;
        if sync != 1 {
            return Err(Error::NotNormal(sync));
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // The setting belongs to the connection, and one that skipped it writes unchecked
        // rows, so the store's own answer is read back like the journal mode and the
        // synchronous level.
        let fk = conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?;
        if fk != 1 {
            return Err(Error::NotForeignKeys(fk));
        }
        // The migrations run in a deferred transaction, which SQLite rolls back when it
        // meets a lock: the statement fails even after the busy handler has waited, so
        // this retry is what waits a second instance's setup out.
        setup(
            || migrations().to_latest(&mut conn),
            |error: &rusqlite_migration::Error| migration_locked(error),
        )?;
        // Setup is over: from here a busy store is waited for, not retried.
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // The claim is taken last, so a store that failed to configure itself leaves
        // the path open for the next attempt.
        let mut held = claims().lock().map_err(|_| Error::Claimed)?;
        if !held.insert(key.clone()) {
            return Err(Error::AlreadyOpen(key));
        }
        Ok(Self { conn, path: key })
    }

    /// Record a session in the store, so a block's rows have something to belong to,
    /// and stamp its creation time (R-session-29): core supplies the value, the
    /// schema has no default.
    pub fn open_session(&self, id: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO sessions (id, created) VALUES (?1, ?2)",
            rusqlite::params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Open a request: the row for one provider round-trip, with its model and an opaque
    /// provider id the store stores as given, its usage null and no terminal reason
    /// (R-session-36). The blocks it produces arrive later, naming it.
    pub fn open_request(
        &self,
        session: &str,
        request: i64,
        provider_id: Option<&str>,
        model: &str,
    ) -> Result<(), Error> {
        self.conn.execute(
            REQUEST_INSERT,
            rusqlite::params![session, request, provider_id, model],
        )?;
        Ok(())
    }

    /// Stamp a request's usage and terminal reason (R-session-40). A count the provider
    /// never reported stays null. This is a write to a mutable row, allowed because
    /// `requests` is a mutable table (R-session-03); the log is never touched.
    #[allow(clippy::too_many_arguments)]
    pub fn stamp_request(
        &self,
        session: &str,
        request: i64,
        usage_in: Option<i64>,
        usage_cache_read: Option<i64>,
        usage_cache_write: Option<i64>,
        usage_out: Option<i64>,
        reason: &str,
    ) -> Result<(), Error> {
        let stamped = self.conn.execute(
            REQUEST_STAMP,
            rusqlite::params![
                session,
                request,
                usage_in,
                usage_cache_read,
                usage_cache_write,
                usage_out,
                reason
            ],
        )?;
        // The statement carries `AND reason IS NULL`, so no row means one of two caller
        // faults: a request that was never opened, or one whose terminal already landed. A
        // terminal is one event (R-session-40), and the second write would replace counts the
        // provider reported with the caller's next argument.
        if stamped == 0 {
            return Err(Error::Terminal(format!(
                "session {session} request {request}"
            )));
        }
        Ok(())
    }

    /// Record one block, with the state pointers it carries, in one transaction
    /// (R-session-02, R-session-35). A failure leaves no block row.
    pub fn commit(&mut self, block: &Block<'_>) -> Result<(), Error> {
        self.commit_all(std::slice::from_ref(block))
    }

    /// Record the blocks one stream closed at once, in log order, in one transaction
    /// (R-session-35): a batch this store refuses leaves none of them.
    pub fn commit_all(&mut self, blocks: &[Block<'_>]) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(BLOCK_INSERT)?;
            for block in blocks {
                stmt.execute(rusqlite::params![
                    block.session,
                    block.request,
                    block.ts,
                    block.kind,
                    block.payload,
                    block.slot,
                    block.state_start,
                    block.state_end
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// A session's blocks, in the store's sequence order (R-session-13). One query over
    /// the log, so a reader parses no format. No range: every payload arrives
    /// materialised, which a caller reading a window of a long session bounds with
    /// `blocks_after`.
    pub fn run(&self, session: &str) -> Result<Vec<StoredBlock>, Error> {
        self.read_all(BLOCKS_SELECT, rusqlite::params![session])
    }

    /// The newest request of a session — the resume read's anchor (R-session-39). An id
    /// with no request answers `None`.
    pub fn latest_request(&self, session: &str) -> Result<Option<Request>, Error> {
        let mut stmt = self.conn.prepare(REQUEST_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        Ok(rows
            .next()?
            .map(|row| -> rusqlite::Result<Request> {
                Ok(Request {
                    request: row.get(0)?,
                    provider_id: row.get(1)?,
                    model: row.get(2)?,
                    usage_in: row.get(3)?,
                    usage_cache_read: row.get(4)?,
                    usage_cache_write: row.get(5)?,
                    usage_out: row.get(6)?,
                    reason: row.get(7)?,
                })
            })
            .transpose()?)
    }

    /// The sequence of the last block a request produced, for the resume read to read
    /// after (R-session-39). A request that streamed no block answers `None`.
    pub fn last_block_seq(&self, session: &str, request: i64) -> Result<Option<i64>, Error> {
        Ok(self.conn.query_row(
            LAST_SEQ_SELECT,
            rusqlite::params![session, request],
            |row| row.get::<_, Option<i64>>(0),
        )?)
    }

    /// A session's blocks after a sequence, in log order — the resume read, which never
    /// walks the blocks a request already produced (R-session-39).
    pub fn blocks_after(&self, session: &str, after: i64) -> Result<Vec<StoredBlock>, Error> {
        self.read_all(BLOCKS_AFTER_SELECT, rusqlite::params![session, after])
    }

    /// A session's blocks that name a slot, in log order (R-session-46). A slot no block
    /// fills answers empty.
    pub fn blocks_in_slot(&self, session: &str, slot: &str) -> Result<Vec<StoredBlock>, Error> {
        self.read_all(SLOT_SELECT, rusqlite::params![session, slot])
    }

    fn read_all(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<StoredBlock>, Error> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, stored_block)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// The state the newest state-carrying block of a session answers, as pointers into
    /// `blocks` (R-session-37). State is pointers, never payload text, so the context
    /// rebuilds from the log alone. A session whose blocks carry no state answers `None`.
    pub fn state(&self, session: &str) -> Result<Option<(i64, i64)>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT state_start, state_end FROM blocks \
             WHERE session = ?1 AND state_start IS NOT NULL ORDER BY seq DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        Ok(rows
            .next()?
            .map(|row| -> rusqlite::Result<(i64, i64)> {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .transpose()?)
    }

    /// The session's creation time, if the session is stored at all (R-session-29).
    pub fn created(&self, session: &str) -> Result<Option<i64>, Error> {
        let mut stmt = self.conn.prepare(SESSION_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        Ok(rows.next()?.map(|row| row.get::<_, i64>(0)).transpose()?)
    }

    /// The connection's own `synchronous` setting (R-session-05). The setting is per
    /// connection, so a reader cannot learn it from a connection of its own. It exists for
    /// the clause's readback; it is not a settings API.
    pub fn synchronous(&self) -> Result<i64, Error> {
        Ok(self
            .conn
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))?)
    }

    /// The connection's own `foreign_keys` setting (R-session-32). It is per connection,
    /// so a reader cannot learn it from a connection of its own; the store's answer is
    /// what enforces the schema's keys on the store's writes.
    pub fn foreign_keys(&self) -> Result<i64, Error> {
        Ok(self
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?)
    }

    /// The refusals this process's setup waited out (R-session-33), as the largest count
    /// of any one setup step: 0 means no other instance held the setup, so the retry never
    /// ran. It answers for the process, not for this store's path; it exists so a scenario
    /// can tell a met lock from a beat that was too short. It never resets, so a nonzero
    /// count can belong to an earlier open in the same process.
    #[must_use]
    pub fn setup_refusals(&self) -> u32 {
        SETUP_REFUSALS.load(Ordering::Relaxed)
    }

    /// Copy an inclusive block sequence range into a new session, recording the parent
    /// (R-session-08). The `requests` rows the copied blocks name are copied with them:
    /// every block names its request, so a copied range without those rows is a broken
    /// record. A copied request keeps its number, and a request that reached its terminal
    /// stays terminal in the child, so the child continues on a new request and appends
    /// without duplicating a copied block.
    pub fn fork(
        &mut self,
        parent: &str,
        child: &str,
        from_seq: i64,
        to_seq: i64,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        // The sequence the first copied block takes is the store's next one.
        let next_seq: i64 =
            tx.query_row("SELECT coalesce(max(seq), 0) + 1 FROM blocks", [], |row| {
                row.get(0)
            })?;
        tx.execute(
            "INSERT INTO sessions (id, parent, created) VALUES (?1, ?2, ?3)",
            rusqlite::params![child, parent, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO requests (session, request, provider_id, model, usage_in, usage_cache_read, \
                 usage_cache_write, usage_out) \
             SELECT ?1, request, provider_id, model, usage_in, usage_cache_read, usage_cache_write, \
                 usage_out FROM requests \
             WHERE session = ?2 AND request IN (\
                 SELECT DISTINCT request FROM blocks WHERE session = ?2 AND seq BETWEEN ?3 AND ?4)",
            rusqlite::params![child, parent, from_seq, to_seq],
        )?;
        // The copied sequences are not contiguous in the store, so a pointer is re-pointed by
        // its rank among them, not by an offset: `next_seq + rank` names the child's copy of
        // the block the pointer named. A pair is kept only when both pointers fall inside the
        // copied range, because a pointer naming an uncopied block would name the parent's row
        // from the child (R-session-37). Two shapes worth naming. The range test on `state_end`
        // is implied, not tested: a copied row's pointers name blocks at or before its own
        // sequence, so a start inside the range puts its end inside too, and no fork can carry
        // a pair that straddles the range's end. `ORDER BY seq` is load-bearing, not
        // decoration: the copies take their child sequences in insert order, and the rank is
        // only the mapping when that order is the parent's. The payload check in the gap test
        // is the read that lands on the copy of a different block if the order goes.
        tx.execute(
            "INSERT INTO blocks (session, request, ts, kind, payload, slot, state_start, state_end) \
             SELECT ?1, request, ts, kind, payload, slot, \
                 CASE WHEN state_start BETWEEN ?3 AND ?4 AND state_end BETWEEN ?3 AND ?4 THEN \
                     ?5 + (SELECT count(*) FROM blocks AS r WHERE r.session = ?2 \
                         AND r.seq BETWEEN ?3 AND ?4 AND r.seq < s.state_start) END, \
                 CASE WHEN state_start BETWEEN ?3 AND ?4 AND state_end BETWEEN ?3 AND ?4 THEN \
                     ?5 + (SELECT count(*) FROM blocks AS r WHERE r.session = ?2 \
                         AND r.seq BETWEEN ?3 AND ?4 AND r.seq < s.state_end) END \
             FROM blocks AS s WHERE s.session = ?2 AND s.seq BETWEEN ?3 AND ?4 ORDER BY seq",
            rusqlite::params![child, parent, from_seq, to_seq, next_seq],
        )?;
        // The terminal reasons are re-stamped last, as the live path stamps them: the copied
        // blocks land while their requests are open, so the trigger that refuses a block
        // after its request's terminal sees what it saw the first time.
        tx.execute(
            "UPDATE requests SET reason = (\
                 SELECT p.reason FROM requests p WHERE p.session = ?2 AND p.request = requests.request) \
             WHERE session = ?1 AND request IN (\
                 SELECT DISTINCT request FROM blocks WHERE session = ?2 AND seq BETWEEN ?3 AND ?4)",
            rusqlite::params![child, parent, from_seq, to_seq],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The session a fork copied from, if it is a fork. A root session stores no parent,
    /// and an id that is not stored answers the same way, so `None` covers both.
    pub fn parent(&self, session: &str) -> Result<Option<String>, Error> {
        let mut stmt = self.conn.prepare(PARENT_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        Ok(rows
            .next()?
            .map(|row| row.get::<_, Option<String>>(0))
            .transpose()?
            .flatten())
    }

    /// Search the stored blocks, answered by the FTS5 index (R-session-10). The query is
    /// FTS5 syntax, so a caller can name a column (`kind:"tool_use"`) or a phrase; a
    /// query with no match answers empty. Text typed by a user is not a query: `"`, `:`,
    /// `-` and parens are syntax, so the caller quotes it before it arrives here. Every
    /// stored block is searched: the index mirrors the log row for row.
    pub fn search(&self, query: &str) -> Result<Vec<Hit>, Error> {
        let mut stmt = self.conn.prepare(SEARCH_SELECT)?;
        let rows = stmt
            .query_map(rusqlite::params![query], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One JSON line per stored block, in log order (R-session-11). The payload goes out
    /// as the text the store holds, so the export carries the store's rows and adds
    /// nothing to them. Writing an export reads the store and never writes it. An id that
    /// is not stored writes nothing and answers `Ok`. A line's keys arrive in the order
    /// `serde_json`'s map gives them — alphabetical — not the order written here.
    /// shortcut: the read stays open while the lines are written, so an export to a
    /// slow writer holds a read transaction for as long as it takes; bound it by session
    /// range when the CLI opens the file.
    pub fn export_jsonl(&self, session: &str, w: &mut impl Write) -> Result<(), Error> {
        let mut stmt = self.conn.prepare(EXPORT_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        while let Some(row) = rows.next()? {
            let line = serde_json::json!({
                "session": session,
                "seq": row.get::<_, i64>(0)?,
                "request": row.get::<_, Option<i64>>(1)?,
                "ts": row.get::<_, i64>(2)?,
                "kind": row.get::<_, String>(3)?,
                "payload": row.get::<_, String>(4)?,
                "slot": row.get::<_, Option<String>>(5)?,
                "state_start": row.get::<_, Option<i64>>(6)?,
                "state_end": row.get::<_, Option<i64>>(7)?,
            });
            writeln!(w, "{line}").map_err(Error::Io)?;
        }
        Ok(())
    }

    /// Rows this connection has changed since it opened. R-session-11's acceptance reads
    /// it on either side of an export, to prove the export changed no row: the counter is
    /// the store's own, and a reader connection cannot answer for it. Not a statistics API.
    pub fn total_changes(&self) -> u64 {
        self.conn.total_changes()
    }
}

/// One row of the log, as the store answers it.
fn stored_block(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredBlock> {
    Ok(StoredBlock {
        seq: row.get(0)?,
        request: row.get(1)?,
        ts: row.get(2)?,
        kind: row.get(3)?,
        payload: row.get(4)?,
        slot: row.get(5)?,
        state_start: row.get(6)?,
        state_end: row.get(7)?,
    })
}

impl Drop for Store {
    fn drop(&mut self) {
        // The store is openable again once it is closed; a poisoned claim stays
        // closed, which is the direction that keeps one writer.
        if let Ok(mut held) = claims().lock() {
            held.remove(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<'a>(
        session: &'a str,
        request: Option<i64>,
        kind: &'a str,
        payload: &'a str,
        ts: i64,
    ) -> Block<'a> {
        Block {
            session,
            request,
            ts,
            kind,
            payload,
            slot: None,
            state_start: None,
            state_end: None,
        }
    }

    /// R-session-02's teeth. The batch's insert is one transaction, and the
    /// `blocks_no_after_terminal` trigger refuses a block naming a request past its
    /// terminal, so the refusal lands after the first row of the batch is inserted. In a
    /// transaction nothing is stored. The same statements outside one leave the first
    /// block, which is the second half: the mutation check, kept as a test.
    #[test]
    fn a_refused_write_at_the_end_of_a_commit_stores_nothing() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let dir =
            std::env::temp_dir().join(format!("tau-storage-atomic-{}-{stamp}", std::process::id()));
        let mut store = Store::open(dir.join("state.db")).unwrap();
        store.open_session("s").unwrap();
        store.open_request("s", 0, None, "m").unwrap();
        store
            .stamp_request("s", 0, None, None, None, None, "completed")
            .unwrap();

        let batch = [
            block("s", None, "user_input", "1", 11),
            block("s", Some(0), "assistant_text", "2", 12),
        ];
        assert!(store.commit_all(&batch).is_err());
        assert!(store.run("s").unwrap().is_empty());

        let conn = &store.conn;
        conn.execute(
            BLOCK_INSERT,
            rusqlite::params![
                "s",
                None::<i64>,
                11,
                "user_input",
                "1",
                None::<String>,
                None::<i64>,
                None::<i64>
            ],
        )
        .unwrap();
        assert!(
            conn.execute(
                BLOCK_INSERT,
                rusqlite::params![
                    "s",
                    0,
                    12,
                    "assistant_text",
                    "2",
                    None::<String>,
                    None::<i64>,
                    None::<i64>
                ]
            )
            .is_err(),
            "a terminal request has to refuse a block"
        );
        let left: i64 = conn
            .query_row(
                "SELECT count(*) FROM blocks WHERE session = 's'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            left, 1,
            "the probe has to leave a row when it runs without a transaction"
        );

        drop(store);
        fs::remove_dir_all(&dir).ok();
    }

    /// R-session-37 and R-session-08's teeth: a fork remaps the state pointers it copies
    /// by their rank among the copied sequences, so a child's block points inside the child.
    #[test]
    fn a_fork_moves_the_pointers_it_copies() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let dir =
            std::env::temp_dir().join(format!("tau-storage-fork-{}-{stamp}", std::process::id()));
        let mut store = Store::open(dir.join("state.db")).unwrap();
        store.open_session("p").unwrap();
        store.open_request("p", 0, None, "m").unwrap();
        store
            .commit(&block("p", None, "user_input", "u", 11))
            .unwrap();
        store
            .commit(&Block {
                session: "p",
                request: Some(0),
                ts: 12,
                kind: "assistant_text",
                payload: "a",
                slot: None,
                state_start: Some(1),
                state_end: Some(2),
            })
            .unwrap();
        store
            .stamp_request("p", 0, None, None, None, None, "completed")
            .unwrap();

        let parent_last = store.run("p").unwrap().last().unwrap().seq;
        store.fork("p", "c", 1, parent_last).unwrap();

        let copied = store.run("c").unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(copied[1].state_start, Some(copied[0].seq));
        assert_eq!(copied[1].state_end, Some(copied[1].seq));
        assert_eq!(store.latest_request("c").unwrap().unwrap().request, 0);
        assert_eq!(
            store.parent("c").unwrap().as_deref(),
            Some("p"),
            "the child records the session it copied"
        );

        drop(store);
        fs::remove_dir_all(&dir).ok();
    }

    /// R-session-37's teeth, the shape a uniform offset gets wrong: the copied sequences are
    /// not contiguous, so a pointer moves by its rank among them, and a pointer naming a
    /// block the range leaves behind is dropped with its pair.
    #[test]
    fn a_fork_moves_the_pointers_across_a_gap() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let dir = std::env::temp_dir().join(format!(
            "tau-storage-fork-gap-{}-{stamp}",
            std::process::id()
        ));
        let mut store = Store::open(dir.join("state.db")).unwrap();
        store.open_session("p").unwrap();
        store.open_session("x").unwrap();
        store
            .commit(&block("p", None, "user_input", "p1", 11))
            .unwrap();
        store
            .commit(&block("x", None, "user_input", "x1", 12))
            .unwrap();
        store
            .commit(&block("p", None, "user_input", "p2", 13))
            .unwrap();
        store
            .commit(&block("x", None, "user_input", "x2", 14))
            .unwrap();
        store
            .commit(&Block {
                session: "p",
                request: None,
                ts: 15,
                kind: "user_input",
                payload: "p3",
                slot: None,
                state_start: Some(1),
                state_end: Some(3),
            })
            .unwrap();

        // The range from p's third block leaves block 1 behind, so the pair names no row of
        // the child and the child records no state.
        store.fork("p", "c", 3, 5).unwrap();
        assert_eq!(
            store.state("c").unwrap(),
            None,
            "the pair names a block the range leaves behind"
        );

        // Over all of p the copied sequences are 1, 3 and 5, so the pair (1, 3) names the
        // child's copies of the blocks whose payloads are p1 and p2. The payload at each named
        // sequence is the proof: a mapping that assigns the child's sequences in an order other
        // than the parent's lands on the copy of a different block, and every shape-only check
        // still passes.
        store.fork("p", "d", 1, 5).unwrap();
        let copied = store.run("d").unwrap();
        assert_eq!(copied.len(), 3);
        let (start, end) = store.state("d").unwrap().unwrap();
        let named = |seq: i64| {
            copied
                .iter()
                .find(|row| row.seq == seq)
                .map(|row| row.payload.as_str())
        };
        assert_eq!(
            (named(start), named(end)),
            (Some("p1"), Some("p2")),
            "a pointer lands on the child's copy of the block it named, not on the copy at its rank"
        );

        drop(store);
        fs::remove_dir_all(&dir).ok();
    }

    /// R-session-40's scoping. A request number is per session, so a stamp naming one
    /// session's request 1 leaves another session's request 1 open: the number alone is not the
    /// key, and a stamp that treats it as one closes a request that never finished and books its
    /// counts to the wrong run.
    #[test]
    fn a_stamp_lands_on_the_session_it_names() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let dir = std::env::temp_dir().join(format!("tau-scope-{}-{stamp}", std::process::id()));
        let store = Store::open(dir.join("state.db")).unwrap();
        for session in ["a", "b"] {
            store.open_session(session).unwrap();
            store.open_request(session, 1, Some("prov"), "m").unwrap();
        }
        store
            .stamp_request("a", 1, Some(10), None, None, Some(2), "completed")
            .unwrap();
        let stamped = store.latest_request("a").unwrap().unwrap();
        assert_eq!(stamped.reason.as_deref(), Some("completed"));
        assert_eq!(stamped.usage_in, Some(10));
        let other = store.latest_request("b").unwrap().unwrap();
        assert_eq!(other.reason, None, "session b's request 1 is still open");
        assert_eq!(other.usage_in, None, "a count of a's stamp landed on b");
        drop(store);
        fs::remove_dir_all(&dir).ok();
    }

    /// R-session-37's start arm, alone. The pair is whole, runs forwards, and its end names a row
    /// the session owns, so the `CHECK` passes and the trigger's end disjunct is satisfied: only
    /// the start disjunct can refuse it. Neuter that disjunct and the block stores, which is the
    /// mutation this test is the check for. The gap comes from a sibling session taking the
    /// sequence between.
    #[test]
    fn a_state_pointer_whose_start_names_nothing_is_refused() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let dir =
            std::env::temp_dir().join(format!("tau-state-arm-{}-{stamp}", std::process::id()));
        let mut store = Store::open(dir.join("state.db")).unwrap();
        store.open_session("p").unwrap();
        store.open_session("x").unwrap();
        store
            .commit(&block("p", None, "user_input", "p1", 11))
            .unwrap();
        store
            .commit(&block("x", None, "user_input", "x1", 12))
            .unwrap();
        store
            .commit(&block("p", None, "user_input", "p2", 13))
            .unwrap();
        // p owns sequences 1 and 3; the pair (2, 3) names p's row 3 and nothing at 2.
        let err = store
            .commit(&Block {
                session: "p",
                request: None,
                ts: 14,
                kind: "user_input",
                payload: "p3",
                slot: None,
                state_start: Some(2),
                state_end: Some(3),
            })
            .unwrap_err();
        assert!(
            format!("{err}").contains("a state pointer names no block of the session"),
            "the start arm answered: {err}"
        );
        assert_eq!(
            store.run("p").unwrap().len(),
            2,
            "the refused block stored nothing"
        );
        drop(store);
        fs::remove_dir_all(&dir).ok();
    }
}
