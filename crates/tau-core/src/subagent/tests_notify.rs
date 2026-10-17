use super::*;

use crate::agent::{Lane, SessionParams};
use crate::session::SessionStore;
use crate::subagent::testkit::*;
use crate::tools;
use serde_json::json;
use std::time::Duration;

/// The output/done rules: output without done is rejected, done
/// without an object output is rejected, a bad `waiting_on` is
/// rejected.
#[tokio::test]
async fn notify_rejects_invalid_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    // A child session file + link: notify validation is checked before
    // any state change.
    let mut store = SessionStore::for_workspace(dir.path(), "child");
    store.create().unwrap();
    let link = Arc::new(ChildLink {
        supervisor: sup,
        handle: "child-1".into(),
    });
    assert!(
        link.notify(&json!({ "text": "t", "output": json!({}) }))
            .contains("output requires done:true")
    );
    assert!(
        link.notify(&json!({ "text": "t", "done": true }))
            .contains("done:true requires an object output")
    );
    assert!(
        link.notify(&json!({ "text": "t", "done": true, "output": json!([1]) }))
            .contains("object output")
    );
    assert!(
        link.notify(&json!({ "text": "t", "waiting_on": "the void" }))
            .contains("waiting_on must be")
    );
}

/// The observed `done` (spec §5.2): the child keeps its own plan file
/// complete (the model's ordinary file edit — here the test plays it),
/// and its `parent_notify {done, output}` declaration rides the observed
/// rest as the result's payload. The core no longer resolves the
/// parent's task record on done: the declaration is a hint, the
/// observation is the state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one end-to-end observed-done flow; splitting is refactoring
async fn an_observed_done_is_confirmed_by_the_complete_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    crate::task::create(
        &mut store,
        "task-1",
        "write the docs",
        vec![],
        vec![crate::task::Criterion {
            text: "docs exist".into(),
            status: crate::task::CriterionStatus::Pending,
        }],
    )
    .unwrap();
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        // One call: the declaration. The pre-delay is the ordering margin —
        // the test's plan-file write (the model's edit) lands before the
        // child's first call, so the observation sees the complete plan.
        scripts: vec![vec![sse(
            "",
            &[notify_call(
                "n1",
                "finished",
                true,
                Some(json!({ "result": "the docs" })),
                None,
            )],
        )]],
        delays: vec![Duration::from_millis(300)],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
        instructions: Arc::new(Mutex::new(Vec::new())),
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: subagent_bridge,
        driver: Arc::new(TestDriver),
        core: None,
    });
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::agent_tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: crate::provider::canned(sse("", &[]).as_str()),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(&parent);
    let spawned = sup
        .spawn(
            "general",
            "work task-1",
            Some(ContextMode::Compacted),
            Some("task-1"),
            "c0",
        )
        .expect("the compacted spawn");
    // The model keeps its plan current: a complete plan file is the
    // observed `done` signal (spec §5.2).
    std::fs::write(
        crate::plan::path(dir.path(), &spawned.session_id),
        "# Plan\n\n## Steps\n\n- [x] write the docs\n",
    )
    .unwrap();
    // Observation confirms the declaration: `done` with the declared output.
    let info = wait_for_done(&sup, &spawned.handle);
    assert_eq!(
        info.state,
        ChildState::Done {
            output: Some(json!({ "result": "the docs" }))
        },
        "the declared output rides the observed done"
    );
    // The child's file carries no task entry: the record never left the
    // parent's session (the single source of truth).
    let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
    child_store.open().unwrap();
    let child_tasks = crate::task::fold_entries(&child_store.entries_range(0, usize::MAX).unwrap());
    assert!(
        child_tasks.is_empty(),
        "the child's file is task-free: {child_tasks:?}"
    );
    // The parent's task record is the model's to resolve (its task tools),
    // not the core's: the observed done leaves it as it stands.
    let mut parent_store = SessionStore::for_workspace(dir.path(), "parent");
    parent_store.open().unwrap();
    let parent_tasks =
        crate::task::fold_entries(&parent_store.entries_range(0, usize::MAX).unwrap());
    assert_eq!(parent_tasks.len(), 1);
    assert_eq!(parent_tasks[0].id, "task-1");
    assert_eq!(parent_tasks[0].status, crate::task::STATUS_IN_PROGRESS);
    assert_eq!(
        parent_tasks[0].worker.as_ref().unwrap().session,
        spawned.session_id
    );
    // The observed done woke the parent with the result.
    let wakes = bridge.wakes.lock().unwrap();
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Done
            && w.child == spawned.session_id
            && w.output == Some(json!({ "result": "the docs" }))),
        "the parent was woken by the observed done: {wakes:?}"
    );
}

