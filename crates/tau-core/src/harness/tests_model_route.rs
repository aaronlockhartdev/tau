//! #336's record leg: a model switch on a **closed** session is routed before it
//! is written, and the meta a restore or an archive reports names the model the
//! record carries. A closed session has no live provider to refuse a send, so the
//! record is the only thing that can reject an id whose prefix names no provider
//! — and the record is exactly what the next open routes from
//! (`sessions.rs::open_route`), which is why an unchecked record path is the bug:
//! the session becomes unopenable with no routing error surfaced.

use super::*;

use crate::harness::testkit::*;

/// A config whose providers answer apart: `aaa` replays the text script (no
/// reasoning), `zzz` the reasoning script, and the default is `aaa` — so a
/// session opens on `aaa`, and only a route that follows the model id's prefix
/// can make the reasoning script answer. The `canned://` base urls are the
/// dev-gated scripted provider (debug builds, which is the test profile).
const CONFIG: &str = r#"
[providers.aaa]
base_url = "canned://text"

[providers.aaa.models.text-model]

[providers.zzz]
base_url = "canned://reasoning"

[providers.zzz.models.reasoning-model]

[generation]
default_model = "aaa/text-model"
"#;

/// A core over `CONFIG` — written to the system dir so every workspace's config
/// carries both providers — with a workspace open at `project`.
async fn core_with_pair(system: &Path, project: &Path) -> (Arc<Core>, Workspace) {
    std::fs::write(system.join("config.toml"), CONFIG).unwrap();
    let core = CoreBuilder::default_system()
        .unwrap()
        .with_system_dir(system.to_path_buf())
        .with_home(system.to_path_buf())
        .build()
        .unwrap();
    let workspace = open_ws(&core, project).await;
    (core, workspace)
}

/// A session opened in the workspace, as its id.
fn new_session(core: &Arc<Core>, workspace: &Workspace) -> String {
    match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session.id,
        other => panic!("expected a session: {other:?}"),
    }
}

/// The model a session's snapshot carries.
fn session_model(core: &Arc<Core>, id: &str) -> Option<String> {
    match core
        .dispatch(Command::SessionOpen { session: id.into() })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot.session.model,
        other => panic!("expected snapshot: {other:?}"),
    }
}

/// A live session's records, in append order.
fn facts_in(cwd: &Path, id: &str) -> Vec<Diagnostic> {
    std::fs::read_to_string(cwd.join(".tau/sessions").join(format!("{id}.jsonl")))
        .unwrap()
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v.get("type")?.as_str()? == "diagnostic")
                .then(|| v.get("payload").and_then(Diagnostic::from_value))
                .flatten()
        })
        .collect()
}

