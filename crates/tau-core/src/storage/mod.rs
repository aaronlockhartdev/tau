//! Session storage (`spec/session/storage`). Core owns the store: one write
//! connection per open store in a process (R-session-01), the `events` log, and the
//! mutable state tables, committed in one transaction (R-session-02).
//!
//! Duties are separated by schema: `events` rows are appended and never updated
//! (R-session-31), a log row names a state row while foreign keys are on (R-session-32),
//! and a turn closed by its committed marker takes no further event (R-session-30). The mutable
//! rows — the turn state and its marker — live in `turn`. A tool call commits its
//! events and its state change together, so no fact has two owners. Times are core's:
//! an event carries its event time and the store records that value (R-session-28),
//! and a session's creation time is stamped on the way in (R-session-29).

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

/// A busy store is waited for, not failed on: SQLite keeps one writer at a time.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const EVENT_INSERT: &str =
    "INSERT INTO events (session, turn, ts, kind, payload) VALUES (?1, ?2, ?3, ?4, ?5)";
const EVENTS_SELECT: &str =
    "SELECT ts, kind, payload FROM events WHERE session = ?1 AND turn = ?2 ORDER BY seq";
const TURN_SELECT: &str = "SELECT state FROM turn WHERE session = ?1 AND turn = ?2";
const SESSION_SELECT: &str = "SELECT created FROM sessions WHERE id = ?1";
const PARENT_SELECT: &str = "SELECT parent FROM sessions WHERE id = ?1";
const VISIBLE_SELECT: &str = "SELECT visible_from_turn FROM sessions WHERE id = ?1";
/// A turn's state row, inserted before its events (the composite foreign key wants it
/// first) and re-written when the turn is re-opened.
const TURN_UPSERT: &str = "INSERT INTO turn (session, turn, state) VALUES (?1, ?2, ?3)
             ON CONFLICT (session, turn) DO UPDATE SET state = excluded.state";
/// A search hit: a stored event, located — `(session, turn, ts, kind, payload)`.
pub type Hit = (String, i64, i64, String, String);
/// The index answers, joined back to the log, so a hit is a row a caller can act on and
/// not an index position.
const SEARCH_SELECT: &str = "SELECT e.session, e.turn, e.ts, e.kind, e.payload \
     FROM events_fts JOIN events e ON e.seq = events_fts.rowid \
     WHERE events_fts MATCH ?1 ORDER BY e.seq";
const EXPORT_SELECT: &str =
    "SELECT turn, seq, ts, kind, payload FROM events WHERE session = ?1 ORDER BY seq";
/// A run reads the state table and joins the log to it, so a turn with no events still
/// answers, and no row is invented for a turn that has none.
const RUN_SELECT: &str = "SELECT t.turn, t.state, t.committed, e.ts, e.kind, e.payload
     FROM turn t LEFT JOIN events e ON e.session = t.session AND e.turn = t.turn
     WHERE t.session = ?1 ORDER BY t.turn, e.seq";
/// A compaction appends this kind, and its turn's state says the same.
const SUMMARY_KIND: &str = "summary";
const COMPACTED_STATE: &str = "compacted";
/// One tool call's worth of storage: the turn's new state and the events it
/// produced, as `(kind, payload, ts)` triples in the order they are appended.
/// `ts` is the event time (glossary): the caller stamps it, the store records it.
#[derive(Debug)]
pub struct Turn<'a> {
    pub session: &'a str,
    pub turn: i64,
    pub state: &'a str,
    pub events: &'a [(&'a str, &'a str, i64)],
}

/// One turn of a run, as the store answers it: the state the turn left, whether it
/// committed, and the events it produced as `(ts, kind, payload)` in log order.
#[derive(Debug, PartialEq)]
pub struct RunTurn {
    pub turn: i64,
    pub state: String,
    pub committed: bool,
    pub events: Vec<(i64, String, String)>,
}

