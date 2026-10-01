//! ChildLink's pointer-model tests (R3): the handle is opaque — mint,
//! store, and routing all go through the supervisor, and a parent id
//! containing a hyphen (the historical breakage case for the handle's
//! format) resolves unambiguously.

use super::testkit::{CannedFactory, ScriptedProvider, TestBridge, TestDriver, sse, wait_for};
use super::{ContextMode, Lane, SubagentBridge, Supervisor, SupervisorParams};
use crate::agent::{AgentSession, TurnConfig};
use crate::agent_type::builtin_general;
use crate::config::{Om, SubAgents, ToolBatchPolicy};
use crate::harness::SessionRole;
use crate::session::SessionStore;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// A supervisor + attached parent whose session id is `parent_session`
/// (the testkit's harness fixes it to "parent"; the hyphenated-id tests
/// need the choice).
fn sup_with_parent(
    dir: &std::path::Path,
    parent_session: &str,
) -> (Arc<Supervisor>, Arc<TestBridge>, Arc<AgentSession>) {
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![sse("child done", &[])]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: parent_session.into(),
        cwd: dir.to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        caps: SubAgents::default(),
        types: vec![builtin_general()],
        depth: 0,
        bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
    });
    // A scripted parent loop (the parent is a full session): the
    // constructor adopts the pre-built supervisor (the test seam).
    let mut store = SessionStore::for_workspace(dir, parent_session);
    store.create().unwrap();
    let parent_provider = Arc::new(ScriptedProvider::new(vec![sse("parent", &[])]));
    let parent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: parent_provider,
            supervisor: Some(sup.clone()),
        },
    )
    .unwrap();
    (sup, bridge, parent)
}

/// Mint → store → route with a parent id containing a hyphen: the link
/// knows its parent from the supervisor's own record and its session id
/// from the children map — never from the handle's format.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_handle_round_trips_with_a_hyphenated_parent_id() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = sup_with_parent(dir.path(), "my-parent");

    let spawned = sup
        .spawn("general", "work", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    assert_eq!(spawned.handle.as_str(), "my-parent-1");

    let link = spawned
        .agent
        .child_link()
        .expect("the child carries its link");

    // The pointer model, without a parse: the parent is the supervisor's
    // own session; the child's session id comes from the children map.
    assert_eq!(link.parent_session(), "my-parent");
    assert_eq!(
        sup.child_session_id(&spawned.handle).as_deref(),
        Some(spawned.session_id.as_str())
    );

    // Route a message through the link: it lands in the child's session
    // (a steering round on its turn, or a resume — routing either way).
    let out = link.send("hello".into(), Lane::Steering).unwrap();
    assert!(!out.is_empty(), "the link routed the message: {out}");
    wait_for(|| {
        let mut store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
        store.open().is_ok()
            && store
                .entries_range(0, usize::MAX)
                .unwrap_or_default()
                .iter()
                .any(|e| {
                    e.kind == crate::agent::KIND_USER
                        && e.payload.get("text").and_then(serde_json::Value::as_str)
                            == Some("hello")
                })
    });
}

/// The child's task view is the parent's task list filtered to
/// worker == this child (ADR-0001): the link owns the projection, and
/// the parent's full list stays reachable for the projection events.
#[tokio::test]
async fn a_child_task_view_is_the_parent_filtered_to_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, parent) = sup_with_parent(dir.path(), "my-parent");

    // Two tasks on the parent's file; the spawn assigns one to the child
    // (the record stays in the parent's file with the child as worker).
    // The tasks are created through the parent agent's store (the single
    // writer): an external store handle would grow the file out from under
    // the agent's in-memory id set, and the next append is refused.
    parent.with_task_store(|cstore| {
        crate::task::create(cstore, "task-1", "the assigned job", vec![], vec![]).unwrap();
        crate::task::create(cstore, "task-2", "unassigned", vec![], vec![]).unwrap();
    });

    let spawned = sup
        .spawn(
            "general",
            "work",
            Some(ContextMode::Fresh),
            Some("task-1"),
            "c0",
        )
        .unwrap();
    let link = spawned
        .agent
        .child_link()
        .expect("the child carries its link");

    // The child's view: exactly the assigned task.
    let view = link.task_view(dir.path());
    assert_eq!(
        view.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["task-1"]
    );

    // The parent's full list: both tasks (the projection events' source).
    let all = link
        .parent_tasks(dir.path())
        .expect("the parent's file is readable");
    assert_eq!(all.len(), 2);
}
