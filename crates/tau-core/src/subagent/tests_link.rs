//! `ChildLink`'s pointer-model tests (R3): the handle is opaque — mint,
//! store, and routing all go through the supervisor, and a parent id
//! containing a hyphen (the historical breakage case for the handle's
//! format) resolves unambiguously.

use super::testkit::{CannedFactory, ScriptedProvider, TestBridge, TestDriver, sse, wait_for};
use super::{ContextMode, SubagentBridge, Supervisor, SupervisorParams, route_parent};
use crate::agent::{AgentSession, Lane, TurnConfig};
use crate::agent_type::builtin_general;
use crate::config::{Om, SubAgents, ToolBatchPolicy};
use crate::harness::SessionRole;
use crate::session::SessionStore;
use crate::tools;
use serde_json::json;
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
        instructions: Arc::new(std::sync::Mutex::new(Vec::new())),
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
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
        gates: None,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        types: vec![builtin_general()],
        depth: 0,
        bridge: subagent_bridge,
        driver: Arc::new(TestDriver),
        core: None,
        parent_entries: None,
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
            system_prompt: None,
            base_prompt: None,
            first_provider: None,
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
        .spawn("general", "work", Some(ContextMode::Fresh), "c0")
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

    // Route a message through the link with no sender stamp: it lands in the
    // child's session (a steering round on its turn, or a resume — routing
    // either way) and the entry carries no source (the unattributed default,
    // distinct from a stamped routed send).
    let out = link.send("hello".into(), Lane::Steering, None).unwrap();
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
                        && e.payload.get("source").is_none()
                })
    });
}

/// #187: a `subagent_message` from the parent model stamps the child's
/// entry with the parent's session id as the sender — the child's
/// transcript attributes the steering to its parent, not to the user.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subagent_message_stamps_the_parent_as_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = sup_with_parent(dir.path(), "parent");

    let spawned = sup
        .spawn("general", "work", Some(ContextMode::Fresh), "c0")
        .unwrap();

    // While the child's first turn is in flight, the parent model messages
    // it: the message queues on the child's lane, stamped with the parent.
    let out = route_parent(
        &sup,
        &tools::ToolCall {
            id: "c1".into(),
            name: "subagent_message".into(),
            args: json!({ "name": spawned.name, "text": "focus on the edge cases" }),
        },
    )
    .unwrap();
    assert!(out.contains(&spawned.name), "{out}");

    // The entry settles on the child's file attributed to the parent.
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
                            == Some("focus on the edge cases")
                        && e.payload.get("source") == Some(&json!("parent"))
                })
    });
}

/// #187: the GUI's send to a child (the `MessageSend` child arm) stamps the
/// child's entry with the user as the sender — the routing boundary carries
/// the sender through to storage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gui_send_to_a_child_stamps_the_user_as_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = sup_with_parent(dir.path(), "parent");

    let spawned = sup
        .spawn("general", "work", Some(ContextMode::Fresh), "c0")
        .unwrap();

    let link = spawned
        .agent
        .child_link()
        .expect("the child carries its link");

    // The GUI stamps the user (the human is the sender).
    let out = link
        .send("steer me".into(), Lane::Steering, Some("user".into()))
        .unwrap();
    assert!(!out.is_empty(), "the link routed the message: {out}");

    // The entry settles on the child's file attributed to the user.
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
                            == Some("steer me")
                        && e.payload.get("source") == Some(&json!("user"))
                })
    });
}
