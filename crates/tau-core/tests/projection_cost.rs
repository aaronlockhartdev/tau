//! The snapshot projection's cost and its cache (#316): the view's skeleton,
//! usage and badge state are read from what the projection projects, never
//! from a deep copy of every record's payload.
//!
//! The fixture is the stress session's shape: 10k chained user records in one
//! session file.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use tau_core::config::Provider;
use tau_core::harness::{Core, CoreBuilder};
use tau_core::session::SessionStore;
use tau_protocol::snapshot::EntryMeta;
use tau_protocol::snapshot::SessionMeta;
use tau_protocol::{Command, CommandOutput, EntryKind};
use tempfile::tempdir;

const ENTRIES: usize = 10_000;

/// A provider that never answers: enough for a session to be live, never
/// used by a snapshot read.
fn dead_providers() -> BTreeMap<String, Provider> {
    let mut m = BTreeMap::new();
    m.insert(
        "dev".into(),
        Provider::with_model("http://127.0.0.1:9/v1", "model"),
    );
    m
}

fn create(dir: &std::path::Path, title: &str) -> SessionMeta {
    let core = CoreBuilder::custom(dead_providers())
        .build()
        .expect("a core");
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: dir.to_string_lossy().into_owned(),
        })
        .expect("workspace opens")
    {
        CommandOutput::Workspace { workspace } => workspace.id,
        other => panic!("expected a workspace: {other:?}"),
    };
    match core
        .dispatch(Command::SessionNew {
            workspace,
            title: Some(title.into()),
            base_prompt: None,
        })
        .expect("session creates")
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    }
}

/// A record appended through a second store handle — the fixture writer's
/// path, the one that grows a live session's file out from under its log.
fn append(dir: &std::path::Path, id: &str, kind: EntryKind, payload: Value) {
    let mut store = SessionStore::for_workspace(dir, id);
    store.open().expect("the file opens");
    let prev = store.leaf().ok().flatten().map(|e| e.id);
    store
        .append(kind, payload, prev.as_deref())
        .expect("an entry appends");
}

/// Opens `session` on a core over `dir` and hands back the snapshot's rows.
fn rows(core: &Arc<Core>, session: &str) -> Arc<[EntryMeta]> {
    match core
        .dispatch(Command::SessionSnapshot {
            session: session.into(),
        })
        .expect("a snapshot")
    {
        CommandOutput::Snapshot { snapshot } => snapshot.entries,
        other => panic!("expected a snapshot: {other:?}"),
    }
}

fn open(dir: &std::path::Path, session: &str) -> Arc<Core> {
    let core = CoreBuilder::custom(dead_providers())
        .build()
        .expect("a core");
    match core
        .dispatch(Command::WorkspaceOpen {
            cwd: dir.to_string_lossy().into_owned(),
        })
        .expect("workspace opens")
    {
        CommandOutput::Workspace { .. } => {}
        other => panic!("expected a workspace: {other:?}"),
    }
    core.dispatch(Command::SessionOpen {
        session: session.into(),
    })
    .expect("session opens");
    core
}

#[tokio::test]
async fn a_snapshot_projection_on_a_10k_session_is_bounded() {
    let dir = tempdir().expect("a temp dir");
    let root = dir.path().to_path_buf();
    let session = create(&root, "s1");
    let sid = session.id.clone();

    let mut store = SessionStore::for_workspace(&root, &sid);
    store.open().expect("the session opens");
    let mut prev = store.leaf().ok().flatten().map(|e| e.id);
    for n in 0..ENTRIES {
        let entry = store
            .append(
                EntryKind::User,
                json!({"role": "user", "text": format!("entry {n:05}")}),
                prev.as_deref(),
            )
            .expect("an entry appends");
        prev = Some(entry.id);
    }
    drop(store);

    // The load is paid at SessionOpen; by the first poll the store has the
    // whole log in memory and the skeleton is built on that load.
    let core = open(&root, &sid);
    let start = std::time::Instant::now();
    let first = rows(&core, &sid);
    let first_poll = start.elapsed();

    let mut polls = Vec::new();
    for _ in 0..20 {
        let start = std::time::Instant::now();
        let in_view = rows(&core, &sid).len();
        polls.push(start.elapsed());
        assert!(
            in_view >= ENTRIES,
            "the skeleton carries the fixture's records, saw {in_view}"
        );
    }
    let total: std::time::Duration = polls.iter().sum();
    let per_poll = total / 20;
    eprintln!(
        "10000-entry session: first snapshot {:?} ({} rows), 20 polls {} ms ({:.1} ms per snapshot)",
        first_poll,
        first.len(),
        total.as_millis(),
        per_poll.as_secs_f64() * 1000.0
    );

    // The bar: a poll is bounded by the metadata it projects, not by the
    // payloads it discards. The pre-#316 poll deep-cloned every record's
    // payload: 10.8 ms median over 20 polls of this fixture, measured on main
    // e70db1c in this same harness (debug build). 8 ms is the regression bar;
    // the cached skeleton answers in microseconds.
    assert!(
        per_poll.as_millis() < 8,
        "a 10k-session snapshot poll must stay under 8 ms, took {per_poll:?}"
    );
    // The session's load — 10k lines read and CRC-checked at open — is pinned
    // by the stress leg, which opens this fixture's shape through the app.
}

