use super::*;

use crate::session::SessionStore;
use crate::subagent::testkit::*;

/// The wake-rule matrix: done always, failed always, idle-parent
/// wakes a parked parent, idle-user / idle-subagent are badge-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wake_rule_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let child_scripts = vec![
        vec![
            sse(
                "",
                &[notify_call(
                    "n1",
                    "waiting for the user",
                    false,
                    None,
                    Some("user"),
                )],
            ),
            sse("", &[]),
        ],
        vec![
            sse(
                "",
                &[notify_call(
                    "n2",
                    "waiting for my child",
                    false,
                    None,
                    Some("subagent"),
                )],
            ),
            sse("", &[]),
        ],
        vec![
            sse(
                "",
                &[notify_call(
                    "n3",
                    "waiting for the parent",
                    false,
                    None,
                    Some("parent"),
                )],
            ),
            sse("", &[]),
        ],
    ];
    let (sup, bridge) = harness(
        dir.path(),
        vec![sse("spawning", &[])],
        child_scripts,
        SubAgents::default(),
    );
    for i in 0..3 {
        let r = sup.spawn(
            "general",
            &format!("child {i}"),
            Some(ContextMode::Fresh),
            None,
            "c0",
        );
        r.unwrap();
    }
    wait_for(|| {
        (0..3).all(|i| {
            matches!(
                sup.state_info(&format!("parent-{}", i + 1)).unwrap().state,
                ChildState::Idle { .. }
            )
        })
    });
    let wakes = bridge.wakes.lock().unwrap();
    // Exactly one wake: the waiting_on=parent child.
    assert_eq!(
        wakes.len(),
        1,
        "only the parent-declared child wakes the parent"
    );
    assert_eq!(wakes[0].kind, WakeKind::Waiting);
    assert_eq!(wakes[0].waiting_on, Some(WaitingOn::Parent));
}

/// A child whose script is only `parent_notify{done}` — no trailing
/// entries — must quiesce with exactly one provider call, one Done
/// state entry, and one parent wake (review B2: the loop used to
/// re-call the model forever, re-appending the Done entry and
/// re-issuing the wake every round).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_done_child_quiesces_with_exactly_one_call() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge, factory) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![vec![sse(
            "",
            &[notify_call(
                "n1",
                "done",
                true,
                Some(json!({ "ok": true })),
                None,
            )],
        )]],
        vec![],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "one call", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    s.drive.await.unwrap();
    assert_eq!(
        factory.calls.load(Ordering::SeqCst),
        1,
        "a done child makes exactly one provider call"
    );
    let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
    store.open().unwrap();
    let done_entries: Vec<_> = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|e| {
            e.kind == KIND_SUBAGENT
                && e.payload["event"] == json!("state")
                && e.payload["state"] == json!("done")
        })
        .collect();
    assert_eq!(done_entries.len(), 1, "exactly one Done state entry");
    // De-duped (the notify record is the report): the state record is
    // the discriminant alone; the output lives in the notify record.
    assert_eq!(
        done_entries[0].payload,
        json!({ "event": "state", "state": "done" })
    );
    let notify = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == json!("notify"))
        .expect("the notify record");
    assert_eq!(notify.payload["output"], json!({ "ok": true }));
    assert_eq!(
        bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.kind == WakeKind::Done)
            .count(),
        1,
        "exactly one Done wake"
    );
}

/// The done-display invariant (the user's 'done shows idle' report):
/// the Done transition is the child's final state event — the wake
/// that follows it never resets the record, and `state_info` (the
/// snapshot's and the GUI badge's source) keeps saying done.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_done_child_stays_done_after_its_wake() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge) = harness(
        dir.path(),
        vec![
            sse("parent keeps working", &[]),
            sse("second parent turn", &[]),
        ],
        vec![vec![sse(
            "",
            &[notify_call(
                "n1",
                "done",
                true,
                Some(json!({ "ok": true })),
                None,
            )],
        )]],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "finish", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    s.drive.await.unwrap();
    assert!(
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Done { .. }
        ),
        "the record must keep saying done"
    );
    // The state stream for the handle ends at Done: nothing the wake
    // triggers may overwrite it.
    let states: Vec<&str> = bridge
        .states
        .lock()
        .unwrap()
        .iter()
        .filter(|n| n.handle == s.handle)
        .map(|n| n.state.kind())
        .collect();
    assert_eq!(
        states.last().copied(),
        Some("done"),
        "the child's last state event is the done one: {states:?}"
    );
    assert!(
        bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.kind == WakeKind::Done),
        "done wakes the parent"
    );
}
