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
/// rest as the result's payload. The declaration is a hint, the
/// observation is the state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one end-to-end observed-done flow; splitting is refactoring
async fn an_observed_done_is_confirmed_by_the_complete_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
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
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: subagent_bridge,
        driver: Arc::new(TestDriver),
        core: None,
        parent_entries: None,
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
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(&parent);
    let spawned = sup
        .spawn("general", "work", Some(ContextMode::Compacted), "c0")
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
    // The observed done woke the parent with the result.
    let wakes = bridge.wakes.lock().unwrap();
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Done
            && w.child == spawned.session_id
            && w.output == Some(json!({ "result": "the docs" }))),
        "the parent was woken by the observed done: {wakes:?}"
    );
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
        .spawn("general", "work", Some(ContextMode::Fresh), "c0")
        .unwrap();
    // Running → stopped (user): the terminal record carries the by.
    tokio::time::sleep(Duration::from_millis(50)).await;
    sup.stop(&s.handle, StoppedBy::User).unwrap();
    // Resumed → observed `idle`: the note's declared target rides the
    // record as the badge hint.
    sup.message(&s.handle, Some("continue".into()), Lane::Steering, None)
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
    sup.message(&s.handle, Some("finish".into()), Lane::Steering, None)
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
        .filter(|e| e.kind == EntryKind::Subagent && e.payload["event"] == json!("state"))
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
        .filter(|e| e.kind == EntryKind::Subagent && e.payload["event"] == json!("notify"))
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
        .spawn("general", "work", Some(ContextMode::Fresh), "c0")
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
    sup.message(&s.session_id, Some("go".into()), Lane::Steering, None)
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
        .message("nobody", Some("hi".into()), Lane::Steering, None)
        .unwrap_err();
    assert!(err.contains("no sub-agent nobody"), "{err}");
    assert!(sup.state_info("nobody").is_none());
}
