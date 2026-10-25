use super::*;

use crate::harness::testkit::*;

/// Bounded wait for the result-phase tool entry (the one with a non-empty
/// output) in a collected event stream.
async fn result_tool_entry(
    collected: &std::sync::Arc<std::sync::Mutex<Vec<Event>>>,
) -> tau_protocol::snapshot::ViewEntry {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let events = collected.lock().unwrap().clone();
        if let Some(Event::EntryUpsert { entry, .. }) = events.iter().rev().find(|e| {
            matches!(
                e,
                Event::EntryUpsert { entry, .. }
                    if entry.kind == EntryKind::Tool && entry.payload["output"] != serde_json::json!("")
            )
        }) {
            return entry.clone();
        }
        assert!(
            tokio::time::Instant::now() <= deadline,
            "no result-phase tool entry within 5s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_successful_tool_emits_status_ok_in_the_entry_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()), // bash "ls" → exit 0
        TurnConfig::default(),
    );
    let collected = collect_events(&core);
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id,
        text: "go".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let entry = result_tool_entry(&collected).await;
    assert_eq!(entry.payload["name"], "bash");
    assert_eq!(entry.payload["status"], serde_json::json!("ok"));
}

#[tokio::test]
async fn a_failing_tool_emits_status_error_in_the_entry_payload() {
    // A read of a path that does not exist: the dispatcher's failure text.
    let item = serde_json::json!({
        "id": "fc_1",
        "type": "function_call",
        "name": "read",
        "call_id": "call_1",
        "arguments": serde_json::json!({"path": "does-not-exist.txt"}).to_string(),
    });
    let data = format!("data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}");
    let body = format!("{data}\n\ndata: [DONE]\n\n");
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&body),
        TurnConfig::default(),
    );
    let collected = collect_events(&core);
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id,
        text: "go".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let entry = result_tool_entry(&collected).await;
    assert_eq!(entry.payload["name"], "read");
    assert_eq!(entry.payload["status"], serde_json::json!("error"));
}
