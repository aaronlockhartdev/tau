use super::*;

use crate::session::SessionStore;
use crate::subagent::testkit::*;

/// The ticket's scripted acceptance: a parent spawns two children
/// concurrently; one finishes (parent woken with the validated
/// result), one is stopped then resumed by message, one
/// nudge-exhausts to failed — and every transition is a session entry
/// with a protocol (bridge) event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lifecycle_acceptance_flow() {
    let dir = tempfile::tempdir().unwrap();
    // Parent script: spawn three children (one call each), then end.
    let parent_bodies = vec![
        sse(
            "",
            &[(
                "subagent_spawn".into(),
                "c1".into(),
                spawn_args("do A", "fresh").to_string(),
            )],
        ),
        sse(
            "",
            &[(
                "subagent_spawn".into(),
                "c2".into(),
                spawn_args("do B", "fresh").to_string(),
            )],
        ),
        sse(
            "",
            &[(
                "subagent_spawn".into(),
                "c3".into(),
                spawn_args("do C", "fresh").to_string(),
            )],
        ),
        sse("all spawned", &[]),
    ];
    // Per-child scripts: A → done; B → a note (park), then a resumed
    // turn that finishes; C → two bare turn-ends (nudge, then failed).
    let child_scripts = vec![
        vec![
            sse(
                "",
                &[notify_call(
                    "n1",
                    "A is done",
                    true,
                    Some(json!({"answer": 42})),
                    None,
                )],
            ),
            sse("", &[]),
        ],
        vec![
            sse("", &[notify_call("n2", "B needs input", false, None, None)]),
            sse("", &[]),
            sse(
                "",
                &[notify_call(
                    "n3",
                    "B finished after resume",
                    true,
                    Some(json!({"ok": true})),
                    None,
                )],
            ),
            sse("", &[]),
        ],
        vec![sse("", &[]), sse("", &[])],
    ];
    let (sup, bridge) = harness(
        dir.path(),
        parent_bodies,
        child_scripts,
        SubAgents::default(),
    );

    // The parent's loop runs the spawns (tools dispatch in-loop).
    sup_spawn_from_parent(&sup).await;

    let handle_a = "parent-1".to_owned();
    let handle_b = "parent-2".to_owned();
    let handle_c = "parent-3".to_owned();

    // A: done — the parent is woken with the schema-validated output.
    wait_for(|| {
        matches!(
            sup.state_info(&handle_a).unwrap().state,
            ChildState::Done { .. }
        )
    });
    let wake = bridge
        .wakes
        .lock()
        .unwrap()
        .iter()
        .find(|w| w.kind == WakeKind::Done)
        .cloned()
        .expect("done wakes the parent");
    assert_eq!(wake.output, Some(json!({"answer": 42})));
    // The transition is a session entry in the child's file.
    let entry = entry_by_event(
        dir.path(),
        &sup.state_info(&handle_a).unwrap().child,
        "state",
    );
    assert!(entry.is_some(), "the done transition is a session entry");

    // B: parked by a note (default waiting_on = parent → a wake).
    wait_for(|| {
        matches!(
            sup.state_info(&handle_b).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    assert!(
        bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.kind == WakeKind::Waiting)
    );
    // Resumed by a message (the tool's non-running branch).
    sup.message(&handle_b, Some("proceed".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&handle_b).unwrap().state,
            ChildState::Done { .. }
        )
    });

    // C: nudge-exhausted → failed, and the parent is notified.
    wait_for(|| {
        matches!(
            sup.state_info(&handle_c).unwrap().state,
            ChildState::Failed { .. }
        )
    });
    assert!(
        bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.kind == WakeKind::Failed),
        "a failed child auto-notifies the parent"
    );
    // The nudge is a session entry (one-shot).
    let c_info = sup.state_info(&handle_c).unwrap();
    let mut cstore = SessionStore::for_workspace(dir.path(), &c_info.child);
    cstore.open().unwrap();
    let centries = cstore.entries_range(0, usize::MAX).unwrap();
    let nudges: Vec<_> = centries
        .iter()
        .filter(|e| {
            e.kind == crate::agent::KIND_SYSTEM
                && e.payload["note"].as_str()
                    == Some("Nudge: state what you are waiting for, or finish")
        })
        .collect();
    assert_eq!(nudges.len(), 1, "one nudge per parked state, no loops");

    // Every spawn produced a bridge event; the state stream saw the
    // full transition set.
    assert_eq!(bridge.spawns.lock().unwrap().len(), 3);
    let states: Vec<&str> = bridge
        .states
        .lock()
        .unwrap()
        .iter()
        .map(|n| n.state.kind())
        .collect();
    assert!(states.contains(&"done") && states.contains(&"idle") && states.contains(&"failed"));
}

/// Drive the parent's loop: it calls the spawn tools in its script.
async fn sup_spawn_from_parent(sup: &Arc<Supervisor>) {
    // The harness's parent agent: find it via the supervisor's parent.
    let parent = sup.parent.lock().unwrap().clone().unwrap();
    parent.send("go", Lane::FollowUp);
    parent.process().await.unwrap();
}
