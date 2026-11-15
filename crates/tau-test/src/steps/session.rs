//! The storage steps: the lane opens a project store, commits a tool call, and
//! reads the store back (R-session-01, 02, 28, 29, 30, 31, 32). The scan for log
//! content in the mutable tables and the checkpoint probe run on a reader connection
//! of the lane's own, `synchronous` on the store's connection, and the schema refuses
//! writes (R-session-03, 04, 05). A crash is a child process the lane kills
//! (`bin/crash_store.rs`), and the store is read back after it dies (R-session-06, 07).
//! A fork copies a range, the index answers a search, an export writes JSONL, and a run
//! is answered from the two tables (R-session-08, 10, 11, 13).
//! A second process opening a store whose setup is locked is a child process as well
//! (`bin/open_store.rs`), and the refusals it waited out are read back (R-session-33).

use crate::steps::{assert_clean, report};
use crate::world::World;
use cucumber::{given, then, when};
use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tau_core::storage::{Error, RunTurn, Store, Turn};

/// The session a scenario opens, and the turn a tool call commits. `pub` because the
/// crash child (`bin/crash_store.rs`) writes exactly these rows, so the steps assert on
/// rows the child can only have written as written.
pub const SESSION: &str = "lane-session";
pub const TURN: i64 = 1;
pub const STATE: &str = "awaiting-model";
pub const EVENTS: &[(&str, &str, i64)] = &[
    ("tool_call", r#"{"tool":"read"}"#, 1001),
    ("tool_result", "ok", 1002),
];

/// The lane's store lives under `target/tmp`, so it stays out of git and
/// `cargo clean` removes it.
fn store_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string());
    World::repo_root()
        .join("target")
        .join("tmp")
        .join(format!("store-{label}-{}-{stamp}", std::process::id()))
        .join("state.db")
}

fn no_store(world: &mut World) {
    report(world, "no store is open".to_string());
}

