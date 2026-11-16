//! The child process of the crash scenarios: it writes blocks to the store the lane
//! hands it, prints a line naming what it wrote, and is then killed by the lane. Mode
//! `committed` runs the commit through `Store` and stamps the request's terminal, then
//! prints `committed`. Mode `blocks` commits the blocks, stops before the stamp, prints
//! `blocks`, and idles with the store open, so the kill lands in the window between the two
//! writes. Mode `mid` opens a transaction, inserts its rows, prints
//! `writing <n>` naming the block rows it wrote, and keeps the connection open while it idles,
//! so the kill lands with the transaction open. The lane asserts on the store after the
//! process dies.
//!
//! The rows the child writes are the lane's own constants from `steps::session`, imported
//! rather than copied, so the crash scenarios assert on rows the child can only have
//! written as written.

use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;
use tau_core::storage::Store;
use tau_test::steps::session::{
    BLOCKS, MODEL, PROVIDER_ID, REASON, REQUEST, SESSION, USAGE_CACHE_READ, USAGE_CACHE_WRITE,
    USAGE_IN, USAGE_OUT,
};

/// Commit the blocks and stop before the terminal: the window R-session-40's second write
/// leaves. The store is returned, so the caller holds it open across the lane's kill.
fn commit_blocks_and_stop(path: &Path) -> Result<Store, String> {
    let mut store = Store::open(path).map_err(|e| format!("opening the store: {e}"))?;
    store
        .open_session(SESSION)
        .map_err(|e| format!("opening the session: {e}"))?;
    store
        .open_request(SESSION, REQUEST, Some(PROVIDER_ID), MODEL)
        .map_err(|e| format!("opening the request: {e}"))?;
    let rows: Vec<tau_core::storage::Block<'_>> = BLOCKS
        .iter()
        .map(|(kind, payload, ts)| tau_core::storage::Block {
            session: SESSION,
            request: Some(REQUEST),
            ts: *ts,
            kind,
            payload,
            slot: None,
            state_start: None,
            state_end: None,
        })
        .collect();
    store
        .commit_all(&rows)
        .map_err(|e| format!("committing the blocks: {e}"))?;
    Ok(store)
}
/// Write the session, request and block rows inside one open transaction, and answer how
/// many block rows landed. The caller keeps the connection open across the lane's kill, since SQLite
/// closes a transaction when its connection closes.
fn write_mid_transaction(conn: &mut Connection) -> Result<usize, String> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("opening the transaction: {e}"))?;
    // The session row a store claims before it writes: the keys bind these rows, so the
    // child writes the same rows a commit in flight writes, and rolls back the lot.
    conn.execute(
        "INSERT INTO sessions (id, created) VALUES (?1, ?2)",
        rusqlite::params![SESSION, now_ms()],
    )
    .map_err(|e| format!("inserting the session: {e}"))?;
    conn.execute(
        "INSERT INTO requests (session, request, provider_id, model) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![SESSION, REQUEST, PROVIDER_ID, MODEL],
    )
    .map_err(|e| format!("inserting the request: {e}"))?;
    let mut written = 0;
    for (kind, payload, ts) in BLOCKS {
        let row = conn
            .execute(
                "INSERT INTO blocks (session, request, ts, kind, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![SESSION, REQUEST, ts, kind, payload],
            )
            .map_err(|e| format!("inserting the block: {e}"))?;
        written += row;
    }
    Ok(written)
}

/// Whole milliseconds since the Unix epoch, the unit the log's stamps use.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// A store whose rows were written in an open transaction: the caller keeps the
/// connection, so the transaction stays open, and the process idles until killed.
fn open_writing(path: &Path) -> Result<(Connection, usize), String> {
    let mut conn = Connection::open(path).map_err(|e| format!("opening the store: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| format!("setting the busy timeout: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("enforcing foreign keys: {e}"))?;
    let written = write_mid_transaction(&mut conn)?;
    Ok((conn, written))
}

/// A store whose block was committed, and whose request was stamped terminal, by a process
/// that is then killed.
fn commit_and_stamp(path: &Path) -> Result<(), String> {
    let mut store = Store::open(path).map_err(|e| format!("opening the store: {e}"))?;
    // The store's own writes are keyed, so the child claims the session before its request.
    store
        .open_session(SESSION)
        .map_err(|e| format!("opening the session: {e}"))?;
    store
        .open_request(SESSION, REQUEST, Some(PROVIDER_ID), MODEL)
        .map_err(|e| format!("opening the request: {e}"))?;
    let blocks: Vec<tau_core::storage::Block<'_>> = BLOCKS
        .iter()
        .map(|(kind, payload, ts)| tau_core::storage::Block {
            session: SESSION,
            request: Some(REQUEST),
            ts: *ts,
            kind,
            payload,
            slot: None,
            state_start: None,
            state_end: None,
        })
        .collect();
    store
        .commit_all(&blocks)
        .map_err(|e| format!("committing the blocks: {e}"))?;
    store
        .stamp_request(
            SESSION,
            REQUEST,
            Some(USAGE_IN),
            Some(USAGE_CACHE_READ),
            Some(USAGE_CACHE_WRITE),
            Some(USAGE_OUT),
            REASON,
        )
        .map_err(|e| format!("stamping the terminal: {e}"))?;
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let path = args.next().unwrap_or_default();
    // The mid-mode connection is held here, so the transaction stays open across the sleep:
    // a connection dropped at the end of the branch would roll the rows back before the
    // lane's kill lands, and the scenario would pass without measuring a crash.
    let mut _writing: Option<Connection> = None;
    let mut _held: Option<Store> = None;
    if mode == "committed" {
        if let Err(e) = commit_and_stamp(Path::new(&path)) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        println!("committed");
    } else if mode == "blocks" {
        // The blocks are committed and the terminal is not: the window R-session-40's second
        // write opens. The store is held here, so the lane's kill lands with it open.
        match commit_blocks_and_stop(Path::new(&path)) {
            Ok(store) => {
                println!("blocks");
                _held = Some(store);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    } else if mode == "mid" {
        match open_writing(Path::new(&path)) {
            Ok((conn, written)) => {
                println!("writing {written}");
                _writing = Some(conn);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("usage: crash_store <committed|blocks|mid> <path>");
        std::process::exit(1);
    }
    // Flush above, then idle with the store open, so the lane's kill lands mid-run.
    // shortcut: a fixed sleep; the lane kills the child. Use 30 s so a child left
    // running by a lane bug ends by itself.
    std::thread::sleep(Duration::from_secs(30));
}
