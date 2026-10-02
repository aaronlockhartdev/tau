//! The tool surface's own tests (R1): the role→specs table, the spawn-time
//! agent-type filter, and the refusal rules — each a direct unit test of
//! the module, previously only reachable by running a whole turn.

use super::surface;
use super::{ToolCall, ToolOutput};
use crate::agent::{AgentSession, TurnConfig};
use crate::agent_type::builtin_general;
use crate::config::{Om, SubAgents, ToolBatchPolicy};
use crate::harness::SessionRole;
use crate::om::OmRecord;
use crate::provider::ToolSpec;
use crate::session::SessionStore;
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

/// The tool output's text: an image block on these paths is a bug.
fn text(out: ToolOutput) -> String {
    match out {
        ToolOutput::Text(t) => t,
        ToolOutput::Image { .. } => panic!("expected text, got an image block"),
    }
}

fn names(specs: Vec<ToolSpec>) -> Vec<String> {
    specs.into_iter().map(|s| s.name).collect()
}

/// A parent + a directly built supervisor + a child through the
/// constructor (R2's seam): the shape the refusal tests route through,
/// without a dispatch round-trip.
fn parent_child(dir: &std::path::Path) -> (Arc<AgentSession>, Arc<Supervisor>, Arc<AgentSession>) {
    let parent = AgentSession::launch(
        fresh_store(dir, "parent"),
        SessionRole::Bare {
            provider: canned_provider(),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: surface::root_specs(),
            cwd: dir.to_path_buf(),
            turn: TurnConfig::default(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            om: None,
            om_model: String::new(),
        },
    )
    .unwrap();

    let bridge: Arc<dyn crate::subagent::SubagentBridge> = Arc::new(TestBridge::default());
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.to_path_buf(),
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
        turn: TurnConfig::default(),
        caps: SubAgents::default(),
        types: vec![builtin_general()],
        depth: 0,
        bridge,
        driver: Arc::new(TestDriver),
    });

    let child = AgentSession::launch(
        fresh_store(dir, "child"),
        SessionRole::Child {
            supervisor: Arc::clone(&sup),
            parent: Arc::downgrade(&parent),
            provider: canned_provider(),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: surface::child_specs(None),
            record: OmRecord::default(),
            handle: "parent-1".into(),
        },
    )
    .unwrap();

    (parent, sup, child)
}

/// The role→specs table (R1): per role, the exact spec set the model sees
/// — and a role never sees another role's tools.
#[test]
fn the_role_specs_table_is_exact() {
    let dir = tempfile::tempdir().unwrap();
    let (parent, sup, _child) = parent_child(dir.path());

    let root = SessionRole::Root {
        core: None,
        workspace: None,
        config: None,
        provider: canned_provider(),
        supervisor: None,
        system_prompt: None,
        first_provider: None,
    };
    assert_eq!(
        names(surface::specs_for(&root, None)),
        vec![
            "read",
            "write",
            "edit",
            "bash",
            "recall",
            "subagent_spawn",
            "subagent_message",
            "subagent_stop",
            "subagent_state",
            "task_create",
            "task_assign",
            "task_start",
            "task_evidence",
            "task_block",
            "task_finish",
            "task_cancel"
        ]
    );

    let child = SessionRole::Child {
        supervisor: sup,
        parent: Arc::downgrade(&parent),
        provider: canned_provider(),
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: vec![],
        record: OmRecord::default(),
        handle: "parent-1".into(),
    };
    let child_names = names(surface::specs_for(&child, None));
    assert_eq!(
        child_names,
        vec![
            "read",
            "write",
            "edit",
            "bash",
            "recall",
            "parent_notify",
            "task_start",
            "task_evidence",
            "task_block",
            "task_finish"
        ]
    );

    let bare = SessionRole::Bare {
        provider: canned_provider(),
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: vec![],
        cwd: dir.path().to_path_buf(),
        turn: TurnConfig::default(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        om: None,
        om_model: String::new(),
    };
    assert_eq!(
        names(surface::specs_for(&bare, None)),
        vec!["read", "write", "edit", "bash", "recall"]
    );

    // Negative: a role never sees another role's tools.
    for foreign in [
        "subagent_spawn",
        "subagent_message",
        "subagent_stop",
        "subagent_state",
        "task_create",
        "task_assign",
        "task_cancel",
    ] {
        assert!(
            !child_names.contains(&foreign.to_owned()),
            "the child sees {foreign}"
        );
    }
    let root_names = names(surface::specs_for(&root, None));
    assert!(
        !root_names.contains(&"parent_notify".to_owned()),
        "the root sees parent_notify"
    );
}

/// The spawn-time agent-type filter (spec §5.5) lives in the surface: a
/// `.md` type's allowlist subsets the child's default set.
#[test]
fn the_child_agent_type_filter_subsets_the_default_set() {
    let allowed = vec!["read".to_owned(), "bash".to_owned()];
    assert_eq!(
        names(surface::child_specs(Some(&allowed))),
        vec!["read", "bash"]
    );
    // An unknown name in the allowlist matches nothing.
    let allowed = vec!["read".to_owned(), "nope".to_owned()];
    assert_eq!(names(surface::child_specs(Some(&allowed))), vec!["read"]);
}

/// A child dispatching a supervisor tool gets the specific refusal, not a
/// silent no-op — through the surface, without a turn.
#[tokio::test]
async fn a_child_refuses_supervisor_tools_through_the_surface() {
    let dir = tempfile::tempdir().unwrap();
    let (_parent, _sup, child) = parent_child(dir.path());
    let cwd = dir.path().to_path_buf();

    let call = ToolCall {
        id: "c1".into(),
        name: "subagent_spawn".into(),
        args: serde_json::json!({ "type": "general", "brief": "x" }),
    };
    let out = surface::dispatch(&child, &cwd, None, None, child.child_link(), &call).await;
    assert_eq!(text(out), "subagent_spawn: not available in this session");

    let call = ToolCall {
        id: "c2".into(),
        name: "task_create".into(),
        args: serde_json::json!({ "title": "t" }),
    };
    let out = surface::dispatch(&child, &cwd, None, None, child.child_link(), &call).await;
    assert_eq!(
        text(out),
        "task_create: not available in a child session — work the task your parent assigned"
    );
}

/// A top-level session dispatching `parent_notify` gets the specific
/// refusal — through the surface, without a turn.
#[tokio::test]
async fn a_parent_refuses_parent_notify_through_the_surface() {
    let dir = tempfile::tempdir().unwrap();
    let (parent, sup, _child) = parent_child(dir.path());
    let cwd = dir.path().to_path_buf();

    let call = ToolCall {
        id: "c1".into(),
        name: "parent_notify".into(),
        args: serde_json::json!({ "text": "hi" }),
    };
    let out = surface::dispatch(&parent, &cwd, None, Some(sup), None, &call).await;
    assert_eq!(
        text(out),
        "parent_notify: not available in a top-level session"
    );
}

/// The routing invariant (ticket #23): a child carries a link AND core
/// tools — a core call is not swallowed by the link's presence.
#[tokio::test]
async fn a_child_core_tool_is_not_swallowed_by_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let (_parent, _sup, child) = parent_child(dir.path());
    let cwd = dir.path().to_path_buf();

    let call = ToolCall {
        id: "c1".into(),
        name: "bash".into(),
        args: serde_json::json!({ "command": "echo surface" }),
    };
    let out = surface::dispatch(&child, &cwd, None, None, child.child_link(), &call).await;
    let t = text(out);
    assert!(t.starts_with("exit 0"), "{t}");
    assert!(t.contains("surface"), "{t}");
}