#[tokio::test]
async fn a_snapshot_reuses_its_rows_and_extends_them_for_new_records() {
    // #316: the skeleton is built once per record, not once per poll. Two
    // polls of an unchanged log hand out the same rows; a poll after a new
    // record carries that record, so the cache extends and never freezes.
    let dir = tempdir().expect("a temp dir");
    let root = dir.path().to_path_buf();
    let session = create(&root, "s1");
    let sid = session.id.clone();
    // A non-empty log: an empty slice's Arc is a shared dangling pointer, and
    // that would make the sharing check below vacuous.
    append(
        &root,
        &sid,
        EntryKind::User,
        json!({"role": "user", "text": "the first record"}),
    );
    let core = open(&root, &sid);

    let first = rows(&core, &sid);
    let base = first.len();
    assert!(base >= 1, "the log has records to share");
    let second = rows(&core, &sid);
    assert!(
        Arc::ptr_eq(&first, &second),
        "an unchanged log shares its rows; a poll copies nothing"
    );

    // An external writer appends: the log grows out from under the live log.
    append(
        &root,
        &sid,
        EntryKind::User,
        json!({"role": "user", "text": "written while open"}),
    );

    let third = rows(&core, &sid);
    assert_eq!(third.len(), base + 1, "the new record is in the skeleton");
    assert!(!Arc::ptr_eq(&second, &third), "new rows are new rows");
    assert_eq!(third[third.len() - 1].preview, "written while open");
}

#[tokio::test]
async fn a_snapshots_usage_is_the_last_assistant_records() {
    // #316: the usage lookup reads the record it is about. The full-log scan
    // took the last record that carried a usage — here a later system record,
    // not the model's answer.
    let dir = tempdir().expect("a temp dir");
    let root = dir.path().to_path_buf();
    let session = create(&root, "s1");
    let sid = session.id.clone();

    append(
        &root,
        &sid,
        EntryKind::Assistant,
        json!({
            "text": "a reply", "reasoning": "", "interrupted": false,
            "usage": {"input_tokens": 7, "output_tokens": 3}
        }),
    );
    append(
        &root,
        &sid,
        EntryKind::System,
        json!({"usage": {"input_tokens": 99, "output_tokens": 99}}),
    );

    let core = open(&root, &sid);
    match core
        .dispatch(Command::SessionSnapshot {
            session: sid.clone(),
        })
        .expect("a snapshot")
    {
        CommandOutput::Snapshot { snapshot } => {
            let usage = snapshot
                .session
                .usage
                .expect("the assistant record's usage");
            assert_eq!(usage.input_tokens, 7);
            assert_eq!(usage.output_tokens, 3);
            assert_eq!(usage.total_tokens, 10);
        }
        other => panic!("expected a snapshot: {other:?}"),
    }
}

#[tokio::test]
async fn a_session_with_no_assistant_record_reports_no_usage() {
    // The negative half: a skeleton with no assistant record answers "no
    // usage", not the usage of some record that happens to carry one.
    let dir = tempdir().expect("a temp dir");
    let root = dir.path().to_path_buf();
    let session = create(&root, "s1");
    let sid = session.id.clone();
    append(
        &root,
        &sid,
        EntryKind::System,
        json!({"usage": {"input_tokens": 99, "output_tokens": 99}}),
    );

    let core = open(&root, &sid);
    match core
        .dispatch(Command::SessionSnapshot { session: sid })
        .expect("a snapshot")
    {
        CommandOutput::Snapshot { snapshot } => assert!(snapshot.session.usage.is_none()),
        other => panic!("expected a snapshot: {other:?}"),
    }
}

#[tokio::test]
async fn a_rewritten_session_file_rebuilds_the_skeleton_from_its_own_records() {
    // The skeleton's rows are only a function of the log's prefix *in one
    // load*: a writer that rewrites the file under a live session makes the
    // store re-open it, and the rows from the previous file must not ride into
    // the next one. The positive half is that the rows are the new file's.
    let dir = tempdir().expect("a temp dir");
    let root = dir.path().to_path_buf();
    let session = create(&root, "s1");
    let sid = session.id.clone();

    append(&root, &sid, EntryKind::User, json!({"text": "old 0"}));
    append(&root, &sid, EntryKind::User, json!({"text": "old 1"}));
    let core = open(&root, &sid);
    let previews = |core: &Arc<Core>| {
        rows(core, &sid)
            .iter()
            .map(|meta| meta.preview.clone())
            .collect::<Vec<_>>()
    };
    let first = previews(&core);
    assert!(
        first.iter().any(|p| p == "old 0") && first.iter().any(|p| p == "old 1"),
        "the first poll shows the records it read: {first:?}"
    );

    // A wholesale rewrite under a live session: same session id (the store
    // checks the header), longer than the original so the store's freshness
    // check sees it as growth.
    let other = tempdir().expect("a second temp dir");
    let mut replacement = SessionStore::for_workspace(other.path(), &sid);
    replacement.create().expect("a file to write into");
    for n in 0..7 {
        replacement
            .append(EntryKind::User, json!({"text": format!("new {n}")}), None)
            .expect("an entry appends");
    }
    drop(replacement);
    let source = other
        .path()
        .join(".tau/sessions")
        .join(format!("{sid}.jsonl"));
    let target = root.join(".tau/sessions").join(format!("{sid}.jsonl"));
    std::fs::copy(&source, &target).expect("the file is rewritten");

    let after = previews(&core);
    assert!(
        after.iter().any(|p| p == "new 6"),
        "the rewrite's records are in the view: {after:?}"
    );
    assert!(
        !after.iter().any(|p| p.starts_with("old ")),
        "no row survives the file it was read from: {after:?}"
    );
}
