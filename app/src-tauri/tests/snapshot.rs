//! The 10k-entry performance bar (spec §8): the snapshot is a metadata
//! skeleton — no payloads, < 2 MB — and the paged reads it points at stay
//! consistent with it.

use std::collections::BTreeMap;

use serde_json::json;
use tau_app::core::CoreBuilder;
use tau_core::config::Provider;
use tau_core::session::SessionStore;
use tau_protocol::snapshot::EntryRange;
use tau_protocol::{Command, CommandOutput};

#[tokio::test]
async fn a_10k_session_snapshots_below_2mb_with_zero_payloads() {
    let tmp = tempfile::tempdir().unwrap();
    let mut providers = BTreeMap::new();
    providers.insert(
        "dev".into(),
        Provider {
            base_url: "http://127.0.0.1:9/v1".into(),
            key_env: String::new(),
            models: vec!["m".into()],
        },
    );
    let core = CoreBuilder::custom(providers).build();

    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .await
        .unwrap()
    {
        CommandOutput::Workspace(w) => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id,
            title: None,
        })
        .await
        .unwrap()
    {
        CommandOutput::Session(m) => m,
        other => panic!("expected a session: {other:?}"),
    };

    // The 10k-entry fixture (the large_session.rs pattern): chained ~200 B
    // entries written straight into the session file.
    let mut store = SessionStore::for_workspace(tmp.path(), &session.id);
    store.open().unwrap();
    let mut prev: Option<String> = None;
    for i in 0..10_000u32 {
        let text = format!("entry {i:05} ") + &"p".repeat(150);
        let e = store
            .append("message", json!({ "text": text }), prev.as_deref())
            .unwrap();
        prev = Some(e.id);
    }

    let snapshot = match core
        .dispatch(Command::SessionOpen {
            session: session.id.clone(),
        })
        .await
        .unwrap()
    {
        CommandOutput::Snapshot(s) => s,
        other => panic!("expected a snapshot: {other:?}"),
    };
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    eprintln!(
        "10k fixture: {} KB snapshot, {} entry metadata rows",
        bytes.len() / 1024,
        snapshot.entries.len()
    );
    assert!(
        bytes.len() < 2 * 1024 * 1024,
        "snapshot is {} B, over the 2 MB bar",
        bytes.len()
    );
    assert_eq!(snapshot.entries.len(), 10_000);
    // No payloads in the snapshot: the only full-payload piece is the OM
    // log, which is null until the OM integration (#22) populates it.
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("\"payload\""),
        "the snapshot carries an entry payload"
    );
    assert!(snapshot.om.is_null(), "no OM log in v0 pre-#22");

    // The cursor is the leaf: an entries-since read at it returns nothing.
    let entries = match core
        .dispatch(Command::SessionEntries {
            session: session.id.clone(),
            since: Some(snapshot.cursor),
            range: None,
        })
        .await
        .unwrap()
    {
        CommandOutput::Entries(e) => e,
        other => panic!("expected entries: {other:?}"),
    };
    assert!(entries.is_empty(), "nothing follows the leaf cursor");

    // A paged range read returns exactly the requested window, with
    // payloads (the page is what the GUI renders).
    let entries = match core
        .dispatch(Command::SessionEntries {
            session: session.id,
            since: None,
            range: Some(EntryRange {
                start: 9_900,
                count: 100,
            }),
        })
        .await
        .unwrap()
    {
        CommandOutput::Entries(e) => e,
        other => panic!("expected entries: {other:?}"),
    };
    assert_eq!(entries.len(), 100);
    assert!(!entries[0].payload.get("text").is_none());
}
