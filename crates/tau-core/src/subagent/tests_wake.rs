//! The wake routing (spec §5.2, the turn-origin rule): the parent is
//! woken only when a child pauses after autonomous (spawn-origin) work.
//! A child resting after a direct message is silent to the parent —
//! the state event is the GUI's badge. `done` and `failed` always wake
//! (the result is worth interrupting for); `stopped` never does.

use std::time::Duration;

use super::testkit::*;
use super::*;
use crate::agent::Lane;
use crate::subagent::WakeKind;
use serde_json::json;

/// A slow stream, for cutting a stop mid-delta.
const SLOW: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
);

/// The wake kind follows the observed state: `done` (plan complete) and
/// `failed` (gate) always report; a spawn-origin rest reports as
/// `waiting`; `stopped` reports nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one wake-rule matrix; each child is one row (a-e)
async fn wake_kinds_follow_the_observed_state() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge, factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![
            // A: declares done, and the test has completed its plan file —
            // the observation confirms: a `Done` wake.
            vec![sse(
                "",
                &[notify_call(
                    "n1",
                    "finished",
                    true,
                    Some(json!({ "r": 1 })),
                    None,
                )],
            )],
            // B: an undeclared stop — observed `idle`: a `Waiting` wake
            // with no declared target.
            vec![sse("kicking off", &[])],
            // C: a `QUESTION:` marker in the final output — observed
            // `awaiting`: a `Waiting` wake.
            vec![sse("QUESTION: which branch?", &[])],
            // D: a declared target (the hint) rides the observed `idle`.
            vec![sse(
                "",
                &[notify_call("n2", "parked", false, None, Some("user"))],
            )],
            // E: stopped mid-stream — no wake at all.
            vec![SLOW.to_owned()],
        ],
        vec![
            Duration::from_millis(200),
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            // E: the stop-cut window — the mid-stream pause after its
            // first delta is what the 50 ms stop cuts.
            Duration::from_millis(200),
        ],
        SubAgents::default(),
    );
    let a = sup
        .spawn("general", "a", Some(ContextMode::Fresh), "c0")
        .unwrap();
    let b = sup
        .spawn("general", "b", Some(ContextMode::Fresh), "c1")
        .unwrap();
    let c = sup
        .spawn("general", "c", Some(ContextMode::Fresh), "c2")
        .unwrap();
    let d = sup
        .spawn("general", "d", Some(ContextMode::Fresh), "c3")
        .unwrap();
    let e = sup
        .spawn("general", "e", Some(ContextMode::Fresh), "c4")
        .unwrap();
    // A's done is observed: its plan file is complete before the
    // declaration lands (the test plays the model's file edit; the
    // 200 ms call pre-delay is the ordering margin).
    std::fs::write(
        crate::plan::path(dir.path(), &a.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    sup.stop(&e.handle, StoppedBy::User).unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&a.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    for h in [&b, &c, &d] {
        let h = &h.handle;
        wait_for(|| !matches!(sup.state_info(h).unwrap().state, ChildState::Running));
    }
    // A: the observed done reports its kind and payload.
    let wakes = bridge.wakes.lock();
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Done
            && w.child == a.session_id
            && w.output == Some(json!({ "r": 1 }))),
        "the observed done woke the parent: {wakes:?}"
    );
    // B: the undeclared rest is a `waiting` wake with no declared target.
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Waiting
            && w.child == b.session_id
            && w.waiting_on.is_none()),
        "the undeclared rest woke the parent as waiting: {wakes:?}"
    );
    // C: the question rest is a `waiting` wake.
    assert!(
        wakes
            .iter()
            .any(|w| w.kind == WakeKind::Waiting && w.child == c.session_id),
        "the awaiting rest woke the parent as waiting: {wakes:?}"
    );
    // D: the declared target rides the waiting wake as the hint.
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Waiting
            && w.child == d.session_id
            && matches!(w.waiting_on, Some(WaitingOn::User))),
        "the declared target is the waiting wake's hint: {wakes:?}"
    );
    // E: a stopped child wakes nothing.
    assert!(
        !wakes.iter().any(|w| w.child == e.session_id),
        "a stopped child never wakes the parent: {wakes:?}"
    );
    drop(wakes);
    // The resting drives (b, c, d, e) persist while their children sleep:
    // a stop ends them (stop_handle: a resting child's sleeping drive is
    // ended), after which every drive is quiescent.
    for n in [&b, &c, &d, &e] {
        let _ = sup.stop_handle(&n.handle, StoppedBy::User);
    }
    for n in [&a, &b, &c, &d, &e] {
        let s = &n;
        wait_for(|| sup.drive_quiescent(&s.handle));
    }
    // The slots are all free again.
    assert_eq!(factory.created.load(std::sync::atomic::Ordering::SeqCst), 5);
}

