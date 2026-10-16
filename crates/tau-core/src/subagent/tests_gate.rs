//! The completion gate at the sub-agent boundary (spec §5.3): a `.md`-type
//! child's prompt is the type's own body, so the gate section must be
//! appended there when a gate is configured — "stated in the system prompt
//! when configured" holds for every child type, not just `general`.

use super::*;
use crate::agent_type::builtin_general;
use crate::harness::SessionRole;
use crate::subagent::testkit::*;

/// A gate-configured supervisor with a `.md` type in its registry: the
/// spawn's prompt assembly is under test; the child's turns are canned.
fn harness_with_gate(
    dir: &std::path::Path,
    gates: Gates,
    types: Vec<crate::agent_type::AgentType>,
) -> (Arc<Supervisor>, Arc<CannedFactory>, Arc<AgentSession>) {
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![sse("", &[])]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
        instructions: Arc::new(Mutex::new(Vec::new())),
    });
    let provider: Arc<dyn ChildProviderFactory> = factory.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.to_path_buf(),
        provider,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: Some(gates),
        caps: SubAgents::default(),
        depth: 0,
        types,
        bridge,
        driver: Arc::new(TestDriver),
        core: None,
    });
    // The spawn resolves the parent through the supervisor's `Weak` link,
    // so the parent session must exist (the test seam's attach).
    let mut store = SessionStore::for_workspace(dir, "parent");
    store.create().unwrap();
    let parent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: Arc::new(ScriptedProvider::new(vec![sse("", &[])])),
            supervisor: Some(Arc::clone(&sup)),
            system_prompt: None,
            base_prompt: None,
            first_provider: None,
        },
    )
    .unwrap();
    (sup, factory, parent)
}

#[tokio::test]
async fn a_md_type_child_prompt_carries_the_gate_section() {
    let dir = tempfile::tempdir().unwrap();
    let gates = Gates {
        command: "cargo nextest run".into(),
        ..Gates::default()
    };
    let reviewer = crate::agent_type::AgentType {
        name: "reviewer".into(),
        description: String::new(),
        body: "You review code strictly.".into(),
        tools: None,
        model: None,
        context_mode: ContextMode::Fresh,
    };
    let (sup, factory, _parent) =
        harness_with_gate(dir.path(), gates, vec![builtin_general(), reviewer]);

    let s = sup
        .spawn("reviewer", "review this", None, None, "c0")
        .unwrap();
    s.drive.await.unwrap();

    // The child's first LLM call carries the type's body plus the gate
    // section — the check is stated in the prompt (spec §5.3).
    let seen = factory.instructions.lock().unwrap();
    assert_eq!(seen.len(), 1, "one child, one first call");
    let prompt = seen[0]
        .as_ref()
        .expect("the child's call carries its system prompt");
    assert!(
        prompt.contains("You review code strictly."),
        "the type's own body: {prompt}"
    );
    assert!(
        prompt.contains("Completion gate"),
        "the gate section: {prompt}"
    );
    assert!(
        prompt.contains("cargo nextest run"),
        "the named check: {prompt}"
    );
}
