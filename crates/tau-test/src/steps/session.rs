//! The storage steps: the lane opens a project store, commits a tool call, and
//! reads the store back (R-session-01, 02, 28, 29, 30, 31, 32).

use crate::steps::{assert_clean, report};
use crate::world::World;
use cucumber::{given, then, when};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tau_core::storage::{Error, Store, Turn};

/// The session a scenario opens, and the turn a tool call commits.
const SESSION: &str = "lane-session";
const TURN: i64 = 1;
const STATE: &str = "awaiting-model";
const EVENTS: &[(&str, &str, i64)] = &[
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

#[given("an open store")]
async fn open_store(world: &mut World) {
    world.findings.clear();
    world.store = None;
    world.store_path = store_path("lane");
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
