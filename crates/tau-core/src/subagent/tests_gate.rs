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
    scripts: Vec<Vec<String>>,
) -> (
    Arc<Supervisor>,
    Arc<TestBridge>,
    Arc<CannedFactory>,
    Arc<AgentSession>,
) {
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts,
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
        bridge: bridge.clone(),
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
    (sup, bridge, factory, parent)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
    let (sup, _bridge, factory, _parent) = harness_with_gate(
        dir.path(),
        gates,
        vec![builtin_general(), reviewer],
        vec![vec![sse("", &[])]],
    );

    let s = sup
        .spawn("reviewer", "review this", None, None, "c0")
        .unwrap();
    // The child's one scripted turn lands (the first call's prompt is
    // captured), and it rests (observed `idle`); stop_handle then ends
    // the resting drive so the test's handles quiesce.
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    sup.stop_handle(&s.handle, StoppedBy::User).unwrap();
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

/// The `gate_failed → failed` arm of the observed rest (spec §5.2): a child
/// whose gate check has exhausted its repairs rests `failed`, not `idle`
/// (the gate is checked before the plan; the idle cases elsewhere are the
/// negative). `failed` also auto-notifies the parent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gate_failed_child_rests_failed_not_idle() {
    let dir = tempfile::tempdir().unwrap();
    let gates = Gates {
        command: "false".into(),
        max_retries: 0,
        ..Gates::default()
    };
    // The child uses a tool (the gate's qualification), then ends the turn:
    // the failing check exhausts its zero repairs and marks the session
    // gate_failed.
    let script = vec![
        sse(
            "",
            &[(
                "bash".into(),
                "c1".into(),
                r#"{"command": "echo work"}"#.into(),
            )],
        ),
        sse("the work is done", &[]),
    ];
    let (sup, bridge, _factory, _parent) =
        harness_with_gate(dir.path(), gates, vec![builtin_general()], vec![script]);
    let s = sup
        .spawn("general", "work", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Failed { .. }
        )
    });
    // The rest is `failed` (observed), carrying the gate's reason.
    let info = sup.state_info(&s.handle).unwrap();
    assert!(
        matches!(
            &info.state,
            ChildState::Failed { reason } if reason.contains("gate_failed")
        ),
        "the observed state: {info:?}"
    );
    // A failed child auto-notifies the parent (ADR-0001 wake rules).
    assert!(
        bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.child == s.session_id && w.kind == WakeKind::Failed)
    );
    s.drive.await.unwrap();
}
