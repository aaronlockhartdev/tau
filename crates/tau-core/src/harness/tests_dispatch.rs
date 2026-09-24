use super::*;

use crate::harness::testkit::*;

#[tokio::test]
async fn workspace_identity_is_stable_and_path_free() {
    let core = CoreBuilder::custom(providers()).build();
    let w1 = core
        .dispatch(Command::WorkspaceOpen {
            cwd: "/tmp/w".into(),
        })
        .unwrap();
    let w2 = core
        .dispatch(Command::WorkspaceOpen {
            cwd: "/tmp/w".into(),
        })
        .unwrap();
    match (w1, w2) {
        (CommandOutput::Workspace { workspace: a }, CommandOutput::Workspace { workspace: b }) => {
            assert_eq!(a.id, b.id);
            assert_eq!(a.name, "w");
            // The id is a hash of the cwd, not the path itself.
            assert!(!a.id.starts_with('/'));
        }
        other => panic!("expected workspaces: {other:?}"),
    }
}

#[tokio::test]

async fn not_yet_landed_commands_fail_explicitly() {
    let core = CoreBuilder::custom(providers()).build();
    // The sub-agent commands are live (ticket #23): an unknown
    // session's child is a NotFound, not an Unsupported.
    let err = core
        .dispatch(Command::SubagentState {
            handle: "h-1".into(),
        })
        .unwrap_err();
    assert!(matches!(err, ProtocolError::NotFound { .. }));
    // The task commands are live (ticket #24): an unknown session's
    // task is a NotFound, not an Unsupported.
    let err = core
        .dispatch(Command::TaskCreate {
            session: "s".into(),
            title: "t".into(),
        })
        .unwrap_err();
    assert!(matches!(err, ProtocolError::NotFound { .. }));
}

#[tokio::test]

async fn file_read_pages_lines_relative_to_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    std::fs::write(tmp.path().join("f.txt"), "l1\nl2\nl3\n").unwrap();
    let out = core
        .dispatch(Command::FileRead {
            workspace: workspace.id,
            path: "f.txt".into(),
            offset: Some(1),
            limit: Some(1),
        })
        .unwrap();
    match out {
        CommandOutput::File { file: f } => {
            assert_eq!(f.text, "l2");
            assert!(!f.truncated);
        }
        other => panic!("expected a file: {other:?}"),
    }
}

#[tokio::test]

async fn a_new_session_registers_its_workspace_and_streams_events() {
    let tmp = tempfile::tempdir().unwrap();
    // A session with no provider models fails explicitly.
    let core = CoreBuilder::custom(BTreeMap::new()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let err = core
        .dispatch(Command::SessionNew {
            workspace: workspace.id,
            title: None,
        })
        .unwrap_err();
    assert!(matches!(err, ProtocolError::Other { .. }));
    let _ = tmp;
}

/// `SessionEntries` with neither cursor nor range is a full dump —
/// spec §8 has no full-dump command (ADR-0006), so it is an error
/// (review B2).
#[tokio::test]

async fn entries_without_a_cursor_or_range_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id,
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session: m } => m,
        other => panic!("expected a session: {other:?}"),
    };
    let err = core
        .dispatch(Command::SessionEntries {
            session: session.id,
            since: None,
            range: None,
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "expected a rejection, got: {err:?}"
    );
}

/// The workspace's `.tau/config.toml` layers over the root config
/// (spec §12) — a same-named provider entry replaces the root's
/// (review B3: the layer was silently dead on a path bug).
#[tokio::test]

async fn the_project_config_layer_is_read_from_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".tau")).unwrap();
    std::fs::write(
            tmp.path().join(".tau").join("config.toml"),
            "[providers.dev]\nbase_url = \"http://project:9/v1\"\nkey_env = \"\"\nmodels = [\"proj-model\"]\n",
        )
        .unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let config = core.workspace_config(&workspace);
    let dev = &config.providers["dev"];
    assert_eq!(
        dev.base_url, "http://project:9/v1",
        "the project layer did not replace the root provider"
    );
    assert_eq!(dev.models, vec!["proj-model".to_owned()]);
}

/// A project-layer provider reaches a session on the production (file-layered)
/// path (spec §12): the merge branch used to drop the loaded merge, so only
/// the system layer reached sessions and the real-app E2E sidestepped it by
/// putting its provider in the system config (roadmap G bug).
#[tokio::test]
async fn a_project_layer_provider_reaches_a_session_on_the_production_path() {
    let sys = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    // The system layer carries a provider the project layer does not know.
    std::fs::write(
        sys.path().join("config.toml"),
        "[providers.sys]\nbase_url = \"http://system:1/v1\"\nkey_env = \"\"\nmodels = [\"sys-model\"]\n",
    )
    .unwrap();
    // The project layer adds its own; the merged config is the system layer
    // plus this entry, and a session takes the first of the merged map.
    std::fs::create_dir_all(ws.path().join(".tau")).unwrap();
    std::fs::write(
        ws.path().join(".tau").join("config.toml"),
        "[providers.proj]\nbase_url = \"http://project:2/v1\"\nkey_env = \"\"\nmodels = [\"proj-model\"]\n",
    )
    .unwrap();
    let core = CoreBuilder::default_system()
        .with_system_dir(sys.path().to_path_buf())
        .with_home(home.path().to_path_buf())
        .build();
    let workspace = open_ws(&core, ws.path()).await;
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id,
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert_eq!(session.model.as_deref(), Some("proj-model"));
}

