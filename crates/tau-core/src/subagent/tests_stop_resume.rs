use super::*;

use crate::agent::Lane;
use crate::session::SessionStore;
use crate::subagent::testkit::*;
use serde_json::json;
use std::time::Duration;

/// A running child is soft-stopped (the stream cut, the partial kept
/// as interrupted) and resumed by a message from a non-running state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_child_keeps_its_partial_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    // Child 1: a slow stream (cut by the stop); after resume an
    // undeclared stop (the observed `idle`), then — once the test has
    // completed its plan file — the done declaration the observation
    // confirms.
    let slow = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
    );
    let (sup, _bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![vec![
            slow.to_owned(),
            sse("resuming", &[]),
            sse(
                "",
                &[notify_call("n1", "done", true, Some(json!({"r": 1})), None)],
            ),
            sse("", &[]),
        ]],
        vec![Duration::from_millis(200)],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "slow work", Some(ContextMode::Fresh), "c0")
        .unwrap();
    let drive = s.drive;
    // Let the slow stream start (past the 200 ms pre-delay), then stop
    // it inside its mid-stream pause — one delta lands, the rest is cut.
    tokio::time::sleep(Duration::from_millis(300)).await;
    sup.stop(&s.handle, StoppedBy::User).unwrap();
    // The stop's cut lands while drive #1 is in flight: await it before
    // resuming — a resume send resets the stop flag, which would let the
    // cut stream run on (the resume must come after the cut).
    drive.await.unwrap();
    // The resumed drive rests (observed `idle`: no plan file yet).
    sup.message(&s.handle, Some("continue".into()), Lane::Steering, None)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The model completes the plan; the resumed declaration is then
    // confirmed by the observation.
    std::fs::write(
        crate::plan::path(dir.path(), &s.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    sup.message(&s.handle, Some("finish".into()), Lane::Steering, None)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    // The partial stands as an interrupted entry in the child's file.
    let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
    store.open().unwrap();
    let entries = store.entries_range(0, usize::MAX).unwrap();
    let interrupted = entries
        .iter()
        .find(|e| e.kind == EntryKind::Assistant && e.payload["interrupted"] == json!(true))
        .expect("the partial is kept as interrupted");
    assert!(
        interrupted.payload["text"]
            .as_str()
            .unwrap()
            .contains("partial")
    );
    // The resume is a session entry (the state transition is durable).
    let mut cstore = SessionStore::for_workspace(dir.path(), &s.session_id);
    cstore.open().unwrap();
    let entries = cstore.entries_range(0, usize::MAX).unwrap();
    assert!(
        entries.iter().any(|e| e.kind == EntryKind::Subagent
            && e.payload["event"] == json!("state")
            && e.payload["state"] == json!("running")),
        "the resume transition is recorded"
    );
}

/// A running child is stopped: the partial kept, the terminal Stopped
/// recorded, the parent woken, and the concurrency slot freed — a
/// second spawn at the cap succeeds while the stopped child rests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_child_is_stopped_and_releases_its_slot() {
    let dir = tempfile::tempdir().unwrap();
    let slow = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
    );
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![
            vec![slow.to_owned(), sse("", &[])],
            vec![
                sse(
                    "",
                    &[notify_call(
                        "n1",
                        "done",
                        true,
                        Some(json!({ "ok": true })),
                        None,
                    )],
                ),
                sse("", &[]),
            ],
        ],
        vec![Duration::from_millis(200), Duration::from_millis(200)],
        SubAgents {
            max_depth: 1,
            max_concurrent: Some(1),
            ..SubAgents::default()
        },
    );
    let first = sup
        .spawn("general", "slow", Some(ContextMode::Fresh), "c0")
        .unwrap();
    let drive = first.drive;
    // Let the slow stream start (past the 200 ms pre-delay), then stop
    // it inside its mid-stream pause.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let out = sup.stop(&first.handle, StoppedBy::User).unwrap();
    assert!(out.contains("stopped"), "{out}");
    drive.await.unwrap();
    assert!(
        matches!(
            sup.state_info(&first.handle).unwrap().state,
            ChildState::Stopped {
                by: StoppedBy::User
            }
        ),
        "the stopped state is recorded with its provenance"
    );
    assert!(bridge.states.lock().unwrap().iter().any(|n| n.state
        == ChildState::Stopped {
            by: StoppedBy::User
        }));
    // Spec §5.2: a stopped child wakes nothing — the state event is the
    // GUI's badge. (Asserted in tests_wake's matrix.)
    // A second stop of the stopped child is a no-op.
    let out = sup.stop(&first.handle, StoppedBy::User).unwrap();
    assert!(out.contains("already stopped"), "{out}");
    // The slot is freed: a second child spawns at the cap. Its plan file
    // is complete (the test plays the model's edit; the 200 ms pre-delay
    // is the ordering margin), so its declared done is observed `done` —
    // the drive ends.
    let second = sup
        .spawn("general", "second", Some(ContextMode::Fresh), "c1")
        .unwrap();
    std::fs::write(
        crate::plan::path(dir.path(), &second.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    second.drive.await.unwrap();
    assert!(
        matches!(
            sup.state_info(&second.handle).unwrap().state,
            ChildState::Done { .. }
        ),
        "the freed slot carried a real spawn"
    );
}

/// A stop of a non-running child is a no-op with a clear message:
/// the state that records how it got there (done's output, the stop's
/// provenance) is never overwritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_running_child_refuses_a_second_stop() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![
            vec![sse(
                "",
                &[notify_call(
                    "n1",
                    "done",
                    true,
                    Some(json!({ "ok": true })),
                    None,
                )],
            )],
            vec![
                sse(
                    "",
                    &[notify_call("n1", "parked", false, None, Some("user"))],
                ),
                sse("", &[]),
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
            ],
        ],
        vec![Duration::from_millis(200), Duration::from_millis(200)],
        SubAgents::default(),
    );
    let done_child = sup
        .spawn("general", "one", Some(ContextMode::Fresh), "c0")
        .unwrap();
    // The model keeps its plan complete: the declared done is observed.
    std::fs::write(
        crate::plan::path(dir.path(), &done_child.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&done_child.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    let out = sup.stop(&done_child.handle, StoppedBy::User).unwrap();
    assert!(out.contains("already done"), "{out}");
    let parked = sup
        .spawn("general", "two", Some(ContextMode::Fresh), "c1")
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&parked.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The model completes its plan before the resumed declaration, so
    // that one is observed `done`.
    std::fs::write(
        crate::plan::path(dir.path(), &parked.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    // A resting child is not running: the stop is a no-op and the rest
    // stands.
    let out = sup.stop(&parked.handle, StoppedBy::User).unwrap();
    assert!(out.contains("resting"), "{out}");
    assert!(matches!(
        sup.state_info(&parked.handle).unwrap().state,
        ChildState::Idle { .. }
    ));
    // A message resumes it; its next scripted turn finishes the child.
    sup.message(&parked.handle, Some("go".into()), Lane::Steering, None)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&parked.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    let out = sup.stop(&parked.handle, StoppedBy::User).unwrap();
    assert!(out.contains("already done"), "{out}");
    // The done child's record is untouched: exactly one state event.
    let done_states: Vec<&str> = bridge
        .states
        .lock()
        .unwrap()
        .iter()
        .filter(|n| n.handle == done_child.handle)
        .map(|n| n.state.kind())
        .collect();
    assert_eq!(done_states, vec!["done"], "{done_states:?}");
}
