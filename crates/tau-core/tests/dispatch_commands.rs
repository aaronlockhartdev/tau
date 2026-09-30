//! The sub-agent and task dispatch arms, driven over the public
//! `Core::dispatch` boundary the way the Tauri app drives it: a scripted
//! child factory behind `CoreBuilder::with_child_factory`, real sessions,
//! and only protocol types on the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tau_core::config::Provider;
use tau_core::harness::{Core, CoreBuilder};
use tau_core::provider;
use tau_core::subagent::ChildProviderFactory;
use tau_protocol::{Command, CommandOutput, ContextMode, MessageLane};

struct CannedChild {
    body: String,
    slow_ms: u64,
}

impl ChildProviderFactory for CannedChild {
    fn create(&self, _child_session_id: &str) -> provider::TurnProviderRef {
        if self.slow_ms > 0 {
            provider::canned_slow(&self.body, self.slow_ms)
        } else {
            provider::canned(&self.body)
        }
    }
}

fn dead_providers() -> BTreeMap<String, Provider> {
    let mut m = BTreeMap::new();
    m.insert(
        "dev".into(),
        Provider::with_model("http://127.0.0.1:9/v1", "model"),
    );
    m
}

/// One scripted `parent_notify` done: the child ends its turn.
fn done_body() -> String {
    let args = json!({
        "text": "the work is done",
        "done": true,
        "output": { "result": "ok" },
    });
    let item = json!({
        "id": "c1",
        "type": "function_call",
        "name": "parent_notify",
        "call_id": "c1",
        "arguments": args.to_string(),
    });
    format!(
        "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\ndata: [DONE]\n\n"
    )
}

/// A plain text turn (no `parent_notify`): the nudge path keeps the child
/// running, which is what the stop/steering tests need.
fn plain_body(deltas: usize) -> String {
    let mut s = String::new();
    for i in 0..deltas {
        s.push_str(&format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"tick {i} \"}}\n\n"
        ));
    }
    s.push_str("data: [DONE]\n\n");
    s
}

struct Rig {
    /// Keeps the workspace dir alive for the rig's lifetime.
    #[allow(dead_code)]
    cwd: tempfile::TempDir,
    core: Arc<Core>,
    parent: String,
}

async fn rig(body: String, slow_ms: u64) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(dead_providers())
        .with_child_factory(Arc::new(CannedChild { body, slow_ms }))
        .build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace } => workspace,
        other => panic!("expected a workspace: {other:?}"),
    };
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
    Rig {
        cwd: tmp,
        core,
        parent: parent.id,
    }
}