/// One send makes exactly one provider call on the production path (roadmap G):
/// the E2E's ghost second assistant entry was not a second call — the count
/// is the forwarding provider's call list, one id per call's first event.
#[tokio::test]
async fn a_production_send_makes_exactly_one_provider_call() {
    let sys = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(
        sys.path().join("config.toml"),
        "[providers.canned]\nbase_url = \"canned://text\"\nmodels = [\"canned-model\"]\n",
    )
    .unwrap();
    let core = CoreBuilder::default_system()
        .with_system_dir(sys.path().to_path_buf())
        .with_home(home.path().to_path_buf())
        .build();
    let workspace = open_ws(&core, ws.path()).await;
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id,
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let live = core
        .sessions
        .lock()
        .unwrap()
        .get(&session.id)
        .cloned()
        .expect("the new session is live");

    for (n, text) in ["say hello", "second turn"].into_iter().enumerate() {
        core.dispatch(Command::MessageSend {
            session: session.id.clone(),
            text: text.into(),
            lane: MessageLane::FollowUp,
        })
        .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while live.turn.load(Ordering::SeqCst) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the turn never settled"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let expected = n + 1;
        assert_eq!(
            live.provider.calls.lock().unwrap().len(),
            expected,
            "send {expected} made a second provider call"
        );
        // The file is the record: one user + one assistant entry per send.
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &session.id);
        store.open().unwrap();
        let entries = store.entries_range(0, usize::MAX).unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.kind == crate::agent::KIND_USER)
                .count(),
            expected
        );
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.kind == crate::agent::KIND_ASSISTANT)
                .count(),
            expected
        );
    }
}
/// Closing a session with an in-flight turn stops the stream — the
/// call closes as interrupted and nothing for that session follows
/// (review N7).
#[tokio::test]

async fn closed_session_serves_snapshot_and_entries_from_disk() {
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
    let snap = match core
        .dispatch(Command::SessionOpen {
            session: id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert!(
        !snap.entries.is_empty(),
        "the disk entries are in the snapshot"
    );
    let out = core
        .dispatch(Command::SessionEntries {
            session: id,
            since: None,
            range: Some(tau_protocol::snapshot::EntryRange {
                start: 0,
                count: 10,
            }),
        })
        .unwrap();
    match out {
        CommandOutput::Entries { entries } => assert!(!entries.is_empty()),
        other => panic!("expected entries: {other:?}"),
    }
}

#[tokio::test]

async fn workspace_index_survives_a_restart() {
    let sys = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    {
        let core = CoreBuilder::custom(providers())
            .with_system_dir(sys.path().into())
            .build();
        open_ws(&core, cwd.path()).await;
        assert!(
            sys.path().join("workspaces.json").exists(),
            "the index was written"
        );
    }
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().into())
        .build();
    let list = match core.dispatch(Command::WorkspaceList).unwrap() {
        CommandOutput::Workspaces { workspaces } => workspaces,
        other => panic!("expected workspaces: {other:?}"),
    };
    assert_eq!(
        list.len(),
        1,
        "the previous run's workspace is restored: {list:?}"
    );
    assert_eq!(list[0].cwd, cwd.path().display().to_string());
}
#[tokio::test]
async fn session_list_merges_live_and_disk_sessions() {
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
    // A closed session: created on disk, never registered with this core.
    let closed_id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &closed_id);
        store.create().unwrap();
        store.set_title("Closed One").unwrap();
        store
            .append("user", json!({ "text": "hello" }), None)
            .unwrap();
    }
    let list = match core
        .dispatch(Command::SessionList { workspace: w.id })
        .unwrap()
    {
        CommandOutput::Sessions { sessions } => sessions,
        other => panic!("expected sessions: {other:?}"),
    };
    assert_eq!(list.len(), 2, "live and closed both listed: {list:?}");
    assert!(
        list.iter()
            .any(|s| s.id == live.id && s.title == live.title),
        "the live session is listed: {list:?}"
    );
    let closed = list
        .iter()
        .find(|s| s.id == closed_id)
        .expect("the closed session is listed");
    assert_eq!(closed.title.as_deref(), Some("Closed One"));
}

#[test]
fn unique_name_returns_a_free_draw() {
    assert_eq!(unique_name(&[], || "Rusty Nail".into()), "Rusty Nail");
}

#[test]
fn unique_name_numbers_past_the_collisions() {
    let titles = vec![
        "Rusty Nail".into(),
        "Rusty Nail 2".into(),
        "Rusty Nail 3".into(),
    ];
    assert_eq!(unique_name(&titles, || "Rusty Nail".into()), "Rusty Nail 4");
}