/// A child resting after a direct message is silent to the parent (the
/// state event is the GUI's badge) — only the spawn-origin rest woke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_origin_rest_is_silent_to_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![vec![
            sse("kicking off", &[]),
            sse("on the way", &[]),
            sse("", &[]),
        ]],
        vec![],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "go", Some(ContextMode::Fresh), "c0")
        .unwrap();
    // The spawn-origin rest: the parent is informed.
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    let wakes_after_spawn_rest = { bridge.wakes.lock().len() };
    // The direct message resumes the child; its next rest is a
    // message-origin rest: silent to the parent.
    sup.message(&s.handle, Some("hi".into()), Lane::Steering, None)
        .unwrap();
    // The child ran the resumed turn and rested again (two `idle`
    // records now — the spawn rest and the message rest).
    wait_for(|| {
        let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
        let _ = store.open();
        let Ok(es) = store.entries_range(0, usize::MAX) else {
            return false;
        };
        let idle = es
            .iter()
            .filter(|e| {
                e.kind == EntryKind::Subagent
                    && e.payload["event"] == json!("state")
                    && e.payload["state"] == json!("idle")
            })
            .count();
        idle >= 2
    });
    // No new wake: the message-origin rest is the badge, not a wake.
    let wakes = bridge.wakes.lock();
    assert_eq!(
        wakes.len(),
        wakes_after_spawn_rest,
        "the message-origin rest woke nothing new: {wakes:?}"
    );
    assert!(
        wakes
            .iter()
            .any(|w| w.kind == WakeKind::Waiting && w.child == s.session_id),
        "the spawn-origin rest did inform the parent: {wakes:?}"
    );
}

/// A `done` rest wakes the parent even when it follows a direct message
/// (the result is worth interrupting for, whatever the origin).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_done_rest_wakes_regardless_of_origin() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![vec![
            sse("kicking off", &[]),
            sse(
                "",
                &[notify_call(
                    "n1",
                    "finished",
                    true,
                    Some(json!({ "r": 2 })),
                    None,
                )],
            ),
            sse("", &[]),
        ]],
        vec![],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "go", Some(ContextMode::Fresh), "c0")
        .unwrap();
    // The spawn-origin rest (idle): informs the parent.
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The model completes the plan before the resumed declaration.
    std::fs::write(
        crate::plan::path(dir.path(), &s.session_id),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    // The direct message resumes the child into its done declaration.
    sup.message(&s.handle, Some("finish".into()), Lane::Steering, None)
        .unwrap();
    let info = wait_for_done(&sup, &s.handle);
    assert!(
        matches!(info.state, ChildState::Done { output: Some(_) }),
        "the observed done carries the declared output"
    );
    // The message-origin done woke the parent with its payload.
    let wakes = bridge.wakes.lock();
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Done
            && w.child == s.session_id
            && w.output == Some(json!({ "r": 2 }))),
        "a done rest wakes the parent regardless of origin: {wakes:?}"
    );
}

/// A child that declares `done` via `parent_notify` wakes the parent even
/// when its plan file is not complete — the declaration is sufficient
/// evidence of completion (issue #194: the declared output was lost and
/// the state settled to `Idle`, which suppresses the wake for
/// message-origin rests).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_done_wakes_without_plan_file() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("spawning", &[])],
        vec![vec![
            sse("kicking off", &[]),
            sse(
                "",
                &[notify_call(
                    "n1",
                    "finished the work",
                    true,
                    Some(json!({ "result": "ok" })),
                    None,
                )],
            ),
            sse("", &[]),
        ]],
        vec![],
        SubAgents::default(),
    );
    let s = sup
        .spawn("general", "go", Some(ContextMode::Fresh), "c0")
        .unwrap();
    // The spawn-origin rest (idle): informs the parent.
    wait_for(|| {
        matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    // The direct message resumes the child into its done declaration.
    // No plan file is written — the declaration alone must suffice.
    sup.message(&s.handle, Some("finish".into()), Lane::Steering, None)
        .unwrap();
    let info = wait_for_done(&sup, &s.handle);
    assert!(
        matches!(info.state, ChildState::Done { output: Some(_) }),
        "the declared done carries the output even without a plan file"
    );
    // The message-origin done woke the parent with its payload.
    let wakes = bridge.wakes.lock();
    assert!(
        wakes.iter().any(|w| w.kind == WakeKind::Done
            && w.child == s.session_id
            && w.output == Some(json!({ "result": "ok" }))),
        "a declared done wakes the parent regardless of plan state: {wakes:?}"
    );
}
