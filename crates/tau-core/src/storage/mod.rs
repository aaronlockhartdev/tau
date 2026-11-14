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
             created INTEGER NOT NULL
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
    /// with `synchronous = NORMAL`. Both settings are read back: a PRAGMA that is
    /// declined is not an error.
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
            "INSERT INTO turn (session, turn, state) VALUES (?1, ?2, ?3)
             ON CONFLICT (session, turn) DO UPDATE SET state = excluded.state",
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
