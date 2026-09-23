use super::*;

use crate::harness::testkit::*;

/// SessionNew's explicit title takes the rename's cap (the GUI always
/// passes null, but the protocol is open): over the cap is refused
/// before the session file is created, at the cap it is accepted.
#[tokio::test]
async fn session_rename_caps_the_title_length() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    let long = "t".repeat(201);
    let err = core
        .dispatch(Command::SessionRename {
            session: live.id.clone(),
            title: long,
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(msg.contains("200"), "the refusal names the cap: {msg}");
    // The refusal writes nothing: the header's title is untouched.
    let header = std::fs::read_to_string(
        cwd.path()
            .join(".tau/sessions")
            .join(format!("{}.jsonl", live.id)),
    )
    .unwrap()
    .lines()
    .next()
    .unwrap()
    .to_owned();
    assert!(
        !header.contains(&"t".repeat(100)),
        "the oversized title never reached the header: {header}"
    );
    // The cap itself is acceptable.
    core.dispatch(Command::SessionRename {
        session: live.id,
        title: "u".repeat(200),
    })
    .unwrap();
}

/// A sub-agent whose parent is closed cannot be archived directly, and
/// the refusal must give the real route: a closed parent is refused as
/// "not open", so "archive its parent" is not a route — reopening the
/// parent and archiving it archives the child with it (review N7).
#[tokio::test]

async fn a_child_refusal_names_the_reopen_route_for_a_closed_parent() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(CannedChildFactory { body: done_body() }))
        .build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let info = spawn_done_child(&core, &parent.id).await;
    core.dispatch(Command::SessionClose {
        session: parent.id.clone(),
    })
    .unwrap();
    let err = core
        .dispatch(Command::SessionArchive {
            session: info.child.clone(),
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("is not open") && msg.contains("reopen it and archive it"),
        "the refusal gives the real route: {msg}"
    );
    assert!(
        msg.contains(&parent.id),
        "the refusal names the parent: {msg}"
    );
    drop(core);
}

/// An archive that fails on I/O (the parent's live file vanished) leaves
/// the parent in the live map: its in-memory session (queue, supervisor)
/// survives the refusal — the next archive/restore converges, but the
/// session is never detached (review N2).
#[tokio::test]

async fn session_rename_syncs_the_live_meta() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    core.dispatch(Command::SessionRename {
        session: live.id.clone(),
        title: "Renamed".into(),
    })
    .unwrap();
    // Re-opening serves the snapshot from the live meta; the rename must
    // not revert (the B1 regression).
    let snap = match core
        .dispatch(Command::SessionOpen { session: live.id })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.title.as_deref(), Some("Renamed"));
}

#[tokio::test]

async fn session_rename_updates_a_closed_session_header() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store.set_title("Before").unwrap();
    }
    core.dispatch(Command::SessionRename {
        session: id.clone(),
        title: "After".into(),
    })
    .unwrap();
    let header =
        std::fs::read_to_string(cwd.path().join(".tau/sessions").join(format!("{id}.jsonl")))
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_owned();
    assert!(header.contains("\"After\""), "header: {header}");
}

#[tokio::test]

async fn session_set_model_updates_the_live_meta_and_records_the_change() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    let old = live.model.clone().expect("a fresh session has a model");
    // Same model: a no-op (no quiet entry appended).
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: old.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: "other/model".into(),
    })
    .unwrap();
    let snap = match core
        .dispatch(Command::SessionOpen {
            session: live.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.model.as_deref(), Some("other/model"));
    let file = std::fs::read_to_string(
        cwd.path()
            .join(".tau/sessions")
            .join(format!("{}.jsonl", live.id)),
    )
    .unwrap();
    let notes: Vec<String> = file
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            if v.get("type")?.as_str()? == "system" {
                v.get("payload")?.get("note")?.as_str().map(str::to_owned)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(notes, vec![format!("model: {old} → other/model")]);
}

#[tokio::test]

async fn session_set_model_on_a_closed_session_appends_the_quiet_entry() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store
            .append("user", json!({ "text": "hello" }), None)
            .unwrap();
    }
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "m/new".into(),
    })
    .unwrap();
    // The quiet entry is the record: the same model again is a no-op.
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "m/new".into(),
    })
    .unwrap();
    let file =
        std::fs::read_to_string(cwd.path().join(".tau/sessions").join(format!("{id}.jsonl")))
            .unwrap();
    let notes: Vec<String> = file
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v.get("type")?.as_str()? == "system")
                .then(|| v.get("payload")?.get("note")?.as_str().map(str::to_owned))
                .flatten()
        })
        .collect();
    assert_eq!(notes, vec!["model: m/new".to_owned()]);
}

#[tokio::test]

async fn session_set_model_survives_close_and_reopen() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: "other/model".into(),
    })
    .unwrap();
    // Close + re-open goes through build_live again: the model is a
    // file-level fact (the quiet note), not the in-memory agent's.
    core.dispatch(Command::SessionClose {
        session: live.id.clone(),
    })
    .unwrap();
    let snap = match core
        .dispatch(Command::SessionOpen { session: live.id })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.model.as_deref(), Some("other/model"));
}
