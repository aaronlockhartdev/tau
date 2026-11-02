use super::*;
use serde_json::json;

use crate::harness::testkit::*;

/// A session file's records, in append order.
fn facts_in(file: &str) -> Vec<Diagnostic> {
    file.lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v.get("type")?.as_str()? == "diagnostic")
                .then(|| v.get("payload").and_then(Diagnostic::from_value))
                .flatten()
        })
        .collect()
}

fn session_file(cwd: &Path, id: &str) -> String {
    std::fs::read_to_string(cwd.join(".tau/sessions").join(format!("{id}.jsonl"))).unwrap()
}

/// `SessionNew`'s explicit title takes the rename's cap (the GUI always
/// passes null, but the protocol is open): over the cap is refused
/// before the session file is created, at the cap it is accepted.
#[tokio::test]
async fn session_rename_caps_the_title_length() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
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
        .build()
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
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
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    let out = core
        .dispatch(Command::SessionRename {
            session: live.id.clone(),
            title: "Renamed".into(),
        })
        .unwrap();
    // Output authority (#346): the command answers the meta it wrote, so the
    // GUI applies the core's value instead of patching its own copy.
    match out {
        CommandOutput::Session { session } => {
            assert_eq!(session.id, live.id);
            assert_eq!(session.title.as_deref(), Some("Renamed"));
        }
        other => panic!("expected session: {other:?}"),
    }
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
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store.set_title("Before").unwrap();
    }
    let out = core
        .dispatch(Command::SessionRename {
            session: id.clone(),
            title: "After".into(),
        })
        .unwrap();
    // The closed arm answers the meta read back from the file it rewrote.
    match out {
        CommandOutput::Session { session } => {
            assert_eq!(session.id, id);
            assert_eq!(session.title.as_deref(), Some("After"));
        }
        other => panic!("expected session: {other:?}"),
    }
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
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    let old = live.model.clone().expect("a fresh session has a model");
    // Same model: a no-op (no quiet entry appended) that still answers the meta.
    let noop = core
        .dispatch(Command::SessionSetModel {
            session: live.id.clone(),
            model: old.clone(),
        })
        .unwrap();
    match noop {
        CommandOutput::Session { session } => {
            assert_eq!(session.id, live.id);
            assert_eq!(session.model.as_deref(), Some(old.as_str()));
        }
        other => panic!("expected session: {other:?}"),
    }
    let changed = core
        .dispatch(Command::SessionSetModel {
            session: live.id.clone(),
            model: "undeclared".into(),
        })
        .unwrap();
    // Output authority (#346): the switch answers the meta it wrote, so the
    // GUI's model chip reads the core's value, not its own guess.
    match changed {
        CommandOutput::Session { session } => {
            assert_eq!(session.model.as_deref(), Some("undeclared"));
        }
        other => panic!("expected session: {other:?}"),
    }
    let snap = match core
        .dispatch(Command::SessionOpen {
            session: live.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.model.as_deref(), Some("undeclared"));
    let facts = facts_in(&session_file(cwd.path(), &live.id));
    assert_eq!(
        facts,
        vec![
            Diagnostic::ModelChanged {
                from: Some(old),
                to: "undeclared".into(),
            },
            Diagnostic::GateUnavailable {
                model: "undeclared".into(),
                // The cap has a default (#338), so the window is the fact that
                // is missing.
                missing: vec!["context_window".into()],
            },
        ],
        "the change, then the gate its facts cannot compute"
    );
}

#[tokio::test]

async fn session_set_model_on_a_closed_session_appends_the_quiet_entry() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store
            .append(EntryKind::User, json!({ "text": "hello" }), None)
            .unwrap();
    }
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "dev/model".into(),
    })
    .unwrap();
    let second = core
        .dispatch(Command::SessionSetModel {
            session: id.clone(),
            model: "dev/model".into(),
        })
        .unwrap();
    // The closed arm answers the meta with the model the record now names, on
    // the no-op second call as on the first (#346).
    match second {
        CommandOutput::Session { session } => {
            assert_eq!(session.id, id);
            assert_eq!(session.model.as_deref(), Some("dev/model"));
        }
        other => panic!("expected session: {other:?}"),
    }
    // The quiet entry is the record: the same model again is a no-op.
    let facts = facts_in(&session_file(cwd.path(), &id));
    assert_eq!(
        facts,
        vec![Diagnostic::ModelChanged {
            from: None,
            to: "dev/model".into(),
        }],
        "one record, naming no predecessor"
    );
}

#[tokio::test]
async fn a_session_written_before_the_records_gains_one_baseline_at_open() {
    // The migration path (ADR-0034): an old file is not re-interpreted. The
    // first open states the model it runs and the gate its facts leave out;
    // every later open reads those records and states nothing new.
    // The ungated model entry is the pre-#338 config shape — a model def with
    // no window — which a file load now refuses (#338); a hand-built provider
    // map still carries it, the shape an old session file was written under.
    let mut provider = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "model");
    provider
        .models
        .insert("model".into(), crate::config::ModelDef::default());
    let core = CoreBuilder::custom(BTreeMap::from([("dev".to_owned(), provider)]))
        .build()
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store
            .append(EntryKind::User, json!({ "text": "a turn" }), None)
            .unwrap();
    }
    for _ in 0..2 {
        core.dispatch(Command::SessionOpen {
            session: id.clone(),
        })
        .unwrap();
        core.dispatch(Command::SessionClose {
            session: id.clone(),
        })
        .unwrap();
    }
    assert_eq!(
        facts_in(&session_file(cwd.path(), &id)),
        vec![
            Diagnostic::ModelChanged {
                from: None,
                to: "model".into(),
            },
            Diagnostic::GateUnavailable {
                model: "model".into(),
                missing: vec!["context_window".into()],
            },
        ],
        "two opens, one pair of records"
    );
}