/// The session's assistant entries as (text, reasoning), in record order.
fn assistant_turns(cwd: &Path, id: &str) -> Vec<(String, String)> {
    let mut store = SessionStore::for_workspace(cwd, id);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Assistant)
        .map(|e| {
            (
                e.payload["text"].as_str().unwrap_or("").to_owned(),
                e.payload["reasoning"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

/// One send, settled on its own: the turn flag is the core's, and the wait is
/// bounded so a hung turn is a failure, not a hang.
async fn send_and_settle(core: &Arc<Core>, id: &str, text: &str) {
    core.dispatch(Command::MessageSend {
        session: id.into(),
        text: text.into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let running = core
            .sessions
            .lock()
            .get(id)
            .is_some_and(|l| l.turn.load(Ordering::SeqCst));
        if !running {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn never settled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The session's meta as one archive and one restore report it.
fn archived_model(core: &Arc<Core>, session: &str) -> SessionMeta {
    match core
        .dispatch(Command::SessionArchive {
            session: session.into(),
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    }
}

/// The positive: a switch written to a **closed** session's record routes by the
/// id's prefix, and the next open answers to that provider. The record is what
/// `open_route` reads, so the reasoning script answering after the reopen is the
/// proof the closed-session write named `zzz`: the config's default is `aaa`,
/// whose script emits no reasoning.
#[tokio::test]
async fn a_closed_session_records_the_provider_the_model_id_names() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, workspace) = core_with_pair(system.path(), project.path()).await;
    let id = new_session(&core, &workspace);
    assert_eq!(
        session_model(&core, &id).as_deref(),
        Some("aaa/text-model"),
        "the session opens on the config default"
    );
    core.dispatch(Command::SessionClose {
        session: id.clone(),
    })
    .unwrap();

    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "zzz/reasoning-model".into(),
    })
    .unwrap();
    assert_eq!(
        facts_in(project.path(), &id),
        vec![Diagnostic::ModelChanged {
            from: None,
            to: "zzz/reasoning-model".into(),
        }],
        "the closed-session switch is recorded on the branch"
    );

    core.dispatch(Command::SessionOpen {
        session: id.clone(),
    })
    .unwrap();
    assert_eq!(
        session_model(&core, &id).as_deref(),
        Some("zzz/reasoning-model"),
        "the reopen carries the recorded model"
    );
    send_and_settle(&core, &id, "after").await;
    let turns = assistant_turns(project.path(), &id);
    assert_eq!(turns.len(), 1, "one send, one assistant entry");
    assert!(
        turns[0].1.starts_with("thought 0"),
        "the reopen addressed the recorded model's own provider: {:?}",
        turns[0]
    );
}

/// The negative: a closed-session switch to an id whose prefix names no
/// configured provider is refused. The live arm refuses it; the record path is
/// the only gate left once the session is closed. Unchecked, the id was appended
/// to the record and the next open routed from it, failed `open_route`, and fell
/// back to a read-only snapshot — the session unopenable, no routing error
/// reaching the caller.
#[tokio::test]
async fn a_closed_session_refuses_a_model_whose_prefix_names_no_provider() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, workspace) = core_with_pair(system.path(), project.path()).await;
    let id = new_session(&core, &workspace);
    core.dispatch(Command::SessionClose {
        session: id.clone(),
    })
    .unwrap();

    let err = core
        .dispatch(Command::SessionSetModel {
            session: id.clone(),
            model: "nosuch/x".into(),
        })
        .expect_err("no provider is named `nosuch`");
    assert!(
        matches!(&err, ProtocolError::Other { message }
            if message.contains("no configured provider") && message.contains("nosuch")),
        "the refusal names the prefix it cannot route: {err}"
    );
    // The refusal moves nothing: the record stays as it was, with no model on
    // the branch at all.
    assert_eq!(
        facts_in(project.path(), &id),
        Vec::<Diagnostic>::new(),
        "a refused closed-session switch leaves no `model_changed`"
    );

    // The session is still openable, on the route it had.
    core.dispatch(Command::SessionOpen {
        session: id.clone(),
    })
    .unwrap();
    assert_eq!(
        session_model(&core, &id).as_deref(),
        Some("aaa/text-model"),
        "the refusal left the open's route alone"
    );
    send_and_settle(&core, &id, "still").await;
    let turns = assistant_turns(project.path(), &id);
    assert_eq!(turns.len(), 1, "one send, one assistant entry");
    assert!(
        turns[0].0.starts_with("word 0") && turns[0].1.is_empty(),
        "the session still answers to its own provider: {:?}",
        turns[0]
    );
}

/// The meta half, positive: an archive and a restore report the model the record
/// carries, so the response meta and the list converge on the model the next open
/// routes to.
#[tokio::test]
async fn archive_and_restore_report_the_recorded_model() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, workspace) = core_with_pair(system.path(), project.path()).await;
    let id = new_session(&core, &workspace);
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "zzz/reasoning-model".into(),
    })
    .unwrap();
    core.dispatch(Command::SessionClose {
        session: id.clone(),
    })
    .unwrap();

    let meta = archived_model(&core, &id);
    assert!(meta.archived, "the flag rides the response meta");
    assert_eq!(
        meta.model.as_deref(),
        Some("zzz/reasoning-model"),
        "the archive's meta names the model the record carries"
    );

    let meta = match core
        .dispatch(Command::SessionRestore {
            workspace: workspace.id.clone(),
            session: id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(!meta.archived, "the flag clears with the file");
    assert_eq!(
        meta.model.as_deref(),
        Some("zzz/reasoning-model"),
        "the restore's meta names the model the record carries"
    );
}

/// The meta half, negative: a session whose record names no model reports
/// `model: None`. The config default is not this session's model, and reporting
/// it would dress a session the record does not address in a model chip.
#[tokio::test]
async fn a_session_with_no_model_record_reports_no_model() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, workspace) = core_with_pair(system.path(), project.path()).await;
    // A session file with no entries, never opened: no record of a model.
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &id);
        store.create().unwrap();
    }

    let meta = archived_model(&core, &id);
    assert!(meta.archived, "the flag rides the response meta");
    assert_eq!(
        meta.model, None,
        "no model record, no model in the meta — not the config default"
    );

    let meta = match core
        .dispatch(Command::SessionRestore {
            workspace: workspace.id.clone(),
            session: id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert_eq!(meta.model, None, "the restore guesses no model");
}
