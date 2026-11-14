//! The storage steps: the lane opens a project store, commits a tool call, and
//! reads the store back (R-session-01, 02, 28, 29, 30, 31, 32). A crash is a child
//! process the lane kills (`bin/crash_store.rs`), and the store is read back after it
//! dies (R-session-06, 07).

use crate::steps::{assert_clean, report};
use crate::world::World;
use cucumber::{given, then, when};
use std::io::BufRead;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tau_core::storage::{Error, Store, Turn};

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
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.created(SESSION) {
            Ok(Some(ts)) if ts > 0 => {}
            Ok(Some(ts)) => findings.push(format!("the session records a creation time of {ts}")),
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
