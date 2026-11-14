//! The crash child: it writes to a store and is then killed by a signal, so the
//! behaviour lane reads a store as it stands after an application crash
//! (R-session-06). `committed` commits a turn and reports it; `mid` writes a
//! transaction's rows, keeps that transaction open, and reports how many it wrote.

use rusqlite::Connection;
use std::io::Write;
use std::time::Duration;
use tau_core::storage::{Store, Turn};
use tau_test::steps::session::{EVENTS, SESSION, STATE, TURN};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or_default();
    let path = args.get(1).map(String::as_str).unwrap_or_default();
    let outcome = match mode {
        "committed" => committed(path).map(|()| None),
        "mid" => mid(path).map(Some),
        _ => Err("usage: crash_store <committed|mid> <path>".to_string()),
    };
    match outcome {
        // In `mid` the connection carries the open transaction, so it lives across the
        // sleep and the signal is what ends it. The sleep is bounded, so a child the
        // lane never killed closes on its own.
        Ok(tx) => {
            let _tx = tx;
            std::thread::sleep(Duration::from_secs(30));
        }
        Err(e) => {
            eprintln!("crash_store: {e}");
            std::process::exit(1);
        }
    }
}

/// Opens the store and its session, the rows the store stamps itself.
fn prepare(path: &str) -> Result<Store, String> {
    let store = Store::open(path).map_err(|e| e.to_string())?;
    store.open_session(SESSION).map_err(|e| e.to_string())?;
    Ok(store)
}

/// Commits a turn through the store, then reports it.
fn committed(path: &str) -> Result<(), String> {
    let mut store = prepare(path)?;
    let turn = Turn {
        session: SESSION,
        turn: TURN,
        state: STATE,
        events: EVENTS,
    };
    store.commit(&turn).map_err(|e| e.to_string())?;
    report("committed");
    Ok(())
}

/// Writes a turn's rows in a transaction and hands the open connection back, so this
/// process's death is what ends the transaction. The session row lands beforehand,
/// committed, because the schema's key requires it. The row count is reported, so the
/// lane kills a process that is known to have written, not one that wrote nothing.
fn mid(path: &str) -> Result<Connection, String> {
    prepare(path)?;
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO turn (session, turn, state) VALUES (?1, ?2, ?3)",
        rusqlite::params![SESSION, TURN, STATE],
    )
    .map_err(|e| e.to_string())?;
    for (kind, payload, ts) in EVENTS {
        conn.execute(
            "INSERT INTO events (session, turn, ts, kind, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![SESSION, TURN, ts, kind, payload],
        )
        .map_err(|e| e.to_string())?;
    }
    report(&format!("writing {}", EVENTS.len()));
    Ok(conn)
}

fn report(token: &str) {
    let out = std::io::stdout();
    let mut out = out.lock();
    let _ = writeln!(out, "{token}");
    let _ = out.flush();
}
