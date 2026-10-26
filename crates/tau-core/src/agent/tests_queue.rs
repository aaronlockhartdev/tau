use super::*;

use crate::agent::testkit::*;

/// Deleting a queued message by its global position removes that one item
/// and keeps the rest in order (ticket #223).
#[tokio::test]
async fn remove_queued_deletes_by_global_index_and_keeps_order() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![]));
    let agent = make_agent(dir.path(), provider);
    agent.send("one", Lane::FollowUp);
    agent.send("two", Lane::FollowUp);
    agent.send("three", Lane::Steering);
    // Deleting the middle item (global index 1) keeps both neighbors, in
    // order.
    let removed = agent.remove_queued(1).unwrap();
    assert_eq!(removed, "two");
    let items = agent.queued_items();
    assert_eq!(
        items.iter().map(|i| i.text.clone()).collect::<Vec<_>>(),
        vec!["one", "three"]
    );
}

#[tokio::test]
async fn remove_queued_out_of_bounds_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![]));
    let agent = make_agent(dir.path(), provider);
    agent.send("one", Lane::FollowUp);
    assert!(agent.remove_queued(5).is_none());
    assert_eq!(agent.queued_items().len(), 1);
    assert!(agent.remove_queued(0).is_some());
    assert!(agent.queued_items().is_empty());
}

/// A stop set while the queue has pending messages pre-empts the whole
/// drain: nothing is appended, and the queue is intact (ticket #223 — the
/// auto-send on stop). The next explicit send resumes the drain: a send
/// clears the stop and the queued items start their turns.
#[tokio::test]
async fn a_stop_preempts_the_drain_and_a_send_resumes_it() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("two", &[]),
        sse("three", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("one", Lane::FollowUp);
    agent.send("two", Lane::FollowUp);
    agent.send("three", Lane::FollowUp);
    // The stop lands while the first turn is in flight (the GUI's stop
    // button): the in-flight call is cut and the drain must stop there.
    agent.stop();
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().store);
    assert!(entries.is_empty(), "no turn ran: {entries:?}");
    // The queue is intact for the next explicit send.
    let items = agent.queued_items();
    assert_eq!(
        items.iter().map(|i| i.text.clone()).collect::<Vec<_>>(),
        vec!["one", "two", "three"]
    );
    // The next explicit send clears the stop and resumes the drain.
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().store);
    let users: Vec<&str> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::User)
        .map(|e| e.payload["text"].as_str().unwrap())
        .collect();
    assert_eq!(users, vec!["one", "two", "three", "go"]);
}
