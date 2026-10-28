use super::*;

use crate::harness::testkit::*;

/// The entry event pipe carries a canned turn's tool events to the sink —
/// the GUI's tool cards are event-driven (spec §8), never polled.
#[tokio::test]
#[allow(clippy::too_many_lines)] // one end-to-end turn flow; splitting is refactoring
async fn the_event_pipe_carries_a_canned_turn_to_the_sink() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );

    // The collector stands in for the Tauri emit: everything the pump
    // flushes is exactly what the Svelte shell would receive.
    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().extend(batch.iter().cloned());
        })
        .await;
    });

    // The meta guard must not live across this await: the dispatch
    // path re-locks the same mutex (emit_queue), and a std mutex is
    // not reentrant.
    let session_id = live.meta.lock().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id,
        text: "do the thing".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // The canned turn is fast; give the pump a bounded time to flush.
    for _ in 0..500 {
        let events = collected.lock().clone();
        if events.iter().any(|e| matches!(e, Event::StreamEnd { .. }))
            && events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        Event::EntryUpsert { entry, .. } if entry.kind == EntryKind::Tool
                    )
                })
                .count()
                >= 2
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let events = collected.lock().clone();

    // The stream: one start, the assistant entry's growth as frame-aligned
    // deltas (#303), one end with the usage; the tool batch: one id, re-emitted
    // call → result; a starter send while idle is the turn itself and never
    // sits in the queue.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StreamStart { .. })),
        "no stream start: {events:?}"
    );
    // The final emission is the file line: it carries the calls.
    let Some(Event::EntryUpsert {
        entry: final_assistant,
        ..
    }) = events.iter().rev().find(|e| {
        matches!(
            e,
            Event::EntryUpsert { entry, .. } if entry.kind == EntryKind::Assistant
        )
    })
    else {
        panic!("no final assistant upsert: {events:?}");
    };
    assert_eq!(final_assistant.payload["reasoning"], "thinking");
    assert_eq!(
        final_assistant.payload["calls"]
            .as_array()
            .map(std::vec::Vec::len),
        Some(1)
    );
    // A mid-stream snapshot never crosses the pipe: the stream's growth
    // arrives as deltas, each carrying only the characters the last flush
    // did not, so their seq is contiguous from 1 and their text and reasoning
    // are a prefix of the file line that settles the card. A prefix, not the
    // whole: the growth inside the last frame window rides on the record, which
    // is why the file line — not the deltas — is the recovery.
    let deltas: Vec<(u64, &str, &str)> = events
        .iter()
        .filter_map(|e| match e {
            Event::StreamDelta {
                id,
                seq,
                text,
                reasoning,
                ..
            } if *id == final_assistant.id => Some((*seq, text.as_str(), reasoning.as_str())),
            _ => None,
        })
        .collect();
    assert!(!deltas.is_empty(), "no stream deltas: {events:?}");
    assert_eq!(
        deltas.iter().map(|d| d.0).collect::<Vec<_>>(),
        (1..=u64::try_from(deltas.len()).expect("a delta count that fits u64")).collect::<Vec<_>>(),
        "the delta seq is not contiguous"
    );
    for (field, streamed) in [
        ("text", deltas.iter().map(|d| d.1).collect::<String>()),
        ("reasoning", deltas.iter().map(|d| d.2).collect::<String>()),
    ] {
        let committed = final_assistant.payload[field].as_str().unwrap_or("");
        assert!(
            committed.starts_with(&streamed),
            "the {field} deltas are not a prefix of the record: {streamed:?} in {committed:?}"
        );
    }
    // The record follows its growth: the stream's last delta precedes the
    // entry's file line, which is the first assistant upsert the pipe carries.
    let last_delta = events
        .iter()
        .rposition(|e| matches!(e, Event::StreamDelta { .. }))
        .expect("a delta");
    let first_upsert = events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::EntryUpsert { entry, .. } if entry.kind == EntryKind::Assistant
            )
        })
        .expect("an assistant upsert");
    assert!(
        last_delta < first_upsert,
        "the file line landed before the stream's last delta"
    );
    let Some(Event::StreamEnd {
        interrupted, usage, ..
    }) = events.iter().find(|e| matches!(e, Event::StreamEnd { .. }))
    else {
        panic!("missing stream end: {events:?}");
    };
    assert!(!interrupted);
    assert_eq!(usage.map(|u| u.total_tokens), Some(15));
    // The tool call: one id, re-emitted call → result (ADR-0008); the
    // post-turn reconciliation re-sends the file line, so the id appears
    // with an empty output and again with the result's.
    let tools: Vec<&tau_protocol::snapshot::ViewEntry> = events
        .iter()
        .filter_map(|e| match e {
            Event::EntryUpsert { entry, .. } if entry.kind == EntryKind::Tool => Some(entry),
            _ => None,
        })
        .collect();
    assert!(!tools.is_empty(), "no tool upserts: {events:?}");
    let first_tool_id = tools[0].id.clone();
    assert!(
        tools
            .iter()
            .any(|t| t.id == first_tool_id && t.payload["output"] == serde_json::json!("")),
        "no call-phase emission for {first_tool_id}: {tools:?}"
    );
    assert!(
        tools
            .iter()
            .any(|t| { t.id == first_tool_id && t.payload["output"] != serde_json::json!("") }),
        "no result emission for {first_tool_id}: {tools:?}"
    );
    assert_eq!(tools[0].payload["name"], "bash");
    let queues: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::Queue { .. }))
        .collect();
    assert!(
        queues
            .iter()
            .all(|e| matches!(e, Event::Queue { items, .. } if items.is_empty())),
        "a starter send must not appear in the queue: {queues:?}"
    );
}
