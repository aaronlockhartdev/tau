//! The storage steps: the lane opens a project store, commits a tool call's blocks, and
//! reads the store back (R-session-01, 02, 28, 29, 35, 36, 37, 38, 45). The scan for log
//! content in the mutable tables and the checkpoint probe run on a reader connection
//! of the lane's own, `synchronous` on the store's connection, and the schema refuses
//! writes (R-session-03, 04, 05, 30, 31, 32). A crash is a child process the lane kills
//! (`bin/crash_store.rs`), and the store is read back after it dies (R-session-06, 07).
//! A fork copies a range and the requests it names, the index answers a search, an
//! export writes JSONL, a run and a resume are answered from the log, and the terminal
//! stamps a request's counts (R-session-08, 10, 11, 13, 39, 40, 46).
//! A second process opening a store whose setup is locked is a child process as well
//! (`bin/open_store.rs`), and the refusals it waited out are read back (R-session-33).

use crate::steps::{assert_clean, report};
use crate::world::World;
use cucumber::{given, then, when};
use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tau_core::storage::{Block, Error, Store, StoredBlock};

/// The session a scenario opens, and the request a tool call's blocks name. `pub` because
/// the crash child (`bin/crash_store.rs`) writes exactly these rows, so the steps assert
/// on rows the child can only have written as written.
pub const SESSION: &str = "lane-session";
pub const REQUEST: i64 = 1;
/// The provider's own id for the round-trip, stored as given.
pub const PROVIDER_ID: &str = "lane-request-1";
pub const MODEL: &str = "lane-model";
pub const REASON: &str = "completed";
/// The context slot an injection block fills.
pub const SLOT: &str = "system";
/// The counts one provider reports at its terminal, and the two it never reported.
pub const USAGE_IN: i64 = 120;
pub const USAGE_OUT: i64 = 34;
/// The cache counts, distinct so a stamp that swaps the two columns is visible: a provider
/// reports cache reads and writes separately, and a run pays them at different rates.
pub const USAGE_CACHE_READ: i64 = 900;
pub const USAGE_CACHE_WRITE: i64 = 70;
/// The blocks one tool call commits, naming the request that streamed them, as
/// `(kind, payload, ts)` in the order they are appended. `ts` is the event time
/// (glossary): the caller stamps it, the store records it.
pub const BLOCKS: &[(&str, &str, i64)] = &[
    ("tool_use", r#"{"tool":"read"}"#, 1001),
    ("tool_result", "ok", 1002),
];
/// The blocks one stream closes at once, committed as a batch.
pub const STREAM: &[(&str, &str, i64)] = &[
    ("assistant_text", "reading the log now", 1003),
    ("tool_use", r#"{"tool":"write"}"#, 1004),
    ("tool_result", "written", 1005),
];
/// A block that arrives from no round-trip: user input, naming no request.
pub const USER_INPUT: (&str, &str, i64) = ("user_input", "read the log", 1000);
/// A block the context carries and the log keeps: an injection, naming no request.
pub const INJECTION: (&str, &str, i64) = ("injection", "the project instructions", 999);
/// A user input that arrived while the model was streaming: appended at the application
/// boundary, so it lands after the blocks it follows.
pub const STEERING: (&str, &str, i64) = ("user_input", "stop and summarise", 1006);

/// A block as the lane writes it: no slot, no state.
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

/// Open the lane's request and commit its blocks — `SESSION`, `REQUEST`, `BLOCKS` —
/// through the open store. A block is committed by the transaction that inserts it, so
/// there is no marker for a caller to close (R-session-35).
pub(crate) fn commit_tool_call(world: &mut World) {
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.open_request(SESSION, REQUEST, Some(PROVIDER_ID), MODEL) {
        report(world, format!("opening the request failed: {e}"));
        return;
    }
    let blocks: Vec<Block<'_>> = BLOCKS
        .iter()
        .map(|(kind, payload, ts)| block(SESSION, Some(REQUEST), kind, payload, *ts))
        .collect();
    if let Err(e) = store.commit_all(&blocks) {
        report(world, format!("the commit failed: {e}"));
    }
}

/// The store's blocks for a session, as `(kind, payload, ts)`, in the store's sequence.
fn logged(store: &Store, session: &str) -> Result<Vec<(String, String, i64)>, Error> {
    store.run(session).map(|rows| {
        rows.into_iter()
            .map(|row| (row.kind, row.payload, row.ts))
            .collect()
    })
}

/// What the lane expects after a tool call: `BLOCKS` as the store answers them.
fn expected_blocks() -> Vec<(String, String, i64)> {
    BLOCKS
        .iter()
        .map(|(kind, payload, ts)| ((*kind).to_string(), (*payload).to_string(), *ts))
        .collect()
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

#[when("a request is opened and a tool call commits its blocks")]
async fn commit_a_tool_call(world: &mut World) {
    commit_tool_call(world);
}

#[then("the blocks are stored in the order they were committed")]
async fn blocks_stored_in_order(world: &mut World) {
    // The store is read while the borrow is held, and its findings are reported
    // after it is dropped: a step cannot report into the world it reads the store from.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match logged(store, SESSION) {
            Ok(rows) if rows == expected_blocks() => {}
            Ok(rows) => findings.push(format!(
                "the store answers {} blocks {rows:?}, expected {:?}",
                rows.len(),
                expected_blocks()
            )),
            Err(e) => findings.push(format!("reading blocks: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
}

#[then("each block carries the time it was given")]
async fn blocks_carry_their_times(world: &mut World) {
    // R-session-28: the store records the event time the block carried, so the read
    // answers the stamps the caller passed, in the order they arrived.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.run(SESSION) {
            Ok(rows)
                if rows
                    .iter()
                    .map(|row| row.ts)
                    .eq(BLOCKS.iter().map(|(_, _, ts)| *ts)) => {}
            Ok(rows) => findings.push(format!(
                "the stored block times are {:?}, expected {:?}",
                rows.iter().map(|row| row.ts).collect::<Vec<_>>(),
                BLOCKS.iter().map(|(_, _, ts)| *ts).collect::<Vec<_>>()
            )),
            Err(e) => findings.push(format!("reading blocks: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
}

#[then("the request row records its model with no usage and no reason")]
async fn request_opened(world: &mut World) {
    // R-session-36: one row per round-trip, with its model, its usage null and no
    // terminal reason while it is open, and no payload of its own.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.latest_request(SESSION) {
            Ok(Some(request)) => {
                if request.request != REQUEST {
                    findings.push(format!(
                        "the newest request is {}, expected {REQUEST}",
                        request.request
                    ));
                }
                if request.model != MODEL {
                    findings.push(format!(
                        "the request's model is {:?}, expected {MODEL:?}",
                        request.model
                    ));
                }
                if request.provider_id.as_deref() != Some(PROVIDER_ID) {
                    findings.push(format!(
                        "the request's provider id is {:?}, expected {PROVIDER_ID:?}",
                        request.provider_id
                    ));
                }
                for (name, count) in [
                    ("usage_in", request.usage_in),
                    ("usage_cache_read", request.usage_cache_read),
                    ("usage_cache_write", request.usage_cache_write),
                    ("usage_out", request.usage_out),
                ] {
                    if count.is_some() {
                        findings.push(format!("an open request carries {name} {count:?}"));
                    }
                }
                if request.reason.is_some() {
                    findings.push(format!(
                        "an open request carries a reason {:?}",
                        request.reason
                    ));
                }
            }
            Ok(None) => findings.push("no request row is stored".to_string()),
            Err(e) => findings.push(format!("reading the request: {e}")),
        },
    }
    // The scenario's last step, so it closes the store and fails the lane: the event time and
    // the open row are only checked if a finding can end the run.
    clean_store(world);
    for finding in findings {
        report(world, finding);
    }
    assert_clean(
        world,
        "the request row records its model with no usage and no reason",
    );
}

#[when("the blocks a stream closed at once are committed")]
async fn commit_a_batch(world: &mut World) {
    // R-session-35: a stream closes several blocks at once, and they land in one
    // transaction.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let blocks: Vec<Block<'_>> = STREAM
        .iter()
        .map(|(kind, payload, ts)| block(SESSION, Some(REQUEST), kind, payload, *ts))
        .collect();
    if let Err(e) = store.commit_all(&blocks) {
        report(world, format!("the batch commit failed: {e}"));
    }
}

#[then("the batch lands as one commit, on adjacent sequences")]
async fn batch_lands_together(world: &mut World) {
    // The batch's rows follow the tool call's rows with nothing between them, so the
    // store's sequence is a single run over one session's blocks (R-session-35).
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.run(SESSION) {
            Ok(rows) => {
                let want = BLOCKS.len() + STREAM.len();
                if rows.len() != want {
                    findings.push(format!("{} blocks are stored, expected {want}", rows.len()));
                }
                let seqs: Vec<i64> = rows.iter().map(|row| row.seq).collect();
                let adjacent = seqs.windows(2).all(|pair| pair[1] == pair[0] + 1);
                if !adjacent {
                    findings.push(format!("the sequences {seqs:?} are not one run"));
                }
                let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
                let want: Vec<&str> = BLOCKS
                    .iter()
                    .chain(STREAM.iter())
                    .map(|(kind, _, _)| *kind)
                    .collect();
                if kinds != want {
                    findings.push(format!("the blocks are {kinds:?}, expected {want:?}"));
                }
            }
            Err(e) => findings.push(format!("reading blocks: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(
        world,
        "the batch lands as one commit, on adjacent sequences",
    );
}

#[when("a request is stamped terminal")]
async fn stamp_terminal(world: &mut World) {
    // R-session-40: the counts and the reason arrive after the last block the request
    // produced, in one write to the mutable row.
    let Some(store) = world.store.as_ref() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.stamp_request(
        SESSION,
        REQUEST,
        Some(USAGE_IN),
        Some(USAGE_CACHE_READ),
        Some(USAGE_CACHE_WRITE),
        Some(USAGE_OUT),
        REASON,
    ) {
        report(world, format!("stamping the request failed: {e}"));
    }
}

#[when("a batch naming a terminal request is committed")]
async fn commit_a_refused_batch(world: &mut World) {
    // The request reached its terminal, so the schema refuses a block naming it
    // (R-session-32): the refusal lands after the first row of this transaction has
    // been written, so the transaction is the unit that fails.
    let batch = [
        block(
            SESSION,
            Some(REQUEST),
            "tool_use",
            "{\"tool\":\"late\"}",
            1007,
        ),
        block(SESSION, None, "user_input", "a clean block", 1008),
    ];
    let committed = match world.store.as_mut() {
        None => {
            no_store(world);
            true
        }
        Some(store) => store.commit_all(&batch).is_ok(),
    };
    if committed {
        report(
            world,
            "a batch naming a terminal request committed".to_string(),
        );
    }
}

#[then("the refused batch stores no block")]
async fn nothing_stored(world: &mut World) {
    // R-session-02: the refused batch left nothing — not its refused row, and not the
    // clean row of the same transaction. The blocks of the commit that succeeded are
    // unchanged.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            match logged(store, SESSION) {
                Ok(rows) if rows == expected_blocks() => {}
                Ok(rows) => findings.push(format!(
                    "{} blocks are stored around a refused batch, expected {:?}",
                    rows.len(),
                    expected_blocks()
                )),
                Err(e) => findings.push(format!("reading blocks: {e}")),
            }
            match store.latest_request(SESSION) {
                Ok(Some(request)) if request.reason.as_deref() == Some(REASON) => {}
                Ok(Some(request)) => findings.push(format!(
                    "the request's reason is {:?}, expected {REASON:?}",
                    request.reason
                )),
                Ok(None) => findings.push("the request row is gone".to_string()),
                Err(e) => findings.push(format!("reading the request: {e}")),
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the refused batch stores no block");
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

#[when("a block for a request with no row is inserted")]
async fn insert_orphan_block(world: &mut World) {
    refused_write(world, "block with no request row", "FOREIGN KEY", |conn| {
        conn.execute(
            "INSERT INTO blocks (session, request, ts, kind, payload) VALUES (?1, 7, 1003, 'tool_use', 'orphan')",
            rusqlite::params![SESSION],
        )
    });
}

#[when("a block naming that request is inserted")]
async fn insert_block_after_terminal(world: &mut World) {
    refused_write(
        world,
        "block for a request past its terminal",
        "a terminal request produces no further block",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload) VALUES (?1, ?2, 1004, 'tool_result', 'late')",
                rusqlite::params![SESSION, REQUEST],
            )
        },
    );
}

#[when("a stored block row is updated")]
async fn update_block_row(world: &mut World) {
    refused_write(
        world,
        "update of a log row",
        "the log is append-only",
        |conn| {
            conn.execute(
                "UPDATE blocks SET payload = 'changed' WHERE session = ?1",
                rusqlite::params![SESSION],
            )
        },
    );
}

#[when("a stored block row is deleted")]
async fn delete_block_row(world: &mut World) {
    refused_write(
        world,
        "delete of a log row",
        "the log is append-only",
        |conn| {
            conn.execute(
                "DELETE FROM blocks WHERE session = ?1",
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

#[given("a store whose block was committed by a process that is killed")]
async fn killed_after_commit(world: &mut World) {
    world.findings.clear();
    kill_after_report(world, "committed", "committed");
}

#[given("a store whose rows were written by a process that is killed")]
async fn killed_mid_transaction(world: &mut World) {
    world.findings.clear();
    // The child names the rows it wrote before it is killed, so a pass means a
    // transaction was open with rows in it, not that the child wrote nothing.
    let token = format!("writing {}", BLOCKS.len());
    kill_after_report(world, "mid", &token);
}

#[then("the committed block is returned in full")]
async fn committed_block_survives(world: &mut World) {
    // R-session-06: the process died after the commit, and the reopened store answers
    // with the whole block, payload and time intact, and with the request row the
    // terminal stamped. That the row arrived inside the transaction, and not after it,
    // is the unit test in tau-core, which fails a batch's last statement.
    let mut findings: Vec<String> = Vec::new();
    let store = match reopen(world) {
        Ok(store) => store,
        Err(e) => {
            report(world, format!("reopening the store: {e}"));
            clean_store(world);
            assert_clean(world, "the committed block is returned in full");
            return;
        }
    };
    match logged(&store, SESSION) {
        Ok(rows) if rows == expected_blocks() => {}
        Ok(rows) => findings.push(format!(
            "the committed blocks are {rows:?}, expected {:?}",
            expected_blocks()
        )),
        Err(e) => findings.push(format!("reading blocks: {e}")),
    }
    match store.latest_request(SESSION) {
        Ok(Some(request)) => {
            if request.reason.as_deref() != Some(REASON) {
                findings.push(format!(
                    "the request's reason is {:?}, expected {REASON:?}",
                    request.reason
                ));
            }
            for (name, got, want) in [
                ("input", request.usage_in, USAGE_IN),
                ("cache read", request.usage_cache_read, USAGE_CACHE_READ),
                ("cache write", request.usage_cache_write, USAGE_CACHE_WRITE),
                ("output", request.usage_out, USAGE_OUT),
            ] {
                if got != Some(want) {
                    findings.push(format!(
                        "the request's {name} count is {got:?}, expected {want}"
                    ));
                }
            }
        }
        Ok(None) => findings.push("the request row is not stored".to_string()),
        Err(e) => findings.push(format!("reading the request: {e}")),
    }
    drop(store);
    for finding in findings {
        report(world, finding);
    }
}

#[then("the log carries no committed marker")]
async fn no_committed_marker(world: &mut World) {
    // R-session-07 as an absence: a block is committed by the transaction that inserts
    // it, so the log has no marker column to read and no unit to close. The step is not
    // vacuous because the step before it read the block this absence is measured on.
    let mut findings: Vec<String> = Vec::new();
    let read = raw_conn(world).and_then(|conn| {
        let columns: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('blocks') WHERE name = 'committed'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let units: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('turn', 'events')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok((columns, units))
    });
    match read {
        Err(e) => findings.push(format!("reading the log's columns: {e}")),
        Ok((columns, _)) if columns != 0 => {
            findings.push("the log carries a committed column".to_string());
        }
        Ok((_, units)) if units != 0 => {
            findings.push("the store still holds the turn and event tables".to_string());
        }
        Ok(_) => {}
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the log carries no committed marker");
}

#[then("the killed transaction leaves no block")]
async fn killed_transaction_leaves_nothing(world: &mut World) {
    // R-session-02's fact under a crash: rows written inside a transaction that never
    // committed are gone after the restart, so the log has no rows and the request has
    // no row.
    let mut findings: Vec<String> = Vec::new();
    let store = match reopen(world) {
        Ok(store) => store,
        Err(e) => {
            report(world, format!("reopening the store: {e}"));
            clean_store(world);
            assert_clean(world, "the killed transaction leaves no block");
            return;
        }
    };
    match store.run(SESSION) {
        Ok(rows) if rows.is_empty() => {}
        Ok(rows) => findings.push(format!(
            "{} blocks are stored by a killed transaction",
            rows.len()
        )),
        Err(e) => findings.push(format!("reading blocks: {e}")),
    }
    match store.latest_request(SESSION) {
        Ok(None) => {}
        Ok(Some(request)) => {
            findings.push(format!(
                "a killed transaction stored request {}",
                request.request
            ));
        }
        Err(e) => findings.push(format!("reading the request: {e}")),
    }
    // The premise row the child claimed is gone too, so the rollback covers the whole
    // transaction the child had open, not only the log's rows.
    match store.created(SESSION) {
        Ok(None) => {}
        Ok(Some(_)) => {
            findings.push("a killed transaction stored the session row".to_string());
        }
        Err(e) => findings.push(format!("reading the creation time: {e}")),
    }
    drop(store);
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the killed transaction leaves no block");
}

#[then("the state tables hold no log content")]
async fn state_tables_hold_no_log(world: &mut World) {
    // R-session-03: block content goes to the log and nowhere else. A copy in a
    // mutable table is the finding, because that copy needs reconciling on the
    // crash day, and the readback proves the commit under test actually ran.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref().map(|store| logged(store, SESSION)) {
        None => findings.push("no store is open".to_string()),
        Some(Err(e)) => findings.push(format!("reading blocks: {e}")),
        Some(Ok(rows)) if rows.len() != BLOCKS.len() => findings.push(format!(
            "{} blocks are stored, expected {}",
            rows.len(),
            BLOCKS.len()
        )),
        Some(Ok(_)) => {}
    }
    // Every table and every column is read, not an agreed list: `sessions` and
    // `requests` gaining a column, or a new table appearing, has to be scanned for log
    // text. Rows are joined on 0x1f, a character JSON payloads cannot contain, so a
    // stored value never splits where a payload does not.
    const MUTABLE: &[&str] = &["requests", "sessions"];
    // shortcut: this is today's schema, and a new mutable table is meant to fire the
    // guard by name. `adopt_observational_memory` lands observations and reflections as
    // blocks in the log, in the `observations` and `reflections` slots, so it names no
    // new mutable table; a change that does add one names it here, and the scan covers
    // it from that line on.
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
                name != "blocks"
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
            for (_kind, payload, _ts) in BLOCKS {
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
    // The locking connection opens the file before the store does, and only the store
    // creates the directory, so the directory is made here.
    let path = store_path("setup-race");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating the store directory: {e}"))?;
    }
    let outcome = run_setup_race(&path, leg, beat);
    if outcome.is_err() {
        // A red path removes its own scratch: the Then step that deletes the directory
        // never runs once the scenario has already failed.
        if let Some(dir) = path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }
    outcome
}

fn run_setup_race(path: &std::path::Path, leg: &str, beat: u64) -> Result<SetupChild, String> {
    let locker = rusqlite::Connection::open(path)
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
    let outcome = race_behind_the_lock(&mut child, &locker, path, leg, beat);
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
    // refused at least once. The beat is not what makes the scenario non-vacuous: the
    // refusals the child reports are. Cutting every beat to 0 turns all three scenarios
    // red — `the second instance met no refusal, so the setup race was never measured`,
    // and on the bounded leg `the second instance did not report a busy store: exit 0;
    // opened refusals=0`.
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
                "SELECT count(*) FROM sqlite_master WHERE name = 'blocks'",
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
    // deferred migration transaction back, so the statement answers busy only after its 100 ms
    // timeout has spent itself, and one beat of 400 ms covers several attempts.
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
    // The retry is bounded, so a lock that outlasts it ends the open instead of parking the
    // instance. This is the journal-upgrade leg, where a standing lock is refused on the spot,
    // so the budget is 20 attempts of a 50 ms backoff, ~1 s, and the 5 s beat leaves ~4 s for
    // the refusal to land before the lock clears. The migration leg is the slow one, ~2.95 s:
    // each attempt burns its 100 ms busy timeout before the backoff.
    // A beat near that budget turns the scenario red on a loaded box, not on R-session-34.
    let outcome = setup_race("wal", 5000);
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

#[when("the log is read between commits")]
async fn read_between_commits(world: &mut World) {
    // The store's reader API, in autocommit: each read ends its own read
    // transaction. The clause's `WHILE` window is the other scenario's: there, the
    // block's own transaction sits open on a second connection while these same reads
    // run.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            if let Err(e) = store.blocks_after(SESSION, 0) {
                findings.push(format!("reading blocks: {e}"));
            }
            if let Err(e) = store.latest_request(SESSION) {
                findings.push(format!("reading the newest request: {e}"));
            }
            if let Err(e) = store.state(SESSION) {
                findings.push(format!("reading the state: {e}"));
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
}

#[when("the log is read while a block commit is in flight")]
async fn read_inside_an_open_commit(world: &mut World) {
    // R-session-04's `WHILE` window, opened: the block's own transaction sits open on a
    // second connection while the store's reader API runs. The transaction closes at the
    // end of this step: a writer in flight blocks a checkpoint on its own, and the
    // closing step's probe measures readers.
    let mut findings: Vec<String> = Vec::new();
    match raw_conn(world) {
        Err(e) => findings.push(format!("opening a writer connection: {e}")),
        Ok(writer) => {
            let open = writer.execute_batch("BEGIN IMMEDIATE").and_then(|_| {
                writer.execute(
                    "INSERT INTO blocks (session, request, ts, kind, payload) \
                     VALUES (?1, ?2, 1003, 'tool_use', 'in flight')",
                    rusqlite::params![SESSION, REQUEST],
                )
            });
            if let Err(e) = open {
                findings.push(format!("opening the block's transaction: {e}"));
            }
            match world.store.as_ref() {
                None => findings.push("no store is open".to_string()),
                Some(store) => {
                    if let Err(e) = store.blocks_after(SESSION, 0) {
                        findings.push(format!("reading blocks: {e}"));
                    }
                    if let Err(e) = store.latest_request(SESSION) {
                        findings.push(format!("reading the newest request: {e}"));
                    }
                    match store.run(SESSION) {
                        Err(e) => findings.push(format!("reading the run: {e}")),
                        // The block in flight is invisible here: its transaction is open
                        // on the other connection, so a read inside the window answers the
                        // committed blocks and nothing else.
                        Ok(rows) => {
                            let kinds: Vec<&str> =
                                rows.iter().map(|row| row.kind.as_str()).collect();
                            let want: Vec<&str> = BLOCKS.iter().map(|(kind, _, _)| *kind).collect();
                            if kinds != want {
                                findings.push(format!(
                                    "a read inside the window answers {kinds:?}, expected {want:?}"
                                ));
                            }
                        }
                    }
                }
            }
            if let Err(e) = writer.execute_batch("COMMIT") {
                findings.push(format!("closing the block's transaction: {e}"));
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
        Some(store) => match store.run(SESSION) {
            Err(e) => findings.push(format!("reading blocks: {e}")),
            // No frames means the checkpoint has nothing to do, and answers
            // `(0, 0)` for reasons unrelated to a free log.
            Ok(rows) if rows.is_empty() => findings.push(
                "the log holds no blocks, so the checkpoint has no frames to copy".to_string(),
            ),
            Ok(_) => {}
        },
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
/// A fork over part of the range, to measure what a pointer outside it becomes.
const FORK_PART: &str = "lane-fork-part";

#[when("the session is forked over its blocks")]
async fn fork_the_session(world: &mut World) {
    // R-session-08: the session's blocks, and the request rows they name, copied into a
    // session of its own.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let last = match store.run(SESSION) {
        Ok(rows) => rows.last().map(|row| row.seq).unwrap_or(0),
        Err(e) => {
            report(world, format!("reading the session's blocks: {e}"));
            return;
        }
    };
    if let Err(e) = store.fork(SESSION, FORK, 1, last) {
        report(world, format!("the fork failed: {e}"));
    }
}

#[then("the fork holds the copied blocks, their request, and names its parent")]
async fn fork_holds_the_copy(world: &mut World) {
    // R-session-08: the copied range reads back the same, the request it names is a row
    // the child can answer, and the new session records the session it came from.
    let mut findings = Vec::new();
    if let Some(store) = world.store.as_ref() {
        match (logged(store, SESSION), logged(store, FORK)) {
            (Ok(want), Ok(got)) => {
                if got.is_empty() {
                    findings.push("the fork holds no blocks".to_string());
                } else if got != want {
                    findings.push(format!("the fork's blocks are {got:?}, expected {want:?}"));
                }
            }
            (Err(e), _) => findings.push(format!("reading the parent's blocks: {e}")),
            (_, Err(e)) => findings.push(format!("reading the fork's blocks: {e}")),
        }
        match store.latest_request(FORK) {
            Ok(Some(request)) => {
                if request.request != REQUEST {
                    findings.push(format!(
                        "the fork's newest request is {}, expected {REQUEST}",
                        request.request
                    ));
                }
                // The copied request carries its terminal, so the child continues on a
                // new request rather than appending to a round-trip that is over.
                if request.reason.as_deref() != Some(REASON) {
                    findings.push(format!(
                        "the fork's request reason is {:?}, expected {REASON:?}",
                        request.reason
                    ));
                }
            }
            Ok(None) => findings.push("the fork holds no request row".to_string()),
            Err(e) => findings.push(format!("reading the fork's request: {e}")),
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
        "the fork holds the copied blocks, their request, and names its parent",
    );
}

#[then("the index answers a search of the stored blocks")]
async fn index_answers_the_search(world: &mut World) {
    // R-session-10: the answer comes from the index, so the query is FTS5 syntax. The
    // first query names a column, which no scan of the payload text can answer; the
    // second asks for a term no block holds, so an index that matched everything is caught.
    let mut findings = Vec::new();
    if let Some(store) = world.store.as_ref() {
        match store.search(r#"kind:"tool use""#) {
            Ok(hits) => {
                let rows: Vec<(&str, &str)> = hits
                    .iter()
                    .map(|(session, _, _, kind, _)| (session.as_str(), kind.as_str()))
                    .collect();
                // The fork's copy arrives by `INSERT … SELECT`, so its index rows prove
                // the trigger fires on the bulk path, not only on a one-row insert.
                if rows != [(SESSION, "tool_use"), (FORK, "tool_use")] {
                    findings.push(format!(
                        "the search answers with {rows:?}, expected [({SESSION:?}, \"tool_use\"), ({FORK:?}, \"tool_use\")]"
                    ));
                }
            }
            Err(e) => findings.push(format!("the search failed: {e}")),
        }
        match store.search("no such term appears here") {
            Ok(hits) if hits.is_empty() => {}
            Ok(hits) => findings.push(format!(
                "a search for a term no block holds answers {} hits",
                hits.len()
            )),
            Err(e) => findings.push(format!("the second search failed: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the index answers a search of the stored blocks");
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

#[when("a request that streamed no block is opened and stamped")]
async fn commit_a_request_with_no_blocks(world: &mut World) {
    // R-session-40: a request that streamed no block is a fact, and its row answers with
    // its counts and its reason. A refusal the model returns is the shape.
    let Some(store) = world.store.as_ref() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.open_request(SESSION, REQUEST + 1, Some("lane-request-2"), MODEL) {
        report(world, format!("opening the second request failed: {e}"));
        return;
    }
    if let Err(e) = store.stamp_request(
        SESSION,
        REQUEST + 1,
        Some(USAGE_IN),
        None,
        None,
        Some(USAGE_OUT),
        "refusal",
    ) {
        report(world, format!("stamping the second request failed: {e}"));
    }
}

#[then("the run answers the session's blocks in sequence")]
async fn run_answers_from_the_store(world: &mut World) {
    // R-session-13: a run is a query over the log, in the store's sequence, carrying the
    // block's own fields and no per-turn state. A request that streamed no block adds no
    // block to the answer: it is a row of `requests`, not a unit the read invents.
    let mut findings = Vec::new();
    if let Some(store) = world.store.as_ref() {
        match logged(store, SESSION) {
            Ok(rows) if rows == expected_blocks() => {}
            Ok(rows) => findings.push(format!(
                "the run answers {rows:?}, expected {:?}",
                expected_blocks()
            )),
            Err(e) => findings.push(format!("reading the run: {e}")),
        }
        match store.latest_request(SESSION) {
            Ok(Some(request)) => {
                if request.request != REQUEST + 1 {
                    findings.push(format!(
                        "the newest request is {}, expected {}",
                        request.request,
                        REQUEST + 1
                    ));
                }
                if request.reason.as_deref() != Some("refusal") {
                    findings.push(format!(
                        "the second request's reason is {:?}, expected \"refusal\"",
                        request.reason
                    ));
                }
            }
            Ok(None) => findings.push("a request that streamed no block left no row".to_string()),
            Err(e) => findings.push(format!("reading the request: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the run answers the session's blocks in sequence");
}

#[then("the export holds one line per block and the store answers the same")]
async fn export_lines_and_store(world: &mut World) {
    // R-session-11: each line carries the store's row, and the store answers the same
    // afterwards. The counter check that proves the export wrote nothing runs in the
    // export step, where the store can be measured on either side of it.
    let mut findings: Vec<String> = Vec::new();
    let lines: Vec<&str> = world.exported.lines().collect();
    if lines.len() != BLOCKS.len() {
        findings.push(format!(
            "the export writes {} lines, expected {}",
            lines.len(),
            BLOCKS.len()
        ));
    }
    for (line, (kind, payload, ts)) in lines.iter().zip(BLOCKS) {
        // The payload is JSON text, so it arrives escaped inside the line's string.
        let escaped = payload.replace('"', "\\\"");
        for want in [
            format!(r#""session":"{SESSION}""#),
            format!(r#""request":{REQUEST}"#),
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
        match logged(store, SESSION) {
            Ok(rows) if rows == expected_blocks() => {}
            Ok(rows) => findings.push(format!(
                "{} blocks are stored after the export, expected {:?}",
                rows.len(),
                expected_blocks()
            )),
            Err(e) => findings.push(format!("reading blocks after the export: {e}")),
        }
    }
    for finding in findings {
        report(world, finding);
    }
}

#[when("a block carrying state pointers is committed")]
async fn commit_a_block_with_state(world: &mut World) {
    // R-session-37: the state a block leaves is recorded on the block's own row, as
    // pointers into `blocks`. The first block is the user input; the second names the
    // range it was built from.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.commit(&block(
        SESSION,
        None,
        USER_INPUT.0,
        USER_INPUT.1,
        USER_INPUT.2,
    )) {
        report(world, format!("the user input block failed: {e}"));
        return;
    }
    let built_from = match store.run(SESSION) {
        Ok(rows) => rows.last().map(|row| (row.seq, row.seq)),
        Err(e) => {
            report(world, format!("reading the session's blocks: {e}"));
            return;
        }
    };
    let (start, end) = built_from.unwrap_or((0, 0));
    let with_state = Block {
        session: SESSION,
        request: None,
        ts: 1009,
        kind: "assistant_text",
        payload: "built from the log",
        slot: None,
        state_start: Some(start),
        state_end: Some(end),
    };
    if let Err(e) = store.commit(&with_state) {
        report(world, format!("the state-carrying block failed: {e}"));
    }
}

#[then("the state answers as pointers into the log")]
async fn state_answers_as_pointers(world: &mut World) {
    // The pointers are the range of block sequences the block was built from: two
    // numbers naming rows of the log, not payload text (R-session-37).
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            let first = store
                .run(SESSION)
                .ok()
                .and_then(|rows| rows.first().map(|row| row.seq))
                .unwrap_or(0);
            match store.state(SESSION) {
                Ok(Some((start, end))) if start == first && end == first => {}
                Ok(Some((start, end))) => findings.push(format!(
                    "the state answers ({start}, {end}), expected ({first}, {first})"
                )),
                Ok(None) => findings.push("no block carries a state".to_string()),
                Err(e) => findings.push(format!("reading the state: {e}")),
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the state answers as pointers into the log");
}

#[when("a user input block and an injection block are committed")]
async fn commit_input_and_injection(world: &mut World) {
    // R-session-38 and R-session-45: neither arrives from a round-trip, so neither names
    // a request, and the injection names the slot it fills.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let input = block(SESSION, None, USER_INPUT.0, USER_INPUT.1, USER_INPUT.2);
    if let Err(e) = store.commit(&input) {
        report(world, format!("the user input block failed: {e}"));
        return;
    }
    let injection = Block {
        session: SESSION,
        request: None,
        ts: INJECTION.2,
        kind: INJECTION.0,
        payload: INJECTION.1,
        slot: Some(SLOT),
        state_start: None,
        state_end: None,
    };
    if let Err(e) = store.commit(&injection) {
        report(world, format!("the injection block failed: {e}"));
    }
}

#[then("the blocks answer their requests, and the two name none")]
async fn blocks_answer_their_requests(world: &mut World) {
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.run(SESSION) {
            Ok(rows) => {
                let named: Vec<(&str, Option<i64>)> = rows
                    .iter()
                    .map(|row| (row.kind.as_str(), row.request))
                    .collect();
                let want: Vec<(&str, Option<i64>)> = vec![
                    (USER_INPUT.0, None),
                    (INJECTION.0, None),
                    ("tool_use", Some(REQUEST)),
                    ("tool_result", Some(REQUEST)),
                ];
                if named != want {
                    findings.push(format!("the blocks name {named:?}, expected {want:?}"));
                }
            }
            Err(e) => findings.push(format!("reading blocks: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(
        world,
        "the blocks answer their requests, and the two name none",
    );
}

#[when("a steering block is committed")]
async fn commit_a_steering_block(world: &mut World) {
    // A user input that arrived while the model was streaming is appended at the
    // application boundary, naming no request, so the queue is the tail of the log.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let steering = block(SESSION, None, STEERING.0, STEERING.1, STEERING.2);
    if let Err(e) = store.commit(&steering) {
        report(world, format!("the steering block failed: {e}"));
    }
}

#[then("the resume reads answer the newest request and the blocks after it")]
async fn resume_reads_answer(world: &mut World) {
    // R-session-39: the newest request row is the anchor, and the read answers the blocks
    // after the last one that request produced — the steering block, and nothing either
    // round-trip streamed. The scenario logs two round-trips, so a read of the oldest one
    // answers this red.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            let newest = match store.latest_request(SESSION) {
                Ok(Some(request)) => {
                    if request.request != REQUEST + 1 {
                        findings.push(format!(
                            "the newest request is {}, expected {}",
                            request.request,
                            REQUEST + 1
                        ));
                    }
                    if request.reason.as_deref() != Some(REASON) {
                        findings.push(format!(
                            "the newest request's reason is {:?}, expected {REASON:?}",
                            request.reason
                        ));
                    }
                    Some(request.request)
                }
                Ok(None) => {
                    findings.push("no request row is stored".to_string());
                    None
                }
                Err(e) => {
                    findings.push(format!("reading the newest request: {e}"));
                    None
                }
            };
            if let Some(request) = newest {
                let anchor = match store.last_block_seq(SESSION, request) {
                    Ok(Some(seq)) => seq,
                    Ok(None) => {
                        findings.push("the newest request streamed no block".to_string());
                        0
                    }
                    Err(e) => {
                        findings.push(format!("reading the request's last block: {e}"));
                        0
                    }
                };
                match store.blocks_after(SESSION, anchor) {
                    Ok(rows) => {
                        let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
                        if kinds != [STEERING.0] {
                            findings.push(format!(
                                "the blocks after the newest request are {kinds:?}, expected [{}]",
                                STEERING.0
                            ));
                        }
                        if rows.iter().any(|row| row.request.is_some()) {
                            findings.push(
                                "a block after the newest request names a request".to_string(),
                            );
                        }
                    }
                    Err(e) => findings.push(format!("reading the blocks after: {e}")),
                }
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
}

#[then("the whole-session read answers every block again")]
async fn whole_session_read_is_not_the_resume_read(world: &mut World) {
    // The mutation that names `run` as no resume read: it answers the blocks the resume
    // pointer already covers as well, materialising every payload (R-session-39).
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            let anchor = store
                .latest_request(SESSION)
                .ok()
                .flatten()
                .and_then(|request| {
                    store
                        .last_block_seq(SESSION, request.request)
                        .ok()
                        .flatten()
                })
                .unwrap_or(0);
            let all = store.run(SESSION).map(|rows| rows.len());
            let after = store.blocks_after(SESSION, anchor).map(|rows| rows.len());
            match (all, after) {
                (Ok(all), Ok(after)) => {
                    if all <= after {
                        findings.push(format!(
                            "the whole-session read answers {all} blocks and the resume read {after}, \
                             so the narrow read is not narrower"
                        ));
                    }
                }
                (Err(e), _) => findings.push(format!("reading the run: {e}")),
                (_, Err(e)) => findings.push(format!("reading the blocks after: {e}")),
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the whole-session read answers every block again");
}

#[then("the newest request answers its counts and its reason")]
async fn terminal_counts_answer(world: &mut World) {
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.latest_request(SESSION) {
            Ok(Some(request)) => {
                if request.usage_in != Some(USAGE_IN) {
                    findings.push(format!(
                        "the request's input count is {:?}, expected {USAGE_IN}",
                        request.usage_in
                    ));
                }
                if request.usage_out != Some(USAGE_OUT) {
                    findings.push(format!(
                        "the request's output count is {:?}, expected {USAGE_OUT}",
                        request.usage_out
                    ));
                }
                if request.reason.as_deref() != Some("refusal") {
                    findings.push(format!(
                        "the request's reason is {:?}, expected \"refusal\"",
                        request.reason
                    ));
                }
                // A request that streamed no block is a row and nothing else: the log
                // answers no block naming it.
                match store.last_block_seq(SESSION, request.request) {
                    Ok(None) => {}
                    Ok(Some(seq)) => {
                        findings.push(format!("a request that streamed no block owns block {seq}"));
                    }
                    Err(e) => findings.push(format!("reading the request's blocks: {e}")),
                }
            }
            Ok(None) => findings.push("no request row is stored".to_string()),
            Err(e) => findings.push(format!("reading the request: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
}

#[then("a count the provider never reported stays null")]
async fn unreported_counts_stay_null(world: &mut World) {
    // R-session-40: one API reports cache counts and another does not, so a null is a
    // fact the store keeps rather than a zero it invents.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.latest_request(SESSION) {
            Ok(Some(request)) => {
                for (name, count) in [
                    ("cache read", request.usage_cache_read),
                    ("cache write", request.usage_cache_write),
                ] {
                    if count.is_some() {
                        findings.push(format!("the {name} count is {count:?}, expected null"));
                    }
                }
            }
            Ok(None) => findings.push("no request row is stored".to_string()),
            Err(e) => findings.push(format!("reading the request: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "a count the provider never reported stays null");
}

#[then("the injection block names the slot it fills")]
async fn injection_names_its_slot(world: &mut World) {
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.run(SESSION) {
            Ok(rows) => {
                let slots: Vec<(&str, Option<&str>)> = rows
                    .iter()
                    .map(|row| (row.kind.as_str(), row.slot.as_deref()))
                    .collect();
                let want = vec![(USER_INPUT.0, None), (INJECTION.0, Some(SLOT))];
                if slots != want {
                    findings.push(format!(
                        "the blocks name slots {slots:?}, expected {want:?}"
                    ));
                }
            }
            Err(e) => findings.push(format!("reading blocks: {e}")),
        },
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the injection block names the slot it fills");
}

#[then("the slot answers the blocks that name it")]
async fn slot_answers_its_blocks(world: &mut World) {
    // R-session-46: the read is by slot, so a resume rebuilds a context section from the
    // log without walking every block.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            match store.blocks_in_slot(SESSION, SLOT) {
                Ok(rows) => {
                    let got: Vec<StoredBlock> = rows
                        .into_iter()
                        .filter(|row| row.kind == INJECTION.0)
                        .collect();
                    if got.len() != 1 {
                        findings.push(format!(
                            "the slot answers {} blocks, expected the one injection",
                            got.len()
                        ));
                    }
                }
                Err(e) => findings.push(format!("reading the slot: {e}")),
            }
            match store.blocks_in_slot(SESSION, "no such slot") {
                Ok(rows) if rows.is_empty() => {}
                Ok(rows) => findings.push(format!(
                    "a slot no block fills answers {} blocks",
                    rows.len()
                )),
                Err(e) => findings.push(format!("reading an empty slot: {e}")),
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
    clean_store(world);
    assert_clean(world, "the slot answers the blocks that name it");
}

// The guards a caller's bug would break: the terminal that lands once, the keys that bind
// every row a commit writes, the state pair a fork re-points, and the round-trip a child
// continues.

#[when("a second terminal is stamped for that request")]
async fn stamp_a_second_terminal(world: &mut World) {
    // R-session-40 stamps a request once, after its last block. A second stamp would rewrite
    // the counts the provider reported, so the store refuses the write.
    let Some(store) = world.store.as_ref() else {
        no_store(world);
        return;
    };
    match store.stamp_request(SESSION, REQUEST, Some(1), None, None, Some(1), "refusal") {
        Ok(()) => report(
            world,
            "a second terminal stamped over the first".to_string(),
        ),
        Err(tau_core::storage::Error::Terminal(_)) => {}
        Err(e) => report(
            world,
            format!("the second stamp failed as {e}, expected the refusal of a stamped request"),
        ),
    }
}

#[then("the first terminal answers its counts and its reason")]
async fn first_terminal_intact(world: &mut World) {
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.latest_request(SESSION) {
            Ok(Some(request)) => {
                if request.reason.as_deref() != Some(REASON) {
                    findings.push(format!(
                        "the reason is {:?}, expected {REASON:?}",
                        request.reason
                    ));
                }
                if request.usage_in != Some(USAGE_IN) {
                    findings.push(format!(
                        "the input count is {:?}, expected {USAGE_IN}",
                        request.usage_in
                    ));
                }
            }
            Ok(None) => findings.push("no request row is stored".to_string()),
            Err(e) => findings.push(format!("reading the newest request: {e}")),
        },
    }
    clean_store(world);
    for finding in findings {
        report(world, finding);
    }
    assert_clean(
        world,
        "the first terminal answers its counts and its reason",
    );
}

#[when("a block naming a session that is not recorded is inserted")]
async fn insert_block_for_no_session(world: &mut World) {
    // The request's key binds a session row, and a block naming no request rides the session's
    // own key: a block for a session the store never claimed is a record no read answers.
    refused_write(
        world,
        "block naming an unrecorded session",
        "FOREIGN KEY",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload) VALUES (?1, NULL, 1010, ?2, ?3)",
                rusqlite::params!["lane-no-session", USER_INPUT.0, USER_INPUT.1],
            )
        },
    );
}

#[when("a block carrying half a state is inserted")]
async fn insert_half_state(world: &mut World) {
    // R-session-37 records the state as pointers, plural: one of a pair names a state with no
    // end, which no read of the context can answer.
    refused_write(
        world,
        "block carrying half a state",
        "CHECK constraint failed",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload, state_start, state_end) \
                 VALUES (?1, NULL, 1010, ?2, ?3, 1, NULL)",
                rusqlite::params![SESSION, USER_INPUT.0, USER_INPUT.1],
            )
        },
    );
}

#[when("a block is committed with a state naming the first block")]
async fn commit_a_block_naming_the_first(world: &mut World) {
    // The pair names the session's first block twice: the state a fork has to re-point into
    // the child (R-session-37).
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let row = Block {
        session: SESSION,
        request: None,
        ts: INJECTION.2,
        kind: INJECTION.0,
        payload: INJECTION.1,
        slot: Some(SLOT),
        state_start: Some(1),
        state_end: Some(1),
    };
    if let Err(e) = store.commit(&row) {
        report(world, format!("committing the state-carrying block: {e}"));
    }
}

#[then("the child's state names the child's copy of that block")]
async fn child_state_names_the_copy(world: &mut World) {
    // R-session-37 and R-session-08: the copy of a state-carrying block points inside the
    // child, at the child's copy of the block the parent's pointer named. A pair that names
    // the parent's sequences is the bug: the child would read its parent's log.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            // The pair names the parent's first block, so the child's pair names the first
            // block of the copy.
            let first = store
                .run(FORK)
                .ok()
                .and_then(|rows| rows.first().map(|row| row.seq));
            match (first, store.state(FORK)) {
                (None, _) => findings.push("the child holds no copy of the state".to_string()),
                (_, Err(e)) => findings.push(format!("reading the child's state: {e}")),
                (Some(_), Ok(None)) => {
                    findings.push("the child carries no state".to_string());
                }
                (Some(copy), Ok(Some((start, end)))) => {
                    if start != copy || end != copy {
                        findings.push(format!(
                            "the child's state is ({start}, {end}), expected ({copy}, {copy})",
                        ));
                    }
                }
            }
        }
    }
    for finding in findings {
        report(world, finding);
    }
}

#[when("the session is forked from its second block")]
async fn fork_from_the_second_block(world: &mut World) {
    // The range starts past the pointer, so the child has no row to name.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let last = match store.run(SESSION) {
        Ok(rows) => rows.last().map(|row| row.seq).unwrap_or(0),
        Err(e) => {
            report(world, format!("reading the session's blocks: {e}"));
            return;
        }
    };
    if let Err(e) = store.fork(SESSION, FORK_PART, 2, last) {
        report(world, format!("the fork failed: {e}"));
    }
}

#[then("the second child carries no state")]
async fn second_child_carries_no_state(world: &mut World) {
    // The pointer names a block the range does not copy, so the child records no pair: the
    // alternative is a child whose pointer names a row of its parent's log.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.state(FORK_PART) {
            Ok(None) => {}
            Ok(Some((start, end))) => {
                findings.push(format!(
                    "the child's state is ({start}, {end}) over a range that copied no such block"
                ));
            }
            Err(e) => findings.push(format!("reading the child's state: {e}")),
        },
    }
    // The scenario's last step, so it is the one that closes the store and fails the lane:
    // a step that only reports leaves the scenario unable to fail.
    clean_store(world);
    for finding in findings {
        report(world, finding);
    }
    assert_clean(world, "the second child carries no state");
}

#[when("the child opens a request and commits a block")]
async fn child_opens_a_request(world: &mut World) {
    // The copied request keeps its terminal, so the child continues on a request of its own:
    // a block naming the copied terminal is the write R-session-32 refuses.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.open_request(FORK, REQUEST + 1, Some("lane-request-2"), MODEL) {
        report(world, format!("opening the child's request: {e}"));
        return;
    }
    let row = block(
        FORK,
        Some(REQUEST + 1),
        USER_INPUT.0,
        "the child continues",
        1010,
    );
    if let Err(e) = store.commit(&row) {
        report(world, format!("the child's block was refused: {e}"));
    }
}

#[then("the child holds the copied blocks and its own")]
async fn child_holds_copy_and_own(world: &mut World) {
    // The copy is intact and the child's block is appended after it: no copied sequence is
    // reused, and the parent keeps the blocks the fork copied.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => {
            let parent = store.run(SESSION);
            let child = store.run(FORK);
            match (parent, child) {
                (Ok(parent), Ok(child)) => {
                    let mut seqs: Vec<i64> = child.iter().map(|row| row.seq).collect();
                    seqs.sort_unstable();
                    if seqs.len() != parent.len() + 1 {
                        findings.push(format!(
                            "the child holds {} blocks over the parent's {}, expected one more",
                            seqs.len(),
                            parent.len()
                        ));
                    }
                    if seqs.iter().collect::<std::collections::HashSet<_>>().len() != seqs.len() {
                        findings.push(format!("the child's sequences repeat: {seqs:?}"));
                    }
                    if let Some(last) = seqs.last() {
                        let child_kinds: Vec<&str> =
                            child.iter().map(|row| row.kind.as_str()).collect();
                        if child_kinds.last() != Some(&USER_INPUT.0) {
                            findings.push(format!("the child's last block is {child_kinds:?}"));
                        }
                        if *last <= parent.last().map(|row| row.seq).unwrap_or(0) {
                            findings.push(format!(
                                "the child's block at {last} sits inside the parent's range"
                            ));
                        }
                    }
                }
                (Err(e), _) => findings.push(format!("reading the parent's blocks: {e}")),
                (_, Err(e)) => findings.push(format!("reading the child's blocks: {e}")),
            }
        }
    }
    clean_store(world);
    for finding in findings {
        report(world, finding);
    }
    assert_clean(world, "the child holds the copied blocks and its own");
}

#[given("a store whose blocks committed and whose terminal was not stamped")]
async fn killed_after_blocks(world: &mut World) {
    // The blocks are in the log and the terminal is not: the window R-session-40's second
    // write opens, and the process is killed with it open.
    world.findings.clear();
    kill_after_report(world, "blocks", "blocks");
}

#[then("the blocks answer and the request carries no reason")]
async fn blocks_answer_and_request_is_open(world: &mut World) {
    // The reopen reads the log the kill left: every block of the round-trip, and a request
    // with no reason and no counts. A further block naming it lands, because the round-trip
    // is open, not finished, so the resume has the blocks to continue from.
    let mut findings: Vec<String> = Vec::new();
    match reopen(world) {
        Err(e) => findings.push(format!("reopening the store: {e}")),
        Ok(mut store) => {
            match logged(&store, SESSION) {
                Ok(rows) => {
                    let want = expected_blocks();
                    if rows != want {
                        findings.push(format!("the log holds {rows:?}, expected {want:?}"));
                    }
                }
                Err(e) => findings.push(format!("reading the blocks: {e}")),
            }
            match store.latest_request(SESSION) {
                Ok(Some(request)) => {
                    if request.reason.is_some() {
                        findings.push(format!(
                            "the request carries a reason {:?} after a kill before its terminal",
                            request.reason
                        ));
                    }
                    if request.usage_in.is_some() {
                        findings.push("the request carries counts it never stamped".to_string());
                    }
                }
                Ok(None) => findings.push("the killed round-trip left no request row".to_string()),
                Err(e) => findings.push(format!("reading the request: {e}")),
            }
            let row = block(
                SESSION,
                Some(REQUEST),
                USER_INPUT.0,
                "the round-trip continues",
                1011,
            );
            if let Err(e) = store.commit(&row) {
                findings.push(format!("a block naming the open request was refused: {e}"));
            }
        }
    }
    clean_store(world);
    for finding in findings {
        report(world, finding);
    }
    assert_clean(world, "the blocks answer and the request carries no reason");
}

#[when("a second request is opened, commits a block and is stamped terminal")]
async fn commit_a_second_request(world: &mut World) {
    // A second round-trip, so the resume's read has an older request to get wrong: the row it
    // names is the newest, and an oldest-first read answers this red (R-session-39).
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    if let Err(e) = store.open_request(SESSION, REQUEST + 1, Some("lane-request-2"), MODEL) {
        report(world, format!("opening the second request: {e}"));
        return;
    }
    let row = block(
        SESSION,
        Some(REQUEST + 1),
        "assistant_text",
        "the second answer",
        1012,
    );
    if let Err(e) = store.commit(&row) {
        report(world, format!("committing the second request's block: {e}"));
        return;
    }
    if let Err(e) = store.stamp_request(
        SESSION,
        REQUEST + 1,
        Some(USAGE_IN),
        None,
        None,
        Some(USAGE_OUT),
        REASON,
    ) {
        report(world, format!("stamping the second request: {e}"));
    }
}

#[when("a block whose state runs backwards is inserted")]
async fn insert_backwards_state(world: &mut World) {
    // R-session-37: the pair is the state a block leaves, so it runs from a block to a later
    // one. Backwards is a pair no context can rebuild.
    refused_write(
        world,
        "block whose state runs backwards",
        "CHECK constraint failed",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload, state_start, state_end) \
                 VALUES (?1, NULL, 1011, ?2, ?3, 2, 1)",
                rusqlite::params![SESSION, USER_INPUT.0, USER_INPUT.1],
            )
        },
    );
}

#[when("a block whose state names no block is inserted")]
async fn insert_state_naming_nothing(world: &mut World) {
    // R-session-37: the pointers are pointers into `blocks`, so a sequence the session does
    // not have is not a state. The read of a pair naming nothing answers a context that is
    // not in the log.
    refused_write(
        world,
        "block whose state names no block",
        "a state pointer names no block",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload, state_start, state_end) \
                 VALUES (?1, NULL, 1011, ?2, ?3, 9999, 9999)",
                rusqlite::params![SESSION, USER_INPUT.0, USER_INPUT.1],
            )
        },
    );
}

#[when("a block whose state ends on no block is inserted")]
async fn insert_state_end_naming_nothing(world: &mut World) {
    // R-session-37: the arm that checks the pair's end. A state whose start names a block and
    // whose end names nothing rebuilds no context, and the read answers a range into midair.
    refused_write(
        world,
        "block whose state ends on no block",
        "a state pointer names no block",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload, state_start, state_end) \
                 VALUES (?1, NULL, 1012, ?2, ?3, 1, 9999)",
                rusqlite::params![SESSION, USER_INPUT.0, USER_INPUT.1],
            )
        },
    );
}

#[when("a block whose state has an end and no beginning is inserted")]
async fn insert_state_without_start(world: &mut World) {
    // R-session-37: the arm that checks the pair is stored whole. `state()` filters on the
    // start, so a pair with only an end is a state no read answers.
    refused_write(
        world,
        "block whose state has an end and no beginning",
        "CHECK constraint failed",
        |conn| {
            conn.execute(
                "INSERT INTO blocks (session, request, ts, kind, payload, state_start, state_end) \
                 VALUES (?1, NULL, 1013, ?2, ?3, NULL, 5)",
                rusqlite::params![SESSION, USER_INPUT.0, USER_INPUT.1],
            )
        },
    );
}

#[when("a block is committed with a state naming the second block")]
async fn commit_a_block_naming_the_second(world: &mut World) {
    // The second pair the session carries: (1, 2) names the first block and the second, both
    // written by now, so it is legal and lands after the first pair.
    let Some(store) = world.store.as_mut() else {
        no_store(world);
        return;
    };
    let row = Block {
        session: SESSION,
        request: None,
        ts: INJECTION.2 + 1,
        kind: INJECTION.0,
        payload: INJECTION.1,
        slot: Some(SLOT),
        state_start: Some(1),
        state_end: Some(2),
    };
    if let Err(e) = store.commit(&row) {
        report(
            world,
            format!("committing the second state-carrying block: {e}"),
        );
    }
}

#[then("the session's state names the pair the second block left")]
async fn state_answers_the_newest_pair(world: &mut World) {
    // R-session-37: two state-carrying blocks, one session. The newest is the state the next
    // request is built from, so the read answers (1, 2); the oldest is a state already spent.
    let mut findings: Vec<String> = Vec::new();
    match world.store.as_ref() {
        None => findings.push("no store is open".to_string()),
        Some(store) => match store.state(SESSION) {
            Ok(Some((start, end))) => {
                if (start, end) != (1, 2) {
                    findings.push(format!(
                        "the session's state is ({start}, {end}), expected (1, 2)"
                    ));
                }
            }
            Ok(None) => findings.push("the session carries no state".to_string()),
            Err(e) => findings.push(format!("reading the session's state: {e}")),
        },
    }
    clean_store(world);
    for finding in findings {
        report(world, finding);
    }
    assert_clean(
        world,
        "the session's state names the pair the second block left",
    );
}