fn spawn(rig: &Rig) -> tau_protocol::SubagentInfo {
    match rig
        .core
        .dispatch(Command::SubagentSpawn {
            session: rig.parent.clone(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent,
        other => panic!("expected a subagent: {other:?}"),
    }
}

fn state_of(rig: &Rig, handle: &str) -> String {
    match rig
        .core
        .dispatch(Command::SubagentState {
            handle: handle.into(),
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent.state,
        other => panic!("expected a subagent: {other:?}"),
    }
}

/// Bounded poll for a child state (a hung drive is a test failure, not a
/// wait). Async on purpose: the child's turn runs on this same
/// current-thread runtime, so a blocking sleep would starve it.
async fn wait_state(rig: &Rig, handle: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = state_of(rig, handle);
        if got == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the child never reached {want} (last: {got})"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn subagent_types_lists_the_builtin_general() {
    let rig = rig(done_body(), 0).await;
    let out = rig.core.dispatch(Command::SubagentTypes).unwrap();
    match out {
        CommandOutput::Agents { agents } => {
            let general = agents
                .iter()
                .find(|a| a.name == "general")
                .expect("the builtin general type is listed");
            assert!(general.builtin);
        }
        other => panic!("expected agents: {other:?}"),
    }
}

#[tokio::test]
async fn subagent_list_is_empty_before_any_spawn() {
    let rig = rig(done_body(), 0).await;
    let out = rig
        .core
        .dispatch(Command::SubagentList {
            session: rig.parent.clone(),
        })
        .unwrap();
    match out {
        CommandOutput::Subagents { subagents } => assert!(subagents.is_empty()),
        other => panic!("expected subagents: {other:?}"),
    }
}

#[tokio::test]
async fn subagent_list_refuses_a_child_session() {
    let rig = rig(done_body(), 0).await;
    let info = spawn(&rig);
    let err = rig
        .core
        .dispatch(Command::SubagentList {
            session: info.child,
        })
        .unwrap_err();
    assert!(
        matches!(err, tau_protocol::ProtocolError::Other { .. }),
        "a child listing its own children is a clean refusal: {err:?}"
    );
}

#[tokio::test]
async fn subagent_state_of_an_unknown_handle_is_not_found() {
    let rig = rig(done_body(), 0).await;
    let err = rig
        .core
        .dispatch(Command::SubagentState {
            handle: format!("{}-9", rig.parent),
        })
        .unwrap_err();
    assert!(matches!(err, tau_protocol::ProtocolError::NotFound { .. }));
}

#[tokio::test]
async fn a_spawned_child_reaches_done_and_lists_itself() {
    let rig = rig(done_body(), 0).await;
    let info = spawn(&rig);
    assert_eq!(info.handle, format!("{}-1", rig.parent));
    wait_state(&rig, &info.handle, "done").await;
    let out = rig
        .core
        .dispatch(Command::SubagentList {
            session: rig.parent,
        })
        .unwrap();
    match out {
        CommandOutput::Subagents { subagents } => {
            assert_eq!(subagents.len(), 1);
            assert_eq!(subagents[0].handle, info.handle);
            assert_eq!(subagents[0].state, "done");
        }
        other => panic!("expected subagents: {other:?}"),
    }
}

#[tokio::test]
async fn subagent_message_to_an_unknown_handle_is_refused() {
    let rig = rig(done_body(), 0).await;
    let err = rig
        .core
        .dispatch(Command::SubagentMessage {
            handle: format!("{}-9", rig.parent),
            text: Some("hello".into()),
        })
        .unwrap_err();
    match err {
        tau_protocol::ProtocolError::Other { message } => assert!(
            message.contains("no sub-agent"),
            "the refusal names the missing child: {message}"
        ),
        other => panic!("expected a clean refusal, got: {other:?}"),
    }
}

#[tokio::test]
async fn subagent_message_steers_a_running_child() {
    let rig = rig(plain_body(8), 100).await;
    let info = spawn(&rig);
    wait_state(&rig, &info.handle, "running").await;
    let out = rig
        .core
        .dispatch(Command::SubagentMessage {
            handle: info.handle.clone(),
            text: Some("steer".into()),
        })
        .unwrap();
    match out {
        CommandOutput::Subagent { subagent } => {
            assert_eq!(subagent.handle, info.handle);
            assert_eq!(subagent.state, "running");
        }
        other => panic!("expected a subagent: {other:?}"),
    }
}

#[tokio::test]
async fn subagent_stop_of_an_unknown_handle_is_not_found() {
    let rig = rig(done_body(), 0).await;
    let err = rig
        .core
        .dispatch(Command::SubagentStop {
            // A handle on the open parent with a bogus child number:
            // the dispatch arm resolves the parent first, so this reaches
            // the supervisor's own unknown-handle refusal.
            handle: format!("{}-999", rig.parent).into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, tau_protocol::ProtocolError::Other { .. }),
        "an unknown handle is a refusal: {err:?}"
    );
}

#[tokio::test]
async fn subagent_stop_of_a_running_child_is_terminal() {
    let rig = rig(plain_body(8), 100).await;
    let info = spawn(&rig);
    wait_state(&rig, &info.handle, "running").await;
    let out = rig
        .core
        .dispatch(Command::SubagentStop {
            handle: info.handle,
        })
        .unwrap();
    match out {
        CommandOutput::Subagent { subagent } => assert_eq!(subagent.state, "stopped"),
        other => panic!("expected a subagent: {other:?}"),
    }
}

#[tokio::test]
async fn a_send_to_a_child_session_routes_through_the_supervisor() {
    let rig = rig(plain_body(8), 100).await;
    let info = spawn(&rig);
    wait_state(&rig, &info.handle, "running").await;
    rig.core
        .dispatch(Command::MessageSend {
            session: info.child,
            text: "a word for the child".into(),
            lane: MessageLane::FollowUp,
        })
        .unwrap();
    // The child is still its parent's running child: the message steered,
    // it did not start a top-level turn.
    assert_eq!(state_of(&rig, &info.handle), "running");
}

#[tokio::test]
async fn a_force_send_to_a_running_child_stops_it_first() {
    // 200ms per delta: the force send lands mid-stream, while the child is
    // running.
    let rig = rig(plain_body(8), 200).await;
    let info = spawn(&rig);
    wait_state(&rig, &info.handle, "running").await;
    rig.core
        .dispatch(Command::MessageSend {
            session: info.child.clone(),
            text: "drop everything".into(),
            lane: MessageLane::Force,
        })
        .unwrap();
    // The force arm stops the child first (a `stopped` state event), then
    // the send resumes it (a `running` state event) with the message. That
    // stop-then-resume pair, plus the delivered message, is the boundary's
    // observable signature. (Note: the resume's `send` clears the stop
    // flag — ticket #23 — so the in-flight turn is not cut; see the report.)
    let path = rig
        .cwd
        .path()
        .join(".tau/sessions")
        .join(format!("{}.jsonl", info.child));
    let mut content = String::new();
    for _ in 0..100 {
        content = std::fs::read_to_string(&path).unwrap_or_default();
        if content.contains("\"state\":\"stopped\"")
            && content.contains("\"state\":\"running\"")
            && content.contains("drop everything")
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!(
        "the forced send left no stop-then-resume signature:\n{content}"
    );
}

#[tokio::test]
async fn a_stop_of_a_child_session_goes_through_the_supervisor() {
    let rig = rig(plain_body(8), 100).await;
    let info = spawn(&rig);
    wait_state(&rig, &info.handle, "running").await;
    rig.core
        .dispatch(Command::MessageStop {
            session: info.child,
        })
        .unwrap();
    wait_state(&rig, &info.handle, "stopped").await;
}

fn task_in(rig: &Rig, session: &str) -> tau_protocol::payload::Task {
    let out = rig
        .core
        .dispatch(tau_protocol::Command::SessionSnapshot {
            session: session.into(),
        })
        .unwrap();
    match out {
        CommandOutput::Snapshot { snapshot } => snapshot
            .live
            .tasks
            .into_iter()
            .next()
            .expect("the snapshot carries the session's task"),
        other => panic!("expected a snapshot: {other:?}"),
    }
}

#[tokio::test]
async fn task_create_via_dispatch_lands_in_the_snapshot() {
    let rig = rig(done_body(), 0).await;
    rig.core
        .dispatch(Command::TaskCreate {
            session: rig.parent.clone(),
            title: "Ship it".into(),
        })
        .unwrap();
    let task = task_in(&rig, &rig.parent);
    assert_eq!(task.title, "Ship it");
    assert_eq!(task.id, "task-1");
}

#[tokio::test]
async fn task_update_of_an_unknown_task_is_rejected() {
    let rig = rig(done_body(), 0).await;
    let err = rig
        .core
        .dispatch(Command::TaskUpdate {
            session: rig.parent,
            task: "task-1".into(),
            note: "a note".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, tau_protocol::ProtocolError::Other { .. }),
        "a note on a task that was never created is a clean error: {err:?}"
    );
}

#[tokio::test]
async fn task_update_records_the_note() {
    let rig = rig(done_body(), 0).await;
    rig.core
        .dispatch(Command::TaskCreate {
            session: rig.parent.clone(),
            title: "Ship it".into(),
        })
        .unwrap();
    rig.core
        .dispatch(Command::TaskUpdate {
            session: rig.parent.clone(),
            task: "task-1".into(),
            note: "blocked on review".into(),
        })
        .unwrap();
    let task = task_in(&rig, &rig.parent);
    assert_eq!(task.notes, vec!["blocked on review"]);
}

#[tokio::test]
async fn task_assign_to_an_unknown_worker_is_not_found() {
    let rig = rig(done_body(), 0).await;
    rig.core
        .dispatch(Command::TaskCreate {
            session: rig.parent.clone(),
            title: "Ship it".into(),
        })
        .unwrap();
    let err = rig
        .core
        .dispatch(Command::TaskAssign {
            session: rig.parent.clone(),
            task: "task-1".into(),
            worker: format!("{}-9", rig.parent),
        })
        .unwrap_err();
    assert!(matches!(err, tau_protocol::ProtocolError::NotFound { .. }));
}

#[tokio::test]
async fn task_assign_to_a_child_sets_the_worker_pointer() {
    let rig = rig(done_body(), 0).await;
    rig.core
        .dispatch(Command::TaskCreate {
            session: rig.parent.clone(),
            title: "Ship it".into(),
        })
        .unwrap();
    let info = spawn(&rig);
    wait_state(&rig, &info.handle, "done").await;
    rig.core
        .dispatch(Command::TaskAssign {
            session: rig.parent.clone(),
            task: "task-1".into(),
            worker: info.handle,
        })
        .unwrap();
    let task = task_in(&rig, &rig.parent);
    assert!(
        task.worker.is_some(),
        "the assignment points at the worker"
    );
}

#[tokio::test]
async fn task_evidence_of_an_unknown_task_is_rejected() {
    let rig = rig(done_body(), 0).await;
    let err = rig
        .core
        .dispatch(Command::TaskEvidence {
            session: rig.parent,
            task: "task-1".into(),
            criterion: "it builds".into(),
            summary: "green".into(),
            passed: Some(true),
        })
        .unwrap_err();
    assert!(
        matches!(err, tau_protocol::ProtocolError::Other { .. }),
        "evidence for a task that was never created is a clean error: {err:?}"
    );
}

#[tokio::test]
async fn task_evidence_on_a_pending_task_is_rejected() {
    let rig = rig(done_body(), 0).await;
    rig.core
        .dispatch(Command::TaskCreate {
            session: rig.parent.clone(),
            title: "Ship it".into(),
        })
        .unwrap();
    // A dispatch-created task has no start path (there is no TaskStart
    // command — the model's task_start tool is the only transition), so
    // evidence through dispatch surfaces the store's state refusal.
    let err = rig
        .core
        .dispatch(Command::TaskEvidence {
            session: rig.parent.clone(),
            task: "task-1".into(),
            criterion: "it builds".into(),
            summary: "green".into(),
            passed: Some(true),
        })
        .unwrap_err();
    assert!(
        matches!(err, tau_protocol::ProtocolError::Other { .. }),
        "a state refusal is an Other error: {err:?}"
    );
}

#[tokio::test]
async fn task_cancel_of_an_unknown_task_is_rejected() {
    let rig = rig(done_body(), 0).await;
    let err = rig
        .core
        .dispatch(Command::TaskCancel {
            session: rig.parent,
            task: "task-1".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, tau_protocol::ProtocolError::Other { .. }),
        "cancelling a task that was never created is a clean error: {err:?}"
    );
}

#[tokio::test]
async fn task_cancel_via_dispatch_terminates_the_task() {
    let rig = rig(done_body(), 0).await;
    rig.core
        .dispatch(Command::TaskCreate {
            session: rig.parent.clone(),
            title: "Ship it".into(),
        })
        .unwrap();
    rig.core
        .dispatch(Command::TaskCancel {
            session: rig.parent.clone(),
            task: "task-1".into(),
        })
        .unwrap();
    let task = task_in(&rig, &rig.parent);
    assert_eq!(task.status, "cancelled");
}