/// Why a store write did not happen.
#[derive(Debug)]
pub enum Error {
    /// The store is already open for writing, so a second write connection was
    /// refused (R-session-01).
    AlreadyOpen(PathBuf),
    /// A store panicked while holding the open-store claim.
    Claimed,
    /// The store settled on a journal mode that is not WAL.
    NotWal(String),
    /// `synchronous` did not come back as NORMAL.
    NotNormal(i64),
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyOpen(path) => write!(f, "{} is already open for writing", path.display()),
            Self::Claimed => f.write_str("the open-store claim is poisoned"),
            Self::NotWal(mode) => write!(f, "journal mode is {mode}, expected wal"),
            Self::NotNormal(sync) => write!(f, "synchronous is {sync}, expected 1 (NORMAL)"),
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
             created INTEGER NOT NULL,
             visible_from_turn INTEGER
         );
         CREATE TABLE turn (
             session TEXT NOT NULL REFERENCES sessions(id),
             turn INTEGER NOT NULL,
             state TEXT NOT NULL,
             committed INTEGER NOT NULL DEFAULT 0 CHECK (committed IN (0, 1)),
             PRIMARY KEY (session, turn)
         );
         CREATE TABLE events (
             seq INTEGER PRIMARY KEY,
             session TEXT NOT NULL,
             turn INTEGER NOT NULL,
             ts INTEGER NOT NULL,
             kind TEXT NOT NULL,
             payload TEXT NOT NULL,
             FOREIGN KEY (session, turn) REFERENCES turn (session, turn)
         );
         CREATE INDEX events_turn ON events (session, turn, seq);
         CREATE VIRTUAL TABLE events_fts USING fts5(kind, payload, content='events',
             content_rowid='seq');
         CREATE TRIGGER events_fts_insert AFTER INSERT ON events BEGIN
             INSERT INTO events_fts (rowid, kind, payload)
             VALUES (NEW.seq, NEW.kind, NEW.payload);
         END;
         CREATE TRIGGER events_no_late_insert
             BEFORE INSERT ON events
             WHEN EXISTS (SELECT 1 FROM turn WHERE turn.session = NEW.session
                 AND turn.turn = NEW.turn AND turn.committed = 1)
             BEGIN
                 SELECT RAISE(ABORT, 'a committed turn accepts no further event');
             END;
         CREATE TRIGGER events_no_update
             BEFORE UPDATE ON events
             BEGIN
                 SELECT RAISE(ABORT, 'the log is append-only');
             END;
         CREATE TRIGGER events_no_delete
             BEFORE DELETE ON events
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
    /// back, because a declined PRAGMA answers instead of erroring. `foreign_keys = ON` is set, not
    /// read back. It is set outside a transaction, where SQLite honours it — inside one the
    /// PRAGMA is a no-op — and nothing observes the store's own setting: the foreign-key
    /// scenario runs on a connection of its own, which sets the PRAGMA for itself.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        let key = path.canonicalize()?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // A declined PRAGMA is not an error: journal_mode answers with the mode it
        // settled on, so the answer is checked. WAL is the design — readers,
        // checkpoint — and a store that stayed in delete mode would not be it.
        let mode = conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        })?;
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
        migrations().to_latest(&mut conn)?;
        // The claim is taken last, so a store that failed to configure itself leaves
        // the path open for the next attempt.
        let mut held = claims().lock().map_err(|_| Error::Claimed)?;
        if !held.insert(key.clone()) {
            return Err(Error::AlreadyOpen(key));
        }
        Ok(Self { conn, path: key })
    }

    /// Record a session in the store, so a turn's rows have something to belong to,
    /// and stamp its creation time (R-session-29): core supplies the value, the
    /// schema has no default.
    pub fn open_session(&self, id: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO sessions (id, created) VALUES (?1, ?2)",
            rusqlite::params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Write a turn's state change and its events in one transaction (R-session-02).
    /// A failure leaves no event and no state change.
    pub fn commit(&mut self, turn: &Turn<'_>) -> Result<(), Error> {
        self.commit_marked(turn, 1)
    }

    /// The transaction's statements, in the order the schema requires. The state row
    /// goes first, because the log's foreign key points at it (R-session-32). The
    /// marker goes last, so a turn that reads as committed is the proof its events are
    /// there (R-session-07).
    /// shortcut: the log has no dedup key, so writing a committed turn's events again is
    /// refused (R-session-30), not deduplicated; idempotent replay arrives with
    /// R-session-06's restart scenario.
    fn commit_marked(&mut self, turn: &Turn<'_>, marker: i64) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            TURN_UPSERT,
            rusqlite::params![turn.session, turn.turn, turn.state],
        )?;
        {
            let mut stmt = tx.prepare(EVENT_INSERT)?;
            for (kind, payload, ts) in turn.events {
                stmt.execute(rusqlite::params![
                    turn.session,
                    turn.turn,
                    ts,
                    kind,
                    payload
                ])?;
            }
        }
        tx.execute(
            "UPDATE turn SET committed = ?3 WHERE session = ?1 AND turn = ?2",
            rusqlite::params![turn.session, turn.turn, marker],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The events of one turn, in the order they were appended, as `(ts, kind, payload)`.
    pub fn events(&self, session: &str, turn: i64) -> Result<Vec<(i64, String, String)>, Error> {
        let mut stmt = self.conn.prepare(EVENTS_SELECT)?;
        let rows = stmt.query_map(rusqlite::params![session, turn], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// The stored state of one turn, if the turn is stored at all.
    pub fn turn_state(&self, session: &str, turn: i64) -> Result<Option<String>, Error> {
        let mut stmt = self.conn.prepare(TURN_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session, turn])?;
        Ok(rows
            .next()?
            .map(|row| row.get::<_, String>(0))
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

    /// Copy an inclusive turn range into a new session, recording the parent
    /// (R-session-08). The copied turn rows arrive uncommitted on purpose: a committed
    /// turn refuses the events a fork copies, so a copy that carried the marker would
    /// be refused by its own log.
    pub fn fork(
        &mut self,
        parent: &str,
        child: &str,
        from_turn: i64,
        to_turn: i64,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sessions (id, parent, created) VALUES (?1, ?2, ?3)",
            rusqlite::params![child, parent, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO turn (session, turn, state)
             SELECT ?1, turn, state FROM turn
             WHERE session = ?2 AND turn BETWEEN ?3 AND ?4 ORDER BY turn",
            rusqlite::params![child, parent, from_turn, to_turn],
        )?;
        tx.execute(
            "INSERT INTO events (session, turn, ts, kind, payload)
             SELECT ?1, turn, ts, kind, payload FROM events
             WHERE session = ?2 AND turn BETWEEN ?3 AND ?4 ORDER BY seq",
            rusqlite::params![child, parent, from_turn, to_turn],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Append a summary event for a turn and mark the session visible from that turn
    /// (R-session-09). The log keeps every event: the mark is where a transcript
    /// starts, not what is dropped. The marker rides the same transaction as the
    /// summary, as any other turn's does (R-session-07). The turn must not be committed:
    /// the log refuses an event for a committed turn, so the whole compaction aborts. A
    /// turn row that exists uncommitted has its state rewritten and is closed.
    pub fn compact(&mut self, session: &str, turn: i64, summary: &str) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            TURN_UPSERT,
            rusqlite::params![session, turn, COMPACTED_STATE],
        )?;
        tx.execute(
            EVENT_INSERT,
            rusqlite::params![session, turn, now_ms(), SUMMARY_KIND, summary],
        )?;
        tx.execute(
            "UPDATE turn SET committed = 1 WHERE session = ?1 AND turn = ?2",
            rusqlite::params![session, turn],
        )?;
        tx.execute(
            "UPDATE sessions SET visible_from_turn = ?2 WHERE id = ?1",
            rusqlite::params![session, turn],
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

    /// The turn a compacted session is visible from, if it was compacted.
    pub fn visible_from(&self, session: &str) -> Result<Option<i64>, Error> {
        let mut stmt = self.conn.prepare(VISIBLE_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        Ok(rows
            .next()?
            .map(|row| row.get::<_, Option<i64>>(0))
            .transpose()?
            .flatten())
    }

    /// Search the stored events, answered by the FTS5 index (R-session-10). The query is
    /// FTS5 syntax, so a caller can name a column (`kind:"tool_call"`) or a phrase; a
    /// query with no match answers empty. Text typed by a user is not a query: `"`, `:`,
    /// `-` and parens are syntax, so the caller quotes it before it arrives here. Every
    /// stored event is searched, including events a compaction hid from the transcript
    /// (R-session-09 marks where a transcript starts, not what is dropped).
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

    /// One JSON line per stored event, in log order (R-session-11). The payload goes out
    /// as the text the store holds, so the export carries the store's rows and adds
    /// nothing to them. Writing an export reads the store and never writes it. An id that
    /// is not stored writes nothing and answers `Ok`. A line's keys arrive in the order
    /// `serde_json`'s map gives them — alphabetical — not the order written here.
    pub fn export_jsonl(&self, session: &str, w: &mut impl Write) -> Result<(), Error> {
        let mut stmt = self.conn.prepare(EXPORT_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        while let Some(row) = rows.next()? {
            let line = serde_json::json!({
                "session": session,
                "turn": row.get::<_, i64>(0)?,
                "seq": row.get::<_, i64>(1)?,
                "ts": row.get::<_, i64>(2)?,
                "kind": row.get::<_, String>(3)?,
                "payload": row.get::<_, String>(4)?,
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

    /// A run, as the store holds it (R-session-13): every turn of a session with its
    /// state, its committed marker, and the events it produced, in log order. A turn
    /// that produced no events arrives with an empty list, and an id that is not stored
    /// answers empty. One query over the two tables, so a reader parses no format. No
    /// range and no `visible_from_turn`: every turn and every payload arrives
    /// materialised, which a caller reading a window of a long session will bound.
    /// shortcut: the read stays open while the lines are written, so an export to a
    /// slow writer holds a read transaction for as long as it takes; bound it by session
    /// range when the CLI opens the file.
    pub fn run(&self, session: &str) -> Result<Vec<RunTurn>, Error> {
        let mut stmt = self.conn.prepare(RUN_SELECT)?;
        let mut rows = stmt.query(rusqlite::params![session])?;
        let mut run: Vec<RunTurn> = Vec::new();
        while let Some(row) = rows.next()? {
            let turn: i64 = row.get(0)?;
            // The log rows of one turn arrive in `seq` order after the state row, so a
            // turn opens when its number arrives and closes when the next one does.
            if run.last().is_none_or(|last| last.turn != turn) {
                run.push(RunTurn {
                    turn,
                    state: row.get(1)?,
                    committed: row.get::<_, i64>(2)? == 1,
                    events: Vec::new(),
                });
            }
            // A turn with no log rows arrives with NULL on the joined side.
            if let (Some(kind), Some(payload)) = (
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ) && let Some(last) = run.last_mut()
            {
                last.events.push((row.get::<_, i64>(3)?, kind, payload));
            }
        }
        Ok(run)
    }
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

    /// R-session-02's teeth. The marker write is the transaction's last statement, and
    /// `committed IN (0, 1)` is a real constraint, so a bad marker fails after the
    /// events land. In a transaction nothing is stored. The same statements outside one
    /// leave an event row and a state row, which is the second half: the mutation
    /// check, kept as a test.
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
        let turn = Turn {
            session: "s",
            turn: 0,
            state: "completed",
            events: &[("a", "1", 11), ("b", "2", 12)],
        };
        assert!(store.commit_marked(&turn, 2).is_err());
        assert_eq!(store.turn_state("s", 0).unwrap(), None);
        assert!(store.events("s", 0).unwrap().is_empty());

        let conn = &store.conn;
        conn.execute(
            "INSERT INTO turn (session, turn, state) VALUES ('s', 1, 'completed')",
            [],
        )
        .unwrap();
        conn.execute(EVENT_INSERT, rusqlite::params!["s", 1, 11, "a", "1"])
            .unwrap();
        assert!(
            conn.execute(
                "UPDATE turn SET committed = 2 WHERE session = 's' AND turn = 1",
                []
            )
            .is_err()
        );
        let left: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE session = 's' AND turn = 1",
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
}
