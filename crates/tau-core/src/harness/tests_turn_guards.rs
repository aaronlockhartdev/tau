use super::*;

use crate::harness::testkit::*;
use crate::provider::TurnResult;
use std::sync::atomic::AtomicBool;
use tau_protocol::EntryKind;

/// #267: the settlement guarantee is a `Drop` guard, so a `run_turn` task
/// that panics mid-turn leaves `live.turn` cleared — the session is not
/// "running" forever. The provider panics on its first call and serves a
/// plain body after: the second turn runs normally, proving the guard
/// settled the flag without wedging the session.
struct PanicOnceProvider {
    panicked: AtomicBool,
}

impl TurnProvider for PanicOnceProvider {
    fn call<'a>(
        &self,
        _req: &ResponseRequest,
        sink: &'a mut dyn TurnSink,
    ) -> super::ProviderTurn<'a> {
        let first = !self.panicked.swap(true, Ordering::SeqCst);
        let body = if first {
            // A body whose events never matter: the panic rides out of the
            // provider call, not the stream.
            String::new()
        } else {
            plain_body()
        };
        Box::pin(async move {
            first.then(|| panic!("injected turn panic (#267)"));
            let (events, calls, ending) = provider::decode_stream(&body).unwrap();
            let mut result = TurnResult {
                outcome: ending,
                ..Default::default()
            };
            for event in &events {
                if !sink.event(event.clone()) {
                    result.outcome = provider::StreamOutcome::Killed;
                    break;
                }
                provider::fold_event(event, &mut result);
            }
            result.calls = calls.into_iter().map(|(_, c)| c).collect();
            Ok(result)
        })
    }
}

#[tokio::test]
async fn a_panicking_turn_settles_the_turn_flag() {
    let inner: TurnProviderRef = Arc::new(PanicOnceProvider {
        panicked: AtomicBool::new(false),
    });
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(&core, &workspace, inner, TurnConfig::default());
    let session_id = live.meta.lock().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    // The panic is caught at the spawned task's `JoinHandle`; the flag must
    // clear by the guard's `Drop` on the unwinding task. Poll for the settle.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while core
        .sessions
        .lock()
        .get(&session_id)
        .is_some_and(|l| l.turn.load(Ordering::SeqCst))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !core
            .sessions
            .lock()
            .get(&session_id)
            .unwrap()
            .turn
            .load(Ordering::SeqCst),
        "the guard settled the flag after the panic"
    );
    // The session is not wedged: a follow-up send starts and settles a turn.
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "again".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while core
        .sessions
        .lock()
        .get(&session_id)
        .is_some_and(|l| l.turn.load(Ordering::SeqCst))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !core
            .sessions
            .lock()
            .get(&session_id)
            .unwrap()
            .turn
            .load(Ordering::SeqCst)
    );
    drop(core);
}

/// The archive's live-turn guard on the other header-writers: a rename of
/// a running session is refused (its read-modify-write header rewrite
/// would clobber entries the in-flight turn appends in the window), the
/// refusal mutates nothing, and the rename lands once the turn settles.
#[tokio::test]
async fn a_rename_of_a_running_session_is_refused() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().id.clone();
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
/// (`set_leaf` would rewrite the header mid-turn and desync the writer's
/// in-memory leaf), and it lands on the settled session.
#[tokio::test]
async fn a_branch_of_a_running_session_is_refused() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().id.clone();
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

/// The conversation entries a `canned_body()` turn records: user, the
/// assistant with its one bash call, and the tool result. `diagnostic`
/// records are excluded: these guards watch for fabricated *turns*, and a
/// diagnostic record is bookkeeping, not a turn (ADR-0034).
fn entry_count(cwd: &Path, session: &str) -> usize {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind != EntryKind::Diagnostic)
        .count()
}

fn diagnostic_count(cwd: &Path, session: &str) -> usize {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Diagnostic)
        .count()
}