/// A child is a leaf (spec §5.3): `task_create/assign/cancel` are not in
/// its tool set, and a model that calls one anyway gets the guard's
/// refusal — no phantom record lands in its session. The child's undeclared
/// stop is an observed `idle` (the nudge is gone, spec §5.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_cannot_create_assign_or_cancel_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![
            sse(
                "",
                &[(
                    "task_create".into(),
                    "c1".into(),
                    r#"{"title":"phantom"}"#.into(),
                )],
            ),
            sse("", &[]),
        ]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
        instructions: Arc::new(Mutex::new(Vec::new())),
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: subagent_bridge,
        driver: Arc::new(TestDriver),
        core: None,
    });
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::agent_tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: crate::provider::canned(sse("", &[]).as_str()),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(&parent);
    let spawned = sup
        .spawn("general", "go", None, None, "c0")
        .expect("the spawn");
    // An undeclared stop is `idle` (spec §5.2): never force-`failed`.
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The refusal is the model's only view: the tool result says so,
    // and no task record of the child's own exists in its session.
    let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
    child_store.open().unwrap();
    let entries = child_store.entries_range(0, usize::MAX).unwrap();
    let refusal = entries
        .iter()
        .filter(|e| e.kind == crate::agent::KIND_TOOL)
        .filter_map(|e| e.payload.get("output"))
        .filter_map(Value::as_str)
        .find(|o| o.contains("not available in a child session"));
    assert!(
        refusal.is_some(),
        "the child's task_create was refused by the guard"
    );
    let tasks = crate::task::fold_entries(&entries);
    assert!(
        tasks.is_empty(),
        "a child cannot create a task of its own: {tasks:?}"
    );
}

/// An assign is delivery, not just a write (the spawn-race fix): the
/// child's message path carries "Assigned {id}: {title}". The child rests
/// (observed `idle`) after its first turn, so the assign deterministically
/// takes the resume branch: the worker set precedes the resuming message,
/// and the child's evidence lands on the parent's record. Its final rest
/// is observed `idle` — the plan file is the `done` signal, and this
/// child's work lives in the parent's task record, not a plan.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one end-to-end resume flow; splitting is refactoring
async fn an_assign_to_a_parked_child_resumes_it_with_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    crate::task::create(
        &mut store,
        "task-1",
        "write the docs",
        vec![],
        vec![crate::task::Criterion {
            text: "docs exist".into(),
            status: crate::task::CriterionStatus::Pending,
        }],
    )
    .unwrap();
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![
            // Turn 1: an undeclared stop — the observed `idle` the assign
            // resumes (the nudge is gone: the rest is simply observed).
            sse("kicking off", &[]),
            // The resume turn: the child's task tools route to the
            // parent's store — the evidence lands on the parent's task.
            sse(
                "",
                &[
                    (
                        "task_evidence".into(),
                        "e1".into(),
                        r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                    ),
                    (
                        "task_finish".into(),
                        "f1".into(),
                        r#"{"task":"task-1"}"#.into(),
                    ),
                ],
            ),
            sse("", &[]),
        ]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
        instructions: Arc::new(Mutex::new(Vec::new())),
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: subagent_bridge,
        driver: Arc::new(TestDriver),
        core: None,
    });
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::agent_tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: crate::provider::canned(sse("", &[]).as_str()),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(&parent);
    // A spawn without a task, then the assign: the child rests after
    // its first turn and the assign moves it — the only
    // ordering that can't race.
    let spawned = sup
        .spawn("general", "working", None, None, "c0")
        .expect("the spawn");
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    sup.assign_task("task-1", &spawned.session_id)
        .expect("the assign");
    // The resumed work ends in an observed `idle` (no plan file): the
    // task record — not the plan — carries this child's completion.
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The delivery reached the child as a message, and the child's file
    // stays task-free — the record it worked is the parent's.
    let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
    child_store.open().unwrap();
    let entries = child_store.entries_range(0, usize::MAX).unwrap();
    assert!(
        entries.iter().any(|e| e.kind == crate::agent::KIND_USER
            && e.payload
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|t| t.contains("Assigned task-1: write the docs"))),
        "the assignment was delivered to the child as a message"
    );
    let tasks = crate::task::fold_entries(&entries);
    assert!(
        tasks.is_empty(),
        "the child's file is task-free (the record lives in the parent's): {tasks:?}"
    );
    // The parent's record: done, with the child's evidence.
    let mut parent_store = SessionStore::for_workspace(dir.path(), "parent");
    parent_store.open().unwrap();
    let parent_tasks =
        crate::task::fold_entries(&parent_store.entries_range(0, usize::MAX).unwrap());
    assert_eq!(parent_tasks.len(), 1);
    assert_eq!(parent_tasks[0].id, "task-1");
    assert_eq!(parent_tasks[0].status, crate::task::STATUS_DONE);
    assert_eq!(parent_tasks[0].evidence.len(), 1);
}

