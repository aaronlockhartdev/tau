use super::*;

use crate::harness::testkit::*;

/// The queue's full-state projection the GUI deletes from: the last
/// `Event::Queue` for the session.
fn queue_of(
    collected: &Arc<Mutex<Vec<Event>>>,
    want: &str,
) -> Vec<tau_protocol::snapshot::QueuedItem> {
    collected
        .lock()
        .clone()
        .iter()
        .filter_map(|e| match e {
            Event::Queue { session, items, .. } if session == want => Some(items.clone()),
            _ => None,
        })
        .next_back()
        .unwrap_or_default()
}

/// A `queue_item_remove` deletes the queued item with the given id and
/// re-projects the queue without it (ticket #223, id-addressed by #346).
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
    let collected = collect_events(&core);
    // Three follow-ups queue behind the in-flight turn. Every dispatch runs
    // before any await, so no turn consumes an item while the test reads the
    // projection it deletes from.
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
    // The projection the GUI deletes from carries the middle item's id, and
    // the agent's queue is the single source `emit_queue` projects (#346).
    let before = live.agent.queued_items();
    assert_eq!(
        before.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
        vec!["start", "first", "second"],
        "the sends project in order"
    );
    assert!(
        before
            .iter()
            .zip(before.iter().skip(1))
            .all(|(a, b)| a.id != b.id),
        "every queued item carries its own id: {before:?}"
    );
    let middle = before[1].id.clone();
    core.dispatch(Command::QueueItemRemove {
        session: session_id.clone(),
        item: middle,
    })
    .unwrap();
    // The full-state replacement the command re-emits drops the deleted item
    // and keeps the survivors' ids — a re-projected queue never renumbers a row.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let last = queue_of(&collected, &session_id);
        if last.len() == 2 {
            assert_eq!(
                last.iter().map(|i| i.text.clone()).collect::<Vec<_>>(),
                vec!["start", "second"],
                "the middle item must be gone, in order"
            );
            assert_eq!(
                last.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                vec![before[0].id.clone(), before[2].id.clone()],
                "the survivors keep their ids"
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

/// An unknown id (the item was consumed first) is a `NotFound`, not a silent
/// success — with items addressed by id, an out-of-range index is
/// unrepresentable (#346).
#[tokio::test]
async fn an_unknown_queue_item_id_is_not_found() {
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
    // Empty queue: no id names anything.
    let err = core
        .dispatch(Command::QueueItemRemove {
            session: session_id,
            item: "q1".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::NotFound { .. }),
        "an unknown queue item must be a NotFound: {err:?}"
    );
}

/// A child session's queue is unmutatable through the seam: the refusal is
/// `Unsupported` — a structural fact about the target, not a transient
/// condition (#346).
#[tokio::test]
async fn a_child_queue_remove_is_unsupported() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(CannedChildFactory { body: done_body() }))
        .build()
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let child = match core
        .dispatch(Command::SubagentSpawn {
            session: parent.id.clone(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent.child,
        other => panic!("expected a subagent: {other:?}"),
    };
    let err = core
        .dispatch(Command::QueueItemRemove {
            session: child,
            item: "q1".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Unsupported { .. }),
        "a child's queue is the parent's: {err:?}"
    );
    drop(core);
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
    // Both queued items are still queued.
    let last_queue = queue_of(&collected, &session_id);
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
    let stream_starts = collected
        .lock()
        .clone()
        .iter()
        .filter(|e| matches!(e, Event::StreamStart { session, .. } if *session == session_id))
        .count();
    assert_eq!(
        stream_starts, 1,
        "the queued items must not start new turns"
    );
    drop(core);
}
