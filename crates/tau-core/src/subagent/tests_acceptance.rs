use super::*;

use crate::agent::Lane;
use crate::session::SessionStore;
use crate::subagent::testkit::*;
use serde_json::json;
use std::time::Duration;

/// The ticket's scripted acceptance: a parent spawns three children
/// concurrently; one is observed `done` (plan complete, parent woken
/// with the declared result), one rests (observed `idle`) then is
/// resumed by message to `done`, one rests undeclared (an observed
/// `idle` — the nudge is gone) — and every transition is a session
/// entry with a protocol (bridge) event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one end-to-end lifecycle flow; splitting is refactoring
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
        vec![sse("", &[])],
    ];
    // A's 300 ms call pre-delay is the ordering margin: its plan file
    // (written by the test right after the spawns) lands before its
    // first call, so its declared done is confirmed by the observation.
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        parent_bodies,
        child_scripts,
        vec![Duration::from_millis(300), Duration::ZERO, Duration::ZERO],
        SubAgents::default(),
    );

    // The parent's loop runs the spawns (tools dispatch in-loop).
    sup_spawn_from_parent(&sup).await;

    let handle_a = "parent-1".to_owned();
    let handle_b = "parent-2".to_owned();
    let handle_c = "parent-3".to_owned();

    // The model keeps A's plan current (the test plays the file edit):
    // the observation then confirms A's declared done.
    std::fs::write(
        crate::plan::path(dir.path(), &sup.state_info(&handle_a).unwrap().child),
        "# Plan\n\n## Steps\n\n- [x] do A\n",
    )
    .unwrap();
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
    // The model completes B's plan before the resumed declaration, so
    // it is confirmed by the observation.
    std::fs::write(
        crate::plan::path(dir.path(), &sup.state_info(&handle_b).unwrap().child),
        "# Plan\n\n## Steps\n\n- [x] do B\n",
    )
    .unwrap();
    // Resumed by a message (the tool's non-running branch).
    sup.message(&handle_b, Some("proceed".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&handle_b).unwrap().state,
            ChildState::Done { .. }
        )
    });

    // C: an undeclared stop is an observed `idle` (the nudge is gone,
    // spec §5.2) — the parent is informed, and no nudge ever lands in
    // the child's file.
    wait_for(|| {
        matches!(
            sup.state_info(&handle_c).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    assert!(
        bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.kind == WakeKind::Waiting
                && w.child == sup.state_info(&handle_c).unwrap().child),
        "the spawn-origin rest informed the parent"
    );
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
    assert_eq!(nudges.len(), 0, "the nudge is deleted: no nudge entry");
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
    assert!(states.contains(&"done") && states.contains(&"idle"));
}

/// Drive the parent's loop: it calls the spawn tools in its script.
async fn sup_spawn_from_parent(sup: &Arc<Supervisor>) {
    // The harness's parent agent: find it via the supervisor's parent.
    let parent = sup
        .parent
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .upgrade()
        .unwrap();
    parent.send("go", Lane::FollowUp);
    parent.process().await.unwrap();
}