/// The shared task model: a child's worker tools (evidence, finish —
/// everything a child is not refused) route to the parent's store: the
/// child's file stays task-free, and the parent's record carries the
/// child's work, applied under the parent's own lock. The child's final
/// rest is observed `idle` — its work lives in the parent's task record,
/// not a plan file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one end-to-end task-routing flow; splitting is refactoring
async fn a_child_task_tools_route_to_the_parents_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    crate::task::create(
        &mut store,
        "task-1",
        "write the docs",
        vec![],
        vec![crate::task::Criterion {
            text: "docs exist".into(),
            status: crate::task::CriterionStatus::Pending,
        }],
    )
    .unwrap();
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![sse(
            "",
            &[
                (
                    "task_evidence".into(),
                    "e1".into(),
                    r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                ),
                (
                    "task_finish".into(),
                    "f1".into(),
                    r#"{"task":"task-1"}"#.into(),
                ),
            ],
        )]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
        instructions: Arc::new(Mutex::new(Vec::new())),
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: subagent_bridge,
        driver: Arc::new(TestDriver),
        core: None,
    });
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::agent_tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: crate::provider::canned(sse("", &[]).as_str()),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(&parent);
    let spawned = sup
        .spawn(
            "general",
            "work task-1",
            Some(ContextMode::Fresh),
            Some("task-1"),
            "c0",
        )
        .expect("the spawn");
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The child's tool calls ran against the parent's record: its file is
    // task-free, and the tool results are successes (not a missing record).
    let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
    child_store.open().unwrap();
    let entries = child_store.entries_range(0, usize::MAX).unwrap();
    assert!(
        crate::task::fold_entries(&entries).is_empty(),
        "the child's file is task-free"
    );
    let outputs: Vec<&str> = entries
        .iter()
        .filter(|e| e.kind == crate::agent::KIND_TOOL)
        .filter_map(|e| e.payload.get("output").and_then(Value::as_str))
        .collect();
    assert!(
        outputs
            .iter()
            .any(|o| o.contains("task-1") && o.contains("in_progress")),
        "the child's task tools ran against the parent's record: {outputs:?}"
    );
    assert!(
        !outputs
            .iter()
            .any(|o| o.contains("not found in this session")),
        "no task call hit a missing record: {outputs:?}"
    );
    // The parent's record: done, with the child's evidence, worker = the child.
    let mut parent_store = SessionStore::for_workspace(dir.path(), "parent");
    parent_store.open().unwrap();
    let parent_tasks =
        crate::task::fold_entries(&parent_store.entries_range(0, usize::MAX).unwrap());
    let p = &parent_tasks[0];
    assert_eq!(p.status, crate::task::STATUS_DONE);
    assert_eq!(p.evidence.len(), 1);
    assert_eq!(p.worker.as_ref().unwrap().session, spawned.session_id);
}
/// The state record carries the discriminant plus the non-output
/// variant fields only — done's output lives in the notify record (the
/// report), never duplicated in the state record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one full state-record timeline; splitting is refactoring
async fn state_records_carry_the_discriminant_and_variant_fields() {
    let dir = tempfile::tempdir().unwrap();
    // One child: a slow stream cut by a stop, then a resting note, then
    // a done — one of each non-running record to inspect. The final done
    // is observed: the test writes the child's plan file complete before
    // the last resume, so the observation confirms the declaration.
    let slow = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
    );
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![vec![
            slow.to_owned(),
            sse(
                "",
                &[notify_call(
                    "n1",
                    "parked for the user",
                    false,
                    None,
                    Some("user"),
                )],
            ),
            sse("", &[]),
            sse(
                "",
                &[notify_call(
                    "n2",
                    "done",
                    true,
                    Some(json!({ "r": 1 })),
                    None,
                )],
            ),
            sse("", &[]),
        ]],
        vec![Duration::from_millis(200)],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "work", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    // Running → stopped (user): the terminal record carries the by.
    tokio::time::sleep(Duration::from_millis(50)).await;
    sup.stop(&s.handle, StoppedBy::User).unwrap();
    // Resumed → observed `idle`: the note's declared target rides the
    // record as the badge hint.
    sup.message(&s.handle, Some("continue".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The model keeps its plan current: complete before the final rest,
    // so the observation confirms the done declaration.
    std::fs::write(
        crate::plan::path(dir.path(), &s.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    // Resumed → observed `done`: the done record is the discriminant alone.
    sup.message(&s.handle, Some("finish".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    // The drive is gone before the file is read from a second store.
    wait_for(|| sup.drive_quiescent(&s.handle));
    let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
    store.open().unwrap();
    let states: Vec<Value> = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == json!("state"))
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(
        states,
        vec![
            json!({ "event": "state", "state": "stopped", "by": "user" }),
            json!({ "event": "state", "state": "running" }),
            json!({ "event": "state", "state": "idle", "waiting_on": "user" }),
            json!({ "event": "state", "state": "running" }),
            json!({ "event": "state", "state": "done" }),
        ]
    );
    // The output is the notify record's, exactly once.
    let notify: Vec<Value> = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == json!("notify"))
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(
        notify.iter().filter(|p| p.get("output").is_some()).count(),
        1,
        "the output is the notify record's, not the state record's: {notify:?}"
    );
    // The state stream (the GUI's source) is untouched by the de-dup.
    assert!(
        bridge
            .states
            .lock()
            .unwrap()
            .iter()
            .any(|n| matches!(n.state, ChildState::Done { .. }))
    );
}

/// The sub-agent tools address a child by its displayed name (the
/// session id stays the machine key, accepted as a fallback): stop,
/// message, and state all resolve a name; the spawn reports the name;
/// an unknown name is refused naming what was looked up. Child one's
/// final `done` is observed: the test completes its plan file before the
/// resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_subagent_tools_address_a_child_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let child_scripts = vec![vec![
        sse("kicking off", &[]),
        sse(
            "",
            &[notify_call(
                "n2",
                "done",
                true,
                Some(json!({ "ok": true })),
                None,
            )],
        ),
        sse("", &[]),
    ]];
    let (sup, _, _parent) = harness(
        dir.path(),
        vec![sse("spawning", &[])],
        child_scripts,
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "work", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    assert_ne!(
        s.name, s.session_id,
        "the name and the id are different keys"
    );
    wait_for(|| {
        matches!(
            sup.state_info(&s.name).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The model keeps its plan current: complete before the resume, so the
    // child's done declaration is confirmed by the observation.
    std::fs::write(
        crate::plan::path(dir.path(), &s.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();

    // The name resolves: a stop of the parked child is a no-op that
    // names it.
    let out = sup.stop(&s.name, StoppedBy::Parent).unwrap();
    assert!(out.contains(&s.name), "{out}");

    // The session id is the machine key: accepted as a fallback.
    sup.message(&s.session_id, Some("go".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.name).unwrap().state,
            ChildState::Done { .. }
        )
    });

    // The handle (the GUI's protocol surface) resolves too.
    let info = sup.state_info(&s.handle).expect("the handle resolves");
    assert_eq!(info.child, s.session_id);

    // The subagent_state tool reports by name, not by handle.
    let out = route_parent(
        &sup,
        &tools::ToolCall {
            id: "c1".into(),
            name: "subagent_state".into(),
            args: json!({ "name": s.name }),
        },
    )
    .unwrap();
    assert!(out.starts_with(&s.name), "{out}");
    assert!(
        !out.contains(&s.handle),
        "the handle is not the model's view: {out}"
    );

    // The spawn reports the child's name — the new session's title,
    // and never its raw id.
    let out = route_parent(
        &sup,
        &tools::ToolCall {
            id: "c9".into(),
            name: "subagent_spawn".into(),
            args: json!({ "type": "general", "brief": "second", "context_mode": "fresh" }),
        },
    )
    .unwrap();
    let name = out
        .strip_prefix("spawned sub-agent ")
        .and_then(|r| r.split_once("; it reports back via parent_notify"))
        .expect("the spawn reports the name")
        .0
        .to_owned();
    let new = sup
        .children
        .lock()
        .unwrap()
        .values()
        .find(|c| c.session_id != s.session_id)
        .expect("the second child")
        .clone();
    assert_eq!(new.name, name, "{out}");
    assert!(
        !out.contains(&new.session_id),
        "the raw id stays internal: {out}"
    );

    // An unknown name is refused, naming what was looked up.
    let err = sup.stop("nobody", StoppedBy::Parent).unwrap_err();
    assert!(err.contains("no sub-agent nobody"), "{err}");
    let err = sup
        .message("nobody", Some("hi".into()), Lane::Steering)
        .unwrap_err();
    assert!(err.contains("no sub-agent nobody"), "{err}");
    assert!(sup.state_info("nobody").is_none());
}
