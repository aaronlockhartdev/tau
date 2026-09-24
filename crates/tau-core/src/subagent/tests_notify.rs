use super::*;

use crate::session::SessionStore;
use crate::subagent::testkit::*;
use std::time::Duration;

/// The output/done rules: output without done is rejected, done
/// without an object output is rejected, a bad waiting_on is
/// rejected.
#[tokio::test]
async fn notify_rejects_invalid_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _) = harness(
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

/// The acceptance flow (ticket #24): the parent creates a task and
/// assigns it to a compacted child; the child works it — evidence,
/// then a gated finish — and ends via parent_notify; the child's
/// session is the live record (done, with the evidence) and the
/// parent's copy is the status pointer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_assigned_task_is_worked_by_the_child_and_resolves_through_the_gate() {
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
                (
                    "parent_notify".into(),
                    "n1".into(),
                    r#"{"text":"finished","done":true,"output":{"result":"the docs"}}"#.into(),
                ),
            ],
        )]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
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
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
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
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(parent.clone());
    let spawned = sup
        .spawn(
            "general",
            "work task-1",
            Some(ContextMode::Compacted),
            Some("task-1"),
            "c0",
        )
        .expect("the compacted spawn");
    // The spawn with a task IS the assignment (spec §5.3): the record
    // is in the child's session before its first turn — no second
    // store, no timing cushion (review N3/B3). The child's session is
    // the live record; the parent's copy is the status pointer.
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    // The child's session is the live record: done, with the evidence.
    let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
    child_store.open().unwrap();
    let child_tasks = crate::task::fold_entries(&child_store.entries_range(0, usize::MAX).unwrap());
    let t = &child_tasks[0];
    assert_eq!(t.id, "task-1");
    assert_eq!(t.status, crate::task::STATUS_DONE);
    assert_eq!(
        t.criteria[0].status,
        crate::task::CriterionStatus::Satisfied
    );
    // The parent's copy is the status pointer to the child.
    let mut parent_store = SessionStore::for_workspace(dir.path(), "parent");
    parent_store.open().unwrap();
    let parent_tasks =
        crate::task::fold_entries(&parent_store.entries_range(0, usize::MAX).unwrap());
    let p = &parent_tasks[0];
    assert_eq!(p.worker.as_ref().unwrap().session, spawned.session_id);
    // The pointer reflects the worker's final state (review B2): the
    // done resolution mirrored through the gate, not just the link.
    assert_eq!(p.worker.as_ref().unwrap().status, crate::task::STATUS_DONE);
    // The parent was woken by the notify.
    let wakes = bridge.wakes.lock().unwrap();
    assert!(
        wakes.iter().any(|w| w.child == spawned.session_id),
        "the parent was woken by the child's notify"
    );
}

/// A child is a leaf (spec §5.3): task_create/assign/cancel are not in
/// its tool set, and a model that calls one anyway gets the guard's
/// refusal — no phantom record lands in its session.
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
            sse(
                "",
                &[(
                    "parent_notify".into(),
                    "n1".into(),
                    r#"{"text":"done","done":true,"output":{"result":"ok"}}"#.into(),
                )],
            ),
        ]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
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
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
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
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(parent.clone());
    let spawned = sup
        .spawn("general", "go", None, None, "c0")
        .expect("the spawn");
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Done { .. }
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
        tasks.iter().all(|t| t.created_in.is_some()),
        "a child cannot create a task of its own: {tasks:?}"
    );
}

/// An assign is delivery, not just a copy (the spawn-race fix): the
/// child's message path carries "Assigned {id}: {title}". The child
/// parks after its first turn, so the assign deterministically takes
/// the resume branch: the record copy precedes the resuming message,
/// and the child's evidence lands on the parent's record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
            // Turn 1: park — the child rests until the assign, so
            // the copy can land neither before nor after its work.
            sse(
                "kicking off",
                &[(
                    "parent_notify".into(),
                    "n0".into(),
                    r#"{"text":"kicking off","done":false,"waiting_on":"parent"}"#.into(),
                )],
            ),
            // The resume turn: the record is already in the child's
            // session — the evidence lands on the parent's task.
            sse(
                "",
                &[(
                    "task_evidence".into(),
                    "e1".into(),
                    r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                )],
            ),
            sse(
                "",
                &[(
                    "parent_notify".into(),
                    "n1".into(),
                    r#"{"text":"finished","done":true,"output":{"result":"the docs"}}"#.into(),
                )],
            ),
        ]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
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
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
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
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(parent.clone());
    // A spawn without a task, then the assign: the child parks after
    // its first turn and rests until the assign moves it — the only
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
    wait_for(|| {
        matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    // The delivery reached the child as a message, and the record it
    // worked is the parent's task (no phantom of its own).
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
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-1");
    assert_eq!(tasks[0].status, crate::task::STATUS_DONE);
}

/// The state record carries the discriminant plus the non-output
/// variant fields only — done's output lives in the notify record (the
/// report), never duplicated in the state record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn state_records_carry_the_discriminant_and_variant_fields() {
    let dir = tempfile::tempdir().unwrap();
    // One child: a slow stream cut by a stop, then a parked note, then
    // a done — one of each non-running record to inspect.
    let slow = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
    );
    let (sup, bridge, _) = harness_full(
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
    // Resumed → parked: the idle record carries the waiting_on.
    sup.message(&s.handle, Some("continue".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // Resumed → done: the done record is the discriminant alone.
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
/// an unknown name is refused naming what was looked up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_subagent_tools_address_a_child_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let child_scripts = vec![vec![
        sse(
            "",
            &[notify_call("n1", "parked", false, None, Some("user"))],
        ),
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
    let (sup, _) = harness(
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
    );
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
    );
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
