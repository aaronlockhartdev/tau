use super::*;

use crate::harness::testkit::*;

/// The archive's live-turn guard on the other header-writers: a rename of
/// a running session is refused (its read-modify-write header rewrite
/// would clobber entries the in-flight turn appends in the window), the
/// refusal mutates nothing, and the rename lands once the turn settles.
#[tokio::test]
async fn a_rename_of_a_running_session_is_refused() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let err = core
        .dispatch(Command::SessionRename {
            session: session_id.clone(),
            title: "New Name".into(),
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("is running"),
        "the refusal names the state: {msg}"
    );
    // The refusal mutated nothing: the header carries no new title.
    let path = Path::new(&workspace.cwd)
        .join(".tau")
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let header = std::fs::read_to_string(&path).unwrap();
    assert!(!header.lines().next().unwrap().contains("New Name"));
    // Let the turn settle, then the same rename lands.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while core
        .sessions
        .lock()
        .unwrap()
        .get(&session_id)
        .is_some_and(|l| l.turn.load(Ordering::SeqCst))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    core.dispatch(Command::SessionRename {
        session: session_id.clone(),
        title: "New Name".into(),
    })
    .unwrap();
    let header = std::fs::read_to_string(&path).unwrap();
    assert!(
        header.lines().next().unwrap().contains("New Name"),
        "the settled rename reaches the header"
    );
    drop(core);
}

/// Same guard for a branch move: refused while the session is running
/// (set_leaf would rewrite the header mid-turn and desync the writer's
/// in-memory leaf), and it lands on the settled session.
#[tokio::test]
async fn a_branch_of_a_running_session_is_refused() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // A real entry to branch to (the turn's first user entry).
    let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &session_id);
    store.open().unwrap();
    let first = store
        .entries_range(0, 1)
        .unwrap()
        .into_iter()
        .next()
        .expect("the in-flight turn recorded its user entry")
        .id;
    let err = core
        .dispatch(Command::SessionBranch {
            session: session_id.clone(),
            at: first.clone(),
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("is running"),
        "the refusal names the state: {msg}"
    );
    // The refusal moved no leaf: the header's leaf is untouched.
    let path = Path::new(&workspace.cwd)
        .join(".tau")
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let header = std::fs::read_to_string(&path).unwrap();
    assert!(!header.lines().next().unwrap().contains("\"leaf\""));
    // Let the turn settle, then the same branch move lands.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while core
        .sessions
        .lock()
        .unwrap()
        .get(&session_id)
        .is_some_and(|l| l.turn.load(Ordering::SeqCst))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    core.dispatch(Command::SessionBranch {
        session: session_id,
        at: first.clone(),
    })
    .unwrap();
    let mut recheck = SessionStore::for_workspace(Path::new(&workspace.cwd), store.id());
    recheck.open().unwrap();
    assert_eq!(recheck.leaf().unwrap().unwrap().id, first);
    drop(core);
}