/// Close the store and remove the directory the scenario created under
/// `target/tmp`, so a lane run leaves no store behind.
fn clean_store(world: &mut World) {
    let _ = world.store.take();
    if let Some(dir) = world.store_path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Open the store at `path` and claim the lane's session, leaving the handle in the
/// world. A failure is reported, not panicked: the scenario's own steps report it.
pub(crate) fn open_at(world: &mut World, path: PathBuf) {
    world.store = None;
    world.store_path = path;
    match Store::open(&world.store_path) {
        Ok(store) => world.store = Some(store),
        Err(e) => {
            report(world, format!("opening the store: {e}"));
            return;
        }
    }
    // A session is open, the trigger R-session-01 speaks of.
    if let Some(store) = world.store.as_ref()
        && let Err(e) = store.open_session(SESSION)
    {
        report(world, format!("opening the session: {e}"));
    }
}

/// Commit the lane's turn — `SESSION`, `TURN`, `EVENTS` — through the open store.
pub(crate) fn commit_tool_call(world: &mut World) {
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let turn = Turn {
        session: SESSION,
        turn: TURN,
        state: STATE,
        events: EVENTS,
    };
    if let Err(e) = store.commit(&turn) {
        report(world, format!("the commit failed: {e}"));
    }
}

#[given("an open store")]
async fn open_store(world: &mut World) {
    world.findings.clear();
    open_at(world, store_path("lane"));
}

#[when("the store is opened again")]
async fn open_store_again(world: &mut World) {
    // R-session-01: core holds one write connection, so a second one is refused
    // rather than opened.
    match Store::open(&world.store_path) {
        Ok(second) => {
            drop(second);
            report(
                world,
                "a second write connection to the store opened".to_string(),
            );
        }
        // Only a refusal to claim the path counts as a refusal; any other open
        // failure is a store that never opened, which is not the clause's fact.
        Err(Error::AlreadyOpen(_)) => {}
        Err(e) => report(world, format!("opening the store again: {e}")),
    }
}

#[then("the second open is refused")]
async fn second_open_refused(world: &mut World) {
    clean_store(world);
    assert_clean(world, "the second open is refused");
}

#[when("a tool call commits its events and its state change")]
async fn commit_a_tool_call(world: &mut World) {
    commit_tool_call(world);
}

#[then("the events and the state change are stored together")]
async fn events_and_state_stored(world: &mut World) {
    // The store is read while the borrow is held, and its findings are reported
    // after it is dropped: a step cannot report into the world it reads the store from.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            match store.events(SESSION, TURN) {
                Ok(events)
                    if events
                        .iter()
                        .map(|(_ts, kind, payload)| (kind.as_str(), payload.as_str()))
                        .eq(EVENTS.iter().map(|(kind, payload, _)| (*kind, *payload))) => {}
                Ok(events) => findings.push(format!(
                    "{} of {} events are stored",
                    events.len(),
                    EVENTS.len()
                )),
                Err(e) => findings.push(format!("reading events: {e}")),
            }
            match store.turn_state(SESSION, TURN) {
                Ok(Some(state)) if state == STATE => {}
                Ok(Some(state)) => {
                    findings.push(format!("the stored state is {state}, expected {STATE}"));
                }
                Ok(None) => findings.push("the state change is not stored".to_string()),
                Err(e) => findings.push(format!("reading the state: {e}")),
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the events and the state change are stored together");
}

#[when("a tool call's commit is refused")]
async fn commit_refused(world: &mut World) {
    // The turn is already committed, so the schema refuses its next event
    // (R-session-30): the refusal lands after the state row and the log rows of
    // this transaction have been written, so the transaction is the unit that fails.
    let turn = Turn {
        session: SESSION,
        turn: TURN,
        state: "refused",
        events: EVENTS,
    };
    let committed = match world.store.as_mut() {
        None => {
            no_store(world);
            true
        }
        Some(store) => store.commit(&turn).is_ok(),
    };
    if committed {
        report(world, "a commit for a committed turn succeeded".to_string());
    }
}

#[then("the refused commit stores nothing")]
async fn nothing_stored(world: &mut World) {
    // The committed turn keeps the events and the state of the commit that
    // succeeded: the refused one appended nothing and changed nothing.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            match store.events(SESSION, TURN) {
                Ok(events) if events.len() == EVENTS.len() => {}
                Ok(events) => findings.push(format!(
                    "{} events are stored by a refused commit",
                    events.len()
                )),
                Err(e) => findings.push(format!("reading events: {e}")),
            }
            match store.turn_state(SESSION, TURN) {
                Ok(Some(state)) if state == STATE => {}
                Ok(Some(state)) => findings.push(format!(
                    "the state change of a refused commit is stored as {state}"
                )),
                Ok(None) => findings.push("the state change is not stored".to_string()),
                Err(e) => findings.push(format!("reading the state: {e}")),
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the refused commit stores nothing");
}

/// A raw connection to the lane's store, the way a reader reaches it: WAL lets it
/// write beside the store's own write connection, one writer at a time. The lane
/// needs it to write rows no `Store` method writes, and watch the schema refuse them.
fn raw_conn(world: &World) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(&world.store_path).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Runs a write the schema has to refuse, and reports it when it was accepted or
/// refused for a reason that is not the clause's.
fn refused_write(
    world: &mut World,
    label: &str,
    expect: &str,
    write: impl Fn(&rusqlite::Connection) -> rusqlite::Result<usize>,
) {
    let conn = match raw_conn(world) {
        Ok(conn) => conn,
        Err(e) => {
            report(world, format!("opening the store: {e}"));
            return;
        }
    };
    match write(&conn) {
        Ok(_) => report(world, format!("the {label} was written")),
        Err(e) if e.to_string().contains(expect) => {}
        Err(e) => report(
            world,
            format!("the {label} failed as {e}, expected {expect}"),
        ),
    }
}

#[when("an event for a turn with no state row is inserted")]
async fn insert_orphan_event(world: &mut World) {
    refused_write(world, "event with no state row", "FOREIGN KEY", |conn| {
        conn.execute(
            "INSERT INTO events (session, turn, ts, kind, payload) VALUES (?1, 7, 1003, 'tool_call', 'orphan')",
            rusqlite::params![SESSION],
        )
    });
}

#[when("a late event for the committed turn is inserted")]
async fn insert_late_event(world: &mut World) {
    refused_write(
        world,
        "event for a committed turn",
        "a committed turn accepts no further event",
        |conn| {
            conn.execute(
                "INSERT INTO events (session, turn, ts, kind, payload) VALUES (?1, ?2, 1004, 'tool_result', 'late')",
                rusqlite::params![SESSION, TURN],
            )
        },
    );
}

#[when("a stored event row is updated")]
async fn update_event_row(world: &mut World) {
    refused_write(
        world,
        "update of a log row",
        "the log is append-only",
        |conn| {
            conn.execute(
                "UPDATE events SET payload = 'changed' WHERE session = ?1",
                rusqlite::params![SESSION],
            )
        },
    );
}

#[when("a stored event row is deleted")]
async fn delete_event_row(world: &mut World) {
    refused_write(
        world,
        "delete of a log row",
        "the log is append-only",
        |conn| {
            conn.execute(
                "DELETE FROM events WHERE session = ?1",
                rusqlite::params![SESSION],
            )
        },
    );
}

#[then("the store refuses the write")]
async fn store_refuses(world: &mut World) {
    clean_store(world);
    assert_clean(world, "the store refuses the write");
}

#[then("the stored events carry the times they were given")]
async fn stored_events_carry_times(world: &mut World) {
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.events(SESSION, TURN) {
            Ok(events)
                if events
                    .iter()
                    .map(|(ts, _, _)| *ts)
                    .eq(EVENTS.iter().map(|(_, _, ts)| *ts)) => {}
            Ok(events) => findings.push(format!(
                "the stored event times are {:?}, expected {:?}",
                events.iter().map(|(ts, _, _)| *ts).collect::<Vec<_>>(),
                EVENTS.iter().map(|(_, _, ts)| *ts).collect::<Vec<_>>()
            )),
            Err(e) => findings.push(format!("reading events: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
    assert_clean(world, "the stored events carry the times they were given");
}

#[then("the session records a creation time")]
async fn session_records_creation(world: &mut World) {
    let mut findings: Vec<String> = Vec::new();
    // R-session-29 says Unix milliseconds UTC, so the value is measured as a
    // millisecond stamp near now: a seconds stamp (1000x smaller) and a microseconds
    // stamp (1000x larger) both fail here, as the clause says they should.
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().try_into().unwrap_or(i64::MAX))
        .unwrap_or(0);
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.created(SESSION) {
            Ok(Some(ts)) if (now_ms - 60_000..=now_ms + 60_000).contains(&ts) => {}
            Ok(Some(ts)) => findings.push(format!(
                "the session's creation time {ts} is not a millisecond stamp near {now_ms}"
            )),
            Ok(None) => findings.push("the session records no creation time".to_string()),
            Err(e) => findings.push(format!("reading the creation time: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the session records a creation time");
}

/// The crash child, a bin of this package: cargo names its binary to the test process.
fn crash_bin() -> Result<PathBuf, String> {
    std::env::var("CARGO_BIN_EXE_crash_store")
        .map(PathBuf::from)
        .map_err(|_| "the crash child's binary is not set".to_string())
}

/// The crash steps run two processes on one store path. `claims()` is per process, so
/// this is not the second write connection R-session-01 refuses: SQLite serializes two
/// processes per transaction, and the parent only reads. The rows the child writes are
/// this module's constants, which `bin/crash_store.rs` imports, so the lane asserts on
/// rows the child can only have written as written.
fn kill_after_report(world: &mut World, mode: &str, token: &str) {
    world.store_path = store_path("crash");
    world.store = match Store::open(&world.store_path) {
        Ok(store) => Some(store),
        Err(e) => {
            report(world, format!("opening the store: {e}"));
            return;
        }
    };
    let bin = match crash_bin() {
        Ok(bin) => bin,
        Err(e) => {
            report(world, e);
            return;
        }
    };
    let mut child = match std::process::Command::new(bin)
        .args([mode, &world.store_path.to_string_lossy()])
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            report(world, format!("spawning the crash child: {e}"));
            return;
        }
    };
    // The child prints its token, then idles under an open connection, so the read
    // ends at the token and the kill lands on a live process.
    // shortcut: the read has no deadline; the child's own sleep is bounded, and its
    // error paths exit and close the pipe. Add a recv timeout if a hang ever shows up.
    let line = child.stdout.take().and_then(|out| {
        std::io::BufReader::new(out)
            .lines()
            .next()
            .and_then(|l| l.ok())
    });
    match line.as_deref() {
        Some(got) if got == token => {}
        Some(got) => {
            report(
                world,
                format!("the crash child reported {got}, expected {token}"),
            );
        }
        None => report(world, "the crash child printed nothing".to_string()),
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Closes the lane's connection and opens the store again: the process restart, seen
/// from the store's side.
fn reopen(world: &mut World) -> Result<Store, String> {
    drop(world.store.take());
    Store::open(&world.store_path).map_err(|e| e.to_string())
}

#[given("a store whose turn was committed by a process that is killed")]
async fn killed_after_commit(world: &mut World) {
    world.findings.clear();
    kill_after_report(world, "committed", "committed");
}

#[given("a store whose rows were written by a process that is killed")]
async fn killed_mid_transaction(world: &mut World) {
    world.findings.clear();
    // The child names the rows it wrote before it is killed, so a pass means a
    // transaction was open with rows in it, not that the child wrote nothing.
    let token = format!("writing {}", EVENTS.len());
    kill_after_report(world, "mid", &token);
}

#[then("the committed turn is returned in full")]
async fn committed_turn_survives(world: &mut World) {
    // R-session-06: the process died after the commit, and the reopened store answers
    // with the whole turn. R-session-07 is the marker read below: it reads committed
    // after the crash. That the marker arrived inside the transaction, and not after
    // it, is the unit test in tau-core, which fails the transaction's last statement.
    let mut findings: Vec<String> = Vec::new();
    let store = match reopen(world) {
        Ok(store) => store,
        Err(e) => {
            report(world, format!("reopening the store: {e}"));
            clean_store(world);
            assert_clean(world, "the committed turn is returned in full");
            return;
        }
    };
    let expected: Vec<(i64, &str, &str)> = EVENTS.iter().map(|(k, p, ts)| (*ts, *k, *p)).collect();
    match store.events(SESSION, TURN) {
        Ok(events)
            if events
                .iter()
                .map(|(ts, kind, payload)| (*ts, kind.as_str(), payload.as_str()))
                .eq(expected.clone()) => {}
        Ok(events) => findings.push(format!(
            "the committed turn's events are {events:?}, expected {expected:?}"
        )),
        Err(e) => findings.push(format!("reading events: {e}")),
    }
    match store.turn_state(SESSION, TURN) {
        Ok(Some(state)) if state == STATE => {}
        Ok(Some(state)) => {
            findings.push(format!("the stored state is {state}, expected {STATE}"));
        }
        Ok(None) => findings.push("the state change is not stored".to_string()),
        Err(e) => findings.push(format!("reading the state: {e}")),
    }
    drop(store);
    let marker = raw_conn(world).and_then(|conn| {
        conn.query_row(
            "SELECT committed FROM turn WHERE session = ?1 AND turn = ?2",
            rusqlite::params![SESSION, TURN],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())
    });
    match marker {
        Ok(1) => {}
        Ok(other) => findings.push(format!("the turn's committed marker is {other}")),
        Err(e) => findings.push(format!("reading the committed marker: {e}")),
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the committed turn is returned in full");
}

#[then("the killed transaction leaves the turn empty")]
async fn killed_transaction_leaves_the_turn_empty(world: &mut World) {
    // R-session-02's fact under a crash: rows written inside a transaction that never
    // committed are gone after the restart, so the turn has no log rows and no state.
    let mut findings: Vec<String> = Vec::new();
    let store = match reopen(world) {
        Ok(store) => store,
        Err(e) => {
            report(world, format!("reopening the store: {e}"));
            clean_store(world);
            assert_clean(world, "the killed transaction leaves the turn empty");
            return;
        }
    };
    match store.events(SESSION, TURN) {
        Ok(events) if events.is_empty() => {}
        Ok(events) => findings.push(format!(
            "{} events are stored by a killed transaction",
            events.len()
        )),
        Err(e) => findings.push(format!("reading events: {e}")),
    }
    match store.turn_state(SESSION, TURN) {
        Ok(None) => {}
        Ok(Some(state)) => {
            findings.push(format!("a killed transaction stored the state {state}"));
        }
        Err(e) => findings.push(format!("reading the state: {e}")),
    }
    drop(store);
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the killed transaction leaves the turn empty");
}

#[then("the state tables hold no log content")]
async fn state_tables_hold_no_log(world: &mut World) {
    // R-session-03: event content goes to the log and nowhere else. A copy in a
    // mutable table is the finding, because that copy needs reconciling on the
    // crash day, and the readback proves the commit under test actually ran.
    let mut findings: Vec<String> = Vec::new();
    match world
        .store
        .as_ref()
        .map(|store| store.events(SESSION, TURN))
    {
        None => findings.push("no store is open".to_string()),
        Some(Err(e)) => findings.push(format!("reading events: {e}")),
        Some(Ok(rows)) if rows.len() != EVENTS.len() => findings.push(format!(
            "{} events are stored, expected {}",
            rows.len(),
            EVENTS.len()
        )),
        Some(Ok(_)) => {}
    }
    // Every table and every column is read, not an agreed list: `turn` and `sessions`
    // gaining a column, or a new table appearing, has to be scanned for log text.
    // Rows are joined on 0x1f, a character JSON payloads cannot contain, so a stored
    // value never splits where a payload does not.
    const MUTABLE: &[&str] = &["sessions", "turn"];
    // shortcut: this is today's schema, and a new mutable table is meant to fire the
    // guard by name. `adopt_observational_memory` adds observation and reflection
    // tables to the same store; whoever lands it names them here, and the scan covers
    // them from that line on.
    let read = raw_conn(world).and_then(|conn| {
        let rows = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<(String, Option<String>)>, _>>()
            .map_err(|e| e.to_string())?;
        // An FTS index is a set of tables: the virtual table and its shadow tables, whose
        // rows are indexed text by design, so they are not mutable state. shortcut: the
        // split is by name — a virtual table and `<name>_*` — so a mutable table wearing
        // an index shadow name would hide from this step; upgrade to reading the schema's
        // own kinds if a real table ever collides.
        let index: Vec<&str> = rows
            .iter()
            .filter(|(_, sql)| {
                sql.as_deref()
                    .is_some_and(|s| s.starts_with("CREATE VIRTUAL TABLE"))
            })
            .map(|(name, _)| name.as_str())
            .collect();
        let tables: Vec<String> = rows
            .iter()
            .filter(|(name, _)| {
                name != "events"
                    && !index
                        .iter()
                        .any(|v| name == v || name.starts_with(&format!("{v}_")))
            })
            .map(|(name, _)| name.clone())
            .collect();
        let mut text = String::new();
        for table in &tables {
            let cols = conn
                .prepare("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .map_err(|e| e.to_string())?
                .query_map(rusqlite::params![table], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<String>, _>>()
                .map_err(|e| e.to_string())?;
            let cast: Vec<String> = cols
                .iter()
                .map(|c| format!("IFNULL(CAST(\"{}\" AS TEXT), '')", c.replace('"', "\"\"")))
                .collect();
            let sql = format!(
                "SELECT IFNULL(group_concat(\"v\", char(31)), '') \
                 FROM (SELECT {} AS \"v\" FROM \"{}\")",
                cast.join(" || char(31) || "),
                table.replace('"', "\"\"")
            );
            let one = conn
                .query_row(&sql, [], |row| row.get::<_, String>(0))
                .map_err(|e| format!("reading {table}: {e}"))?;
            text.push_str(&one);
            text.push('\u{1f}');
        }
        Ok((tables, text))
    });
    match read {
        Err(e) => findings.push(format!("reading the store's rows: {e}")),
        Ok((tables, text)) => {
            let names: Vec<&str> = tables.iter().map(String::as_str).collect();
            if names != MUTABLE {
                findings.push(format!(
                    "the store holds the tables {}, this step reads {}",
                    names.join(", "),
                    MUTABLE.join(", ")
                ));
            }
            // Each stored value is asked on its own, and a payload too short to be
            // distinctive is skipped: matching a one-character payload would report
            // the schema's own values as log content.
            let values: Vec<&str> = text.split('\u{1f}').collect();
            for (_kind, payload, _ts) in EVENTS {
                if payload.len() < 2 {
                    continue;
                }
                if values
                    .iter()
                    .any(|value| value.len() >= 2 && value.contains(payload))
                {
                    findings.push(format!("a state table holds log content {payload:?}"));
                }
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the state tables hold no log content");
}

#[then("the store's synchronous is NORMAL")]
async fn synchronous_is_normal(world: &mut World) {
    // R-session-05: 1 is NORMAL, and the store's own connection is the one that
    // answers, since the setting is per connection.
    match world.store.as_ref().map(Store::synchronous) {
        None => report(world, "no store is open".to_string()),
        Some(Err(e)) => report(world, format!("reading synchronous: {e}")),
        Some(Ok(1)) => {}
        Some(Ok(v)) => report(world, format!("synchronous is {v}, expected 1 (NORMAL)")),
    }
    clean_store(world);
    assert_clean(world, "the store's synchronous is NORMAL");
}

#[given("the store's own connection enforces foreign keys")]
async fn store_enforces_foreign_keys(world: &mut World) {
    // R-session-32: the schema's keys bind the store's own writes. The setting is per
    // connection, so a reader connection cannot answer for the store.
    match world.store.as_ref().map(Store::foreign_keys) {
        None => report(world, "no store is open".to_string()),
        Some(Err(e)) => report(world, format!("reading foreign_keys: {e}")),
        Some(Ok(1)) => {}
        Some(Ok(v)) => report(world, format!("foreign keys are {v}, expected 1 (ON)")),
    }
}

/// What the setup child ended with: its stdout lines, its stderr, its exit status, and
/// the scratch path it was given.
struct SetupChild {
    path: PathBuf,
    lines: Vec<String>,
    stderr: String,
    status: std::process::ExitStatus,
}

/// The R-session-33 premise: a fresh store path, a connection of the step's own holding a
/// write lock across the store's setup, and a child instance opening the same path behind
/// that lock. `leg` picks what sits behind the lock: `wal` leaves the file in the mode a
/// fresh file starts in, so the journal upgrade is what is refused; `migrations` puts the
/// file in WAL first, so the upgrade succeeds and the migrations are what is refused. The
/// witness under the lock proves the store's setup had not finished while the lock stood.
fn setup_race(leg: &str, beat: u64) -> Result<SetupChild, String> {
    let path = store_path("setup-race");
    // The locking connection opens the file before the store does, and only the store
    // creates the directory, so the directory is made here.
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating the store directory: {e}"))?;
    }
    let locker = rusqlite::Connection::open(&path)
        .map_err(|e| format!("opening the locking connection: {e}"))?;
    // A cold setup has no store tables yet, so the lock is held on a marker table of the
    // step's own. `execute_batch` leaves the transaction open, which is the point: an open
    // write transaction is what the setup waits behind.
    let sql = if leg == "migrations" {
        // WAL from the locker first: the store's own upgrade then succeeds, and the lock
        // falls on the migrations, whose refusal has the other shape.
        "PRAGMA journal_mode = WAL; CREATE TABLE IF NOT EXISTS setup_lock (a); \
         BEGIN IMMEDIATE; INSERT INTO setup_lock (a) VALUES (1);"
    } else {
        "CREATE TABLE IF NOT EXISTS setup_lock (a); \
         BEGIN IMMEDIATE; INSERT INTO setup_lock (a) VALUES (1);"
    };
    locker
        .execute_batch(sql)
        .map_err(|e| format!("taking the setup lock: {e}"))?;
    let bin = std::env::var("CARGO_BIN_EXE_open_store")
        .map(PathBuf::from)
        .map_err(|_| "the setup child's binary is not set".to_string())?;
    let mut child = std::process::Command::new(&bin)
        .args([path.to_string_lossy().as_ref()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawning the setup child: {e}"))?;
    let outcome = race_behind_the_lock(&mut child, &locker, &path, leg, beat);
    // The child is reaped on every path, red included: a child left opening a store keeps
    // writing into a directory this scenario deletes, and the store recreates it.
    child.kill().ok();
    child.wait().ok();
    outcome
}

fn race_behind_the_lock(
    child: &mut std::process::Child,
    locker: &rusqlite::Connection,
    path: &std::path::Path,
    leg: &str,
    beat: u64,
) -> Result<SetupChild, String> {
    // Both pipes are drained by a thread of their own. A child blocked on the lock has
    // nothing else to say, and a full pipe would stop it for the wrong reason.
    let stdout = child.stdout.take().ok_or("the setup child has no stdout")?;
    let mut stderr = child.stderr.take().ok_or("the setup child has no stderr")?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let (etx, erx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let _ = etx.send(text);
    });
    // The child's first line says it has reached the store, so the parent is measuring an
    // instance at the setup, not one that merely started.
    let reaching = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "the setup child never reported reaching the store".to_string())?;
    if reaching.trim() != "opening" {
        return Err(format!("the setup child spoke first with {reaching:?}"));
    }
    // shortcut: a fixed beat, held long enough for the child to meet the lock and be
    // refused at least once. It is not what makes the scenario non-vacuous: the refusals
    // the child reports are, and a beat that ends too early turns the scenario red.
    std::thread::sleep(std::time::Duration::from_millis(beat));
    // The setup is provably unfinished on this side of the lock, while the lock stands.
    if leg == "migrations" {
        // The leg's own premise, checked: the file is already in WAL, so the store's
        // upgrade succeeds and the lock it meets is the migrations'.
        let mode = locker
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(|e| format!("reading the mode under the lock: {e}"))?;
        if mode != "wal" {
            return Err(format!(
                "the migration leg left the store in {mode} mode, expected wal"
            ));
        }
        let tables = locker
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'events'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("reading the schema under the lock: {e}"))?;
        if tables != 0 {
            return Err("the store's tables were created while the lock was held".to_string());
        }
    } else {
        let mode = locker
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(|e| format!("reading the mode under the lock: {e}"))?;
        if mode != "delete" {
            return Err(format!(
                "the store was set up to {mode} mode while the lock was held"
            ));
        }
    }
    locker
        .execute_batch("COMMIT")
        .map_err(|e| format!("releasing the setup lock: {e}"))?;
    let status = child
        .wait()
        .map_err(|e| format!("waiting for the setup child: {e}"))?;
    // The reader thread closes the channel when the child's stdout ends, so this drains
    // every line the child said and returns when it has heard them all.
    let mut lines = Vec::new();
    while let Ok(line) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
        lines.push(line);
    }
    let stderr_text = erx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap_or_default();
    Ok(SetupChild {
        path: path.to_path_buf(),
        lines,
        stderr: stderr_text,
        status,
    })
}

/// The child opened, and met the lock. A refusal count of 0 is reported: it would mean
/// the beat ended before the child met the lock, and a scenario that measured no retry
/// must not be allowed to read as a pass.
fn report_setup_open(world: &mut World, child: &SetupChild) {
    if !child.status.success() {
        report(
            world,
            format!("the second instance failed: {}", child.stderr.trim()),
        );
        return;
    }
    let refusals = child
        .lines
        .iter()
        .find(|line| line.starts_with("opened"))
        .and_then(|line| line.split("refusals=").last())
        .and_then(|tail| tail.trim().parse::<u32>().ok());
    match refusals {
        Some(0) => report(
            world,
            "the second instance met no refusal, so the setup race was never measured".to_string(),
        ),
        Some(_) => {}
        None => report(
            world,
            format!(
                "the second instance reported no refusal count: {}",
                child.lines.join(" ")
            ),
        ),
    }
}

#[when("a second instance opens the store while its setup is locked")]
async fn second_instance_opens_under_a_locked_setup(world: &mut World) {
    // R-session-33, the journal-upgrade leg: the WAL upgrade is refused the moment a lock
    // stands, so the store's bounded retry is what carries the open.
    match setup_race("wal", 300) {
        Ok(child) => {
            world.store_path = child.path.clone();
            report_setup_open(world, &child);
        }
        Err(e) => report(world, e),
    }
}

#[when("a second instance opens the store while its migrations are locked")]
async fn second_instance_opens_under_a_locked_migration(world: &mut World) {
    // R-session-33, the migration leg, whose refusal has the other shape: SQLite rolls the
    // deferred migration transaction back, so the statement fails even after the busy
    // handler has waited. The beat is longer, because an attempt here spends the setup's
    // 100 ms busy timeout before it is refused.
    match setup_race("migrations", 400) {
        Ok(child) => {
            world.store_path = child.path.clone();
            report_setup_open(world, &child);
        }
        Err(e) => report(world, e),
    }
}

#[when("the setup lock outlasts the store's retry budget")]
async fn setup_lock_outlasts_the_budget(world: &mut World) {
    // The retry is bounded, so a lock that outlasts it ends the open instead of parking
    // the instance. The beat is 1.5 s: past the budget the upgrade leg spends, and well
    // inside any plausible stall.
    let outcome = setup_race("wal", 1500);
    if let Ok(child) = outcome.as_ref() {
        // Kept as text, so the Then step reads the caller-visible refusal.
        world.setup_child = format!(
            "exit {}; {}; {}",
            child.status.code().unwrap_or(-1),
            child.lines.join(" "),
            child.stderr.trim()
        );
        // Nothing after this step reads the store, so its scratch goes here.
        if let Some(dir) = child.path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }
    if let Err(e) = outcome {
        report(world, e);
    }
}

#[then("the store on disk is in WAL mode")]
async fn store_on_disk_is_in_wal(world: &mut World) {
    // The mode lives in the file, so a reader connection answers for every instance.
    match rusqlite::Connection::open(&world.store_path)
        .and_then(|conn| conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)))
    {
        Ok(mode) if mode == "wal" => {}
        Ok(mode) => report(
            world,
            format!("the store on disk is in {mode} mode, expected wal"),
        ),
        Err(e) => report(world, format!("reading the store's journal mode: {e}")),
    }
    // This scenario's store is its own scratch, so it is removed here.
    if let Some(dir) = world.store_path.parent() {
        std::fs::remove_dir_all(dir).ok();
    }
    clean_store(world);
    assert_clean(world, "the store on disk is in WAL mode");
}

#[then("the second instance is refused with a busy store")]
async fn second_instance_is_refused_by_the_setup_lock(world: &mut World) {
    // The budget is bounded, so this is what a caller sees: the open fails with SQLite's
    // own words, and no store is reported open.
    let said = std::mem::take(&mut world.setup_child);
    if !said.contains("database is locked") || said.contains("opened") {
        report(
            world,
            format!("the second instance did not report a busy store: {said}"),
        );
    }
    clean_store(world);
    assert_clean(world, "the second instance is refused with a busy store");
}
#[when("the log is read between turns")]
async fn read_between_turns(world: &mut World) {
    // The store's reader API, in autocommit: each read ends its own read
    // transaction. No turn is open here, because a turn's rows exist only once its
    // commit has run. The clause's `WHILE` window is the other scenario's: there, the
    // turn's own transaction sits open on a second connection while these same reads run.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            if let Err(e) = store.events(SESSION, TURN + 1) {
                findings.push(format!("reading events: {e}"));
            }
            if let Err(e) = store.turn_state(SESSION, TURN + 1) {
                findings.push(format!("reading the turn state: {e}"));
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
}

#[when("the log is read while a turn's transaction is open")]
async fn read_inside_an_open_turn(world: &mut World) {
    // R-session-04's `WHILE` window, opened: the turn's own transaction — state row
    // first, then events, the order the schema's foreign key asks for — sits open on a
    // second connection while the store's reader API runs. The transaction closes at the
    // end of this step: a writer in flight blocks a checkpoint on its own, and the
    // closing step's probe measures readers.
    let mut findings: Vec<String> = Vec::new();
    let in_flight = TURN + 1;
    match raw_conn(world) {
        Err(e) => findings.push(format!("opening a writer connection: {e}")),
        Ok(writer) => {
            let open = writer.execute_batch("BEGIN IMMEDIATE").and_then(|_| {
                writer.execute(
                    "INSERT INTO turn (session, turn, state) VALUES (?1, ?2, ?3)",
                    rusqlite::params![SESSION, in_flight, STATE],
                )?;
                writer.execute(
                    "INSERT INTO events (session, turn, ts, kind, payload)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![SESSION, in_flight, 1003, "tool_call", "in flight"],
                )
            });
            if let Err(e) = open {
                findings.push(format!("opening the turn's transaction: {e}"));
            }
            match world.store.as_ref() {
                None => findings.push("no store is open".to_string()),
                Some(store) => {
                    if let Err(e) = store.events(SESSION, in_flight) {
                        findings.push(format!("reading events: {e}"));
                    }
                    if let Err(e) = store.turn_state(SESSION, in_flight) {
                        findings.push(format!("reading the turn state: {e}"));
                    }
                    match store.run(SESSION) {
                        Err(e) => findings.push(format!("reading the run: {e}")),
                        // The turn in flight is invisible here: its transaction is open
                        // on the other connection, so a run read inside the window
                        // answers the committed turns and nothing else.
                        Ok(run) => {
                            let turns: Vec<i64> = run.iter().map(|t| t.turn).collect();
                            let want = vec![TURN];
                            if turns != want {
                                findings.push(format!(
                                    "a run read inside the window answers turns {turns:?}, expected {want:?}"
                                ));
                            }
                        }
                    }
                }
            }
            if let Err(e) = writer.execute_batch("COMMIT") {
                findings.push(format!("closing the turn's transaction: {e}"));
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
}

#[then("the log checkpoints with no reader in the way")]
async fn checkpoint_has_no_reader(world: &mut World) {
    // R-session-04's `Why:` names the auto-checkpoint, and that one is PASSIVE: with a
    // reader in the way it copies what it can and leaves the frames, so the harm it
    // promises is a log that does not shrink. `TRUNCATE` is the
    // stricter probe — it reports a lock — and its lock leg only speaks when the log
    // holds frames, which is why a commit runs before this step.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            match store.events(SESSION, TURN) {
                Err(e) => findings.push(format!("reading events: {e}")),
                // No frames means the checkpoint has nothing to do, and answers
                // `(0, 0)` for reasons unrelated to a free log.
                Ok(rows) if rows.is_empty() => findings.push(
                    "the log holds no events, so the checkpoint has no frames to copy".to_string(),
                ),
                Ok(_) => {}
            }
            if let Err(e) = store.turn_state(SESSION, TURN) {
                findings.push(format!("reading the turn state: {e}"));
            }
        }
    }
    let outcome = raw_conn(world).and_then(|conn| {
        // No waiting: a checkpoint that blocks out a reader which then finishes is
        // the probe passing while the clause is violated.
        conn.busy_timeout(std::time::Duration::ZERO)
            .map_err(|e| e.to_string())?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())
    });
    match outcome {
        Err(e) => findings.push(format!("the checkpoint failed: {e}")),
        // A store not in WAL mode answers with -1 frames; `Store::open` rules that
        // out, so this is a report, not a route.
        Ok((_, -1)) => findings.push("the store answers the checkpoint as not WAL".to_string()),
        Ok((busy, _)) if busy != 0 => {
            findings.push("the checkpoint hit a lock: a reader holds the log".to_string());
        }
        // The pragma's second column is the frames left in the log, the number the
        // `Why:` says must not grow.
        Ok((_, frames)) if frames != 0 => {
            findings.push(format!(
                "the checkpoint left {frames} frames in the log: the log grows"
            ));
        }
        Ok((_, _)) => {}
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the log checkpoints with no reader in the way");
}

/// The fork's session id.
const FORK: &str = "lane-fork";

#[when("the session is forked at the committed turn")]
async fn fork_the_session(world: &mut World) {
    // R-session-08: one committed turn, copied into a session of its own.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.fork(SESSION, FORK, TURN, TURN) {
        report(world, format!("the fork failed: {e}"));
    }
}

#[then("the fork holds the copied events and names its parent")]
async fn fork_holds_the_copy(world: &mut World) {
    // R-session-08: the copied range reads back the same, and the new session records
    // the session it came from.
    let mut findings = Vec::new();
    if let Some(store) = world.store.as_ref() {
        match (store.events(SESSION, TURN), store.events(FORK, TURN)) {
            (Ok(want), Ok(got)) => {
                if got.is_empty() {
                    findings.push("the fork holds no events".to_string());
                } else if got != want {
                    findings.push(format!("the fork's events are {got:?}, expected {want:?}"));
                }
            }
            (Err(e), _) => findings.push(format!("reading the parent's events: {e}")),
            (_, Err(e)) => findings.push(format!("reading the fork's events: {e}")),
        }
        match store.parent(FORK) {
            Ok(Some(parent)) if parent == SESSION => {}
            Ok(Some(parent)) => findings.push(format!(
                "the fork names {parent} as its parent, expected {SESSION}"
            )),
            Ok(None) => findings.push("the fork names no parent".to_string()),
            Err(e) => findings.push(format!("reading the fork's parent: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(
        world,
        "the fork holds the copied events and names its parent",
    );
}

#[then("the index answers a search of the stored events")]
async fn index_answers_the_search(world: &mut World) {
    // R-session-10: the answer comes from the index, so the query is FTS5 syntax. The
    // first query names a column, which no scan of the payload text can answer; the
    // second asks for a term no event holds, so an index that matched everything is caught.
    let mut findings = Vec::new();
    if let Some(store) = world.store.as_ref() {
        match store.search(r#"kind:"tool call""#) {
            Ok(hits) => {
                let rows: Vec<(&str, &str)> = hits
                    .iter()
                    .map(|(session, _, _, kind, _)| (session.as_str(), kind.as_str()))
                    .collect();
                // The fork's copy arrives by `INSERT … SELECT`, so its index rows prove
                // the trigger fires on the bulk path, not only on a one-row insert.
                if rows != [(SESSION, "tool_call"), (FORK, "tool_call")] {
                    findings.push(format!(
                        "the search answers with {rows:?}, expected [({SESSION:?}, \"tool_call\"), ({FORK:?}, \"tool_call\")]"
                    ));
                }
            }
            Err(e) => findings.push(format!("the search failed: {e}")),
        }
        match store.search("no such term appears here") {
            Ok(hits) if hits.is_empty() => {}
            Ok(hits) => findings.push(format!(
                "a search for a term no event holds answers {} hits",
                hits.len()
            )),
            Err(e) => findings.push(format!("the second search failed: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the index answers a search of the stored events");
}

#[when("the session is exported")]
async fn export_the_session(world: &mut World) {
    // R-session-11: the export goes to bytes the lane keeps, so the next step reads the
    // lines the store wrote. The store counts the rows its connection has changed, read
    // on either side of the export: a reader connection cannot answer for that counter,
    // and "leave the store unchanged" is a fact about the store's writes. It is measured
    // at row level, not as bytes of the file.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let before = store.total_changes();
    let mut out: Vec<u8> = Vec::new();
    let written = store.export_jsonl(SESSION, &mut out);
    let after = store.total_changes();
    world.exported = String::from_utf8_lossy(&out).into_owned();
    if let Err(e) = written {
        report(world, format!("the export failed: {e}"));
    }
    if after != before {
        report(
            world,
            format!("the export wrote {} rows of the store", after - before),
        );
    }
}

/// The state of a turn that produced no events.
const EMPTY_STATE: &str = "awaiting-user";

#[when("a turn that produced no events commits")]
async fn commit_a_turn_with_no_events(world: &mut World) {
    // R-session-13: a model turn with no tool call is a real shape of a run, and the
    // harness reads it as a turn with an empty event list.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let turn = Turn {
        session: SESSION,
        turn: TURN + 1,
        state: EMPTY_STATE,
        events: &[],
    };
    if let Err(e) = store.commit(&turn) {
        report(world, format!("the turn with no events failed: {e}"));
    }
}

#[then("the run answers every turn of the session with its events")]
async fn run_answers_from_the_store(world: &mut World) {
    // R-session-13: a run is a query over the state table and the log — each turn's
    // state, its committed marker, its events. A turn that produced no events arrives with
    // an empty list, not as a missing turn.
    let mut findings = Vec::new();
    if let Some(store) = world.store.as_ref() {
        match store.run(SESSION) {
            Ok(run) => {
                let want = RunTurn {
                    turn: TURN,
                    state: STATE.to_string(),
                    committed: true,
                    events: EVENTS
                        .iter()
                        .map(|(kind, payload, ts)| {
                            (*ts, (*kind).to_string(), (*payload).to_string())
                        })
                        .collect(),
                };
                match run.first() {
                    Some(got) if *got == want => {}
                    Some(got) => findings.push(format!(
                        "the run's first turn is {got:?}, expected {want:?}"
                    )),
                    None => findings.push("the run answers no turns".to_string()),
                }
                let want = RunTurn {
                    turn: TURN + 1,
                    state: EMPTY_STATE.to_string(),
                    committed: true,
                    events: Vec::new(),
                };
                match run.get(1) {
                    Some(got) if *got == want => {}
                    Some(got) => findings.push(format!(
                        "the run's second turn is {got:?}, expected a turn with no events: {want:?}"
                    )),
                    None => findings.push(format!(
                        "the run answers {} turns, expected 2 with a turn that produced no events",
                        run.len()
                    )),
                }
            }
            Err(e) => findings.push(format!("reading the run: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(
        world,
        "the run answers every turn of the session with its events",
    );
}

#[then("the export holds one line per event and the store answers the same")]
async fn export_lines_and_store(world: &mut World) {
    // R-session-11: each line carries the store's row, and the store answers the same
    // afterwards. The counter check that proves the export wrote nothing runs in the
    // export step, where the store can be measured on either side of it.
    let mut findings: Vec<String> = Vec::new();
    let lines: Vec<&str> = world.exported.lines().collect();
    if lines.len() != EVENTS.len() {
        findings.push(format!(
            "the export writes {} lines, expected {}",
            lines.len(),
            EVENTS.len()
        ));
    }
    for (line, (kind, payload, ts)) in lines.iter().zip(EVENTS) {
        // The payload is JSON text, so it arrives escaped inside the line's string.
        let escaped = payload.replace('"', "\\\"");
        for want in [
            format!(r#""session":"{SESSION}""#),
            format!(r#""turn":{TURN}"#),
            format!(r#""ts":{ts}"#),
            format!(r#""kind":"{kind}""#),
            format!(r#""payload":"{escaped}""#),
            r#""seq":"#.to_string(),
        ] {
            if !line.contains(&want) {
                findings.push(format!("an export line holds no {want}: {line}"));
            }
        }
    }
    if let Some(store) = world.store.as_ref() {
        match store.events(SESSION, TURN) {
            Ok(rows) if rows.len() == EVENTS.len() => {}
            Ok(rows) => findings.push(format!(
                "{} events are stored after the export, expected {}",
                rows.len(),
                EVENTS.len()
            )),
            Err(e) => findings.push(format!("reading events after the export: {e}")),
        }
        match store.turn_state(SESSION, TURN) {
            Ok(Some(state)) if state == STATE => {}
            Ok(other) => findings.push(format!(
                "the state change after the export is {other:?}, expected {STATE:?}"
            )),
            Err(e) => findings.push(format!("reading the state after the export: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
}