/// Bounded wait for a session's turn flag to clear (past the deadline the
/// caller's assert fails — a hung turn is a test failure, not a wait).
async fn wait_settled(core: &Arc<Core>, session: &str) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while core
        .sessions
        .lock()
        .get(session)
        .is_some_and(|l| l.turn.load(Ordering::SeqCst))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The #267 kill, seeded the way the dead turn left it: an assistant entry
/// with a call and no tool result, appended through a scratch store on the
/// session file.
fn append_dangling_assistant(cwd: &Path, session: &str) {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    let payload = tau_protocol::payload::AssistantPayload {
        text: String::new(),
        reasoning: String::new(),
        interrupted: true,
        stop_reason: Some(tau_protocol::StreamOutcome::Killed),
        usage: None,
        duration_secs: None,
        calls: vec![tau_protocol::payload::FunctionCall {
            id: "fc_9".into(),
            call_id: "call_9".into(),
            name: "edit".into(),
            arguments: "{}".into(),
        }],
    };
    store
        .append(EntryKind::Assistant, payload.to_value(), None)
        .unwrap();
}

/// #267: a session whose last entry is an assistant with a call that never
/// got its tool result (the turn died mid-execution) is healed at re-open:
/// the missing result is minted as an `error` entry, and the session
/// accepts a new message afterwards.
#[tokio::test]
async fn a_reopened_session_repairs_dangling_tool_calls() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    wait_settled(&core, &session_id).await;
    let healthy = entry_count(Path::new(&workspace.cwd), &session_id);
    assert_eq!(
        healthy, 3,
        "the canned turn records user + assistant + tool"
    );
    // The kill: the turn wrote the assistant's call and died before the
    // tool result landed.
    append_dangling_assistant(Path::new(&workspace.cwd), &session_id);
    // Close, then re-open: the repair mints the missing tool result.
    core.dispatch(Command::SessionClose {
        session: session_id.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionOpen {
        session: session_id.clone(),
    })
    .unwrap();
    let mut recheck = SessionStore::for_workspace(Path::new(&workspace.cwd), &session_id);
    recheck.open().unwrap();
    let entries = recheck.entries_range(0, usize::MAX).unwrap();
    let repaired = entries.iter().find(|e| {
        e.kind == EntryKind::Tool && e.payload.get("call_id").is_some_and(|c| c == "call_9")
    });
    let repaired = repaired.expect("the repair minted the dangling call's tool entry");
    assert_eq!(
        repaired
            .payload
            .get("status")
            .and_then(serde_json::Value::as_str),
        Some("error"),
        "the minted entry carries the error status"
    );
    // After the repair the session accepts a new message: the dispatch
    // starts a turn, which settles.
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "again".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    wait_settled(&core, &session_id).await;
    assert!(
        !core
            .sessions
            .lock()
            .get(&session_id)
            .unwrap()
            .turn
            .load(Ordering::SeqCst)
    );
    drop(core);
}

/// #267 negative: a healthy file (its last entry is the tool result)
/// re-opens byte-identical — the repair mints nothing.
#[tokio::test]
async fn a_healthy_reopen_mints_nothing() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    wait_settled(&core, &session_id).await;
    let healthy = entry_count(Path::new(&workspace.cwd), &session_id);
    assert_eq!(
        diagnostic_count(Path::new(&workspace.cwd), &session_id),
        0,
        "a session that ran with no model record mints nothing mid-run"
    );
    core.dispatch(Command::SessionClose {
        session: session_id.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionOpen {
        session: session_id.clone(),
    })
    .unwrap();
    assert_eq!(
        entry_count(Path::new(&workspace.cwd), &session_id),
        healthy,
        "the healthy file gains no conversation entry"
    );
    assert_eq!(
        diagnostic_count(Path::new(&workspace.cwd), &session_id),
        1,
        "the reopen records the model it opens on; its facts are declared, so the gate is armed (#338)"
    );
    drop(core);
}
