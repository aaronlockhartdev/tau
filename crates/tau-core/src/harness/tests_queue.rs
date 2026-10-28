use super::*;

use crate::harness::testkit::*;

/// A `queue_item_remove` deletes the queued item at the given global
/// position and re-emits the queue projection (ticket #223).
#[tokio::test]
async fn a_queue_item_remove_deletes_and_re_projects_the_queue() {
    let cwd = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().id.clone();
    // Two follow-ups queue behind the in-flight turn.
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "start".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "first".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "second".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();
    let collected = collect_events(&core);
    core.dispatch(Command::QueueItemRemove {
        session: session_id.clone(),
        index: 1,
    })
    .unwrap();
    // The projection re-emitted by the command drops the deleted item.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let events = collected.lock().clone();
        let last = events
            .iter()
            .filter_map(|e| match e {
                Event::Queue { session, items, .. } if *session == session_id => {
                    Some(items.clone())
                }
                _ => None,
            })
            .next_back()
            .unwrap_or_default();
        if last.len() == 2 {
            assert_eq!(
                last.iter().map(|i| i.text.clone()).collect::<Vec<_>>(),
                vec!["start", "second"],
                "the middle item must be gone, in order"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no re-projected queue without the deleted item: {last:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    drop(core);
}

/// An out-of-bounds `queue_item_remove` is a clean error, not a silent
/// success (ticket #223).
#[tokio::test]
async fn an_out_of_bounds_queue_item_remove_is_an_error() {
    let cwd = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().id.clone();
    // Empty queue: any index is out of bounds.
    let err = core
        .dispatch(Command::QueueItemRemove {
            session: session_id,
            index: 0,
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "an out-of-bounds delete must be a clean error: {err:?}"
    );
}

/// Stopping a running turn leaves every queued message queued, and starts
/// no new turn for them (ticket #223 — the auto-send on stop): the queue
/// event after the stop shows all N, and no second stream ever starts.
#[tokio::test]
async fn a_stop_with_queued_messages_leaves_them_and_starts_no_new_turn() {
    let cwd = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    // A 60 s prefill: the stop lands long before the first event, and the
    // turn settles quickly after the cut.
    let live = manual_session(
        &core,
        &workspace,
        provider::prefill_hang(&canned_body(), 60_000),
        TurnConfig::default(),
    );
    let collected = collect_events(&core);
    let session_id = live.meta.lock().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "hang".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    // Two follow-ups queue behind the in-flight turn.
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "queued a".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "queued b".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();
    core.dispatch(Command::MessageStop {
        session: session_id.clone(),
    })
    .unwrap();
    // Let the in-flight turn settle and (in the bug) the drain run.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let events = collected.lock().clone();
    // Both queued items are still queued.
    let last_queue = events
        .iter()
        .filter_map(|e| match e {
            Event::Queue { session, items, .. } if *session == session_id => Some(items.clone()),
            _ => None,
        })
        .next_back()
        .unwrap_or_default();
    assert_eq!(
        last_queue
            .iter()
            .map(|i| i.text.clone())
            .collect::<Vec<_>>(),
        vec!["queued a", "queued b"],
        "the stop must leave the queue intact: {last_queue:?}"
    );
    // Exactly one stream started (the stopped one): the queued items did
    // not start turns of their own.
    let stream_starts = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::StreamStart { session, .. } if *session == session_id
            )
        })
        .count();
    assert_eq!(
        stream_starts, 1,
        "the queued items must not start new turns: {events:?}"
    );
    drop(core);
}
