//! Direct construction tests for the session constructor (R2): the role's
//! wiring defaults, without a dispatch round-trip.

use super::{
    AgentSession, Config, CoreBuilder, Event, SessionRole, SessionStore,
    testkit::{collect_events, open_ws, providers},
};
use crate::agent_type::builtin_general;
use crate::config::{Om, Provider, SubAgents, ToolBatchPolicy};
use crate::om::OmRecord;
use crate::subagent::testkit::{CannedFactory, TestBridge, TestDriver};
use crate::subagent::{Supervisor, SupervisorParams};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

fn canned_provider() -> crate::provider::TurnProviderRef {
    crate::provider::canned("data: [DONE]\n\n")
}

fn fresh_store(cwd: &std::path::Path, id: &str) -> SessionStore {
    let mut store = SessionStore::for_workspace(cwd, id);
    store.create().unwrap();
    store
}

#[tokio::test]
async fn launch_root_builds_the_supervisor_and_wires_the_events() {
    let core = CoreBuilder::custom(providers()).build();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());
    let collected = collect_events(&core);

    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core.clone()),
            workspace: Some(ws.clone()),
            config: Some(core.system_config()),
            provider: canned_provider(),
            supervisor: None,
        },
    )
    .unwrap();

    // The role's wiring defaults: the supervisor is built from the config
    // (the root's child surface) and the model is resolved from it.
    assert!(agent.subagents().is_some());
    assert_eq!(agent.model(), "model");

    // The entry hook is wired: an append rides the channel as an
    // EntryUpsert (ADR-0008's file-line tee).
    agent
        .append_entry("user", serde_json::json!({ "text": "hi" }))
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let seen = collected.lock().unwrap().iter().any(
            |e| matches!(e, Event::EntryUpsert { session, .. } if session == &agent_sid(&agent)),
        );
        if seen {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no EntryUpsert for the launched session within 5 s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn agent_sid(agent: &AgentSession) -> String {
    // The store is consumed by the constructor: the id is re-read through
    // the session's own store.
    agent.with_task_store(|s| s.id().to_owned())
}

#[tokio::test]
async fn launch_root_without_providers_is_a_config_error() {
    let core = CoreBuilder::custom(std::collections::BTreeMap::new()).build();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());

    let err = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core),
            workspace: Some(ws),
            config: Some(Config::default()),
            provider: canned_provider(),
            supervisor: None,
        },
    )
    .err()
    .expect("launch fails without providers");
    assert!(
        matches!(
            err,
            tau_protocol::ProtocolError::Other { ref message }
                if message == "no providers configured; add a [providers.x] section"
        ),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn launch_root_with_a_modelless_provider_names_it() {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "dev".to_owned(),
        Provider {
            base_url: "http://127.0.0.1:9/v1".into(),
            key_env: String::new(),
            models: std::collections::BTreeMap::new(),
        },
    );
    let core = CoreBuilder::custom(providers).build();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());

    let err = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core.clone()),
            workspace: Some(ws),
            config: Some(core.system_config()),
            provider: canned_provider(),
            supervisor: None,
        },
    )
    .err()
    .expect("launch fails with a modelless provider");
    assert!(
        matches!(
            err,
            tau_protocol::ProtocolError::Other { ref message }
                if message == "provider dev has no models"
        ),
        "unexpected error: {err:?}"
    );
}

/// The point of the seam: a child built through the constructor routes its
/// task tools through its parent — the wiring that was previously only
/// testable end-to-end. No supervisor harness: the pieces are built
/// directly.
#[test]
fn launch_child_routes_task_tools_through_the_parent() {
    let dir = tempfile::tempdir().unwrap();

    // The parent: a bare session (no supervisor harness).
    let parent = AgentSession::launch(
        fresh_store(dir.path(), "parent"),
        SessionRole::Bare {
            provider: canned_provider(),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: crate::tools::agent_tool_specs(),
            cwd: dir.path().to_path_buf(),
            turn: crate::agent::TurnConfig::default(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
        },
    )
    .unwrap();

    // A task on the parent's file — the single source of truth.
    let created = parent.with_task_store(|s| {
        crate::task::tool_call(
            s,
            "task_create",
            &serde_json::json!({ "title": "child ping" }),
        )
    });
    assert!(
        created.starts_with("created task-1: child ping"),
        "{created}"
    );

    // The supervisor the child's link routes to (a direct build).
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: Arc::new(CannedFactory {
            scripts: vec![],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: crate::agent::TurnConfig::default(),
        caps: SubAgents::default(),
        types: vec![builtin_general()],
        depth: 0,
        bridge: Arc::new(TestBridge::default()) as Arc<dyn crate::subagent::SubagentBridge>,
        driver: Arc::new(TestDriver),
    });

    // The child through the constructor: the role carries the link and
    // the parent task store.
    let child = AgentSession::launch(
        fresh_store(dir.path(), &SessionStore::new_session_id()),
        SessionRole::Child {
            supervisor: sup,
            parent: Arc::downgrade(&parent),
            provider: canned_provider(),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: crate::tools::child_tool_specs(),
            record: OmRecord::default(),
            handle: "parent-1".into(),
        },
    )
    .unwrap();

    // A child is a leaf: the planning tools are refused on the child.
    let refused = child.task_tool_call("task_create", &serde_json::json!({ "title": "no" }));
    assert!(
        refused.starts_with("task_create: not available in a child session"),
        "{refused}"
    );

    // The worker tools route to the parent's store: the child sees the
    // parent's task.
    let started = child.task_tool_call("task_start", &serde_json::json!({ "task": "task-1" }));
    assert!(started.contains("task-1"), "{started}");
    assert!(started.contains("child ping"), "{started}");
}
