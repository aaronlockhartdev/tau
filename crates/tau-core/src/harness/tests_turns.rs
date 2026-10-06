use super::*;

use crate::harness::testkit::*;

#[tokio::test]
async fn closing_a_session_stops_its_in_flight_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
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
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );

    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });

    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "first".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // In flight: the first deltas have landed.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    core.dispatch(Command::SessionClose {
        session: session_id.clone(),
    })
    .unwrap();

    // The sink cuts the stream at the next delta (≤ 600 ms); the turn
    // then winds down — give it room to finish.
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
    let events = collected.lock().unwrap().clone();
    let session_events: Vec<&Event> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::StreamStart { session, .. }
                    | Event::StreamEnd { session, .. }
                    | Event::EntryUpsert { session, .. }
                    if *session == session_id
            )
        })
        .collect();
    // The stream was cut, not completed: the last stream event for the
    // session is an interrupted end, with no live activity after it.
    let Some(Event::StreamEnd { interrupted, .. }) = session_events.last().copied() else {
        panic!("the closed session's stream never closed: {session_events:?}");
    };
    assert!(interrupted, "the close did not cut the stream");
    assert!(
        session_events
            .iter()
            .any(|e| matches!(e, Event::StreamStart { .. })),
        "no stream ever started"
    );
}

/// A task command emits a `task_changed` event carrying the file's
/// folded task list — the GUI's tasks tab is event-driven (spec §8),
/// never polled; the payload is a projection of the file.
#[tokio::test]
#[allow(clippy::too_many_lines)] // one end-to-end turn flow; splitting is refactoring
async fn the_event_pipe_carries_a_canned_turn_to_the_sink() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
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
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });

    // The meta guard must not live across this await: the dispatch
    // path re-locks the same mutex (emit_queue), and a std mutex is
    // not reentrant.
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id,
        text: "do the thing".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // The canned turn is fast; give the pump a bounded time to flush.
    for _ in 0..500 {
        let events = collected.lock().unwrap().clone();
        if events.iter().any(|e| matches!(e, Event::StreamEnd { .. }))
            && events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        Event::EntryUpsert { entry, .. } if entry.kind == "tool"
                    )
                })
                .count()
                >= 2
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let events = collected.lock().unwrap().clone();

    // The stream: one start, the growing assistant entry (frame-aligned
    // snapshots), one end with the usage; the tool batch: one id, re-emitted
    // call → result; a starter send while idle is the turn itself and never
    // sits in the queue.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StreamStart { .. })),
        "no stream start: {events:?}"
    );
    let Some(Event::EntryUpsert { entry: first, .. }) = events.iter().find(|e| {
        matches!(
            e,
            Event::EntryUpsert { entry, .. } if entry.kind == "assistant"
        )
    }) else {
        panic!("no assistant upsert: {events:?}");
    };
    assert_eq!(first.payload["text"], "partial");
    // The final emission is the file line: it carries the calls.
    let Some(Event::EntryUpsert {
        entry: final_assistant,
        ..
    }) = events.iter().rev().find(|e| {
        matches!(
            e,
            Event::EntryUpsert { entry, .. } if entry.kind == "assistant"
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
            Event::EntryUpsert { entry, .. } if entry.kind == "tool" => Some(entry),
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

/// A send that lands while a turn is in flight must not spawn a second
/// concurrent `process()`: the message is queued and the in-flight turn
/// absorbs it (spec §7/§8 single writer, review B1).
#[tokio::test]

async fn a_mid_turn_send_is_queued_not_a_second_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    // The slow stream (~2.4 s in flight per call) is the in-flight
    // window the second send lands in.
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );

    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });

    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "first".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // Mid-stream of call 1: the first deltas have arrived, the stream
    // is still going (call 1 ends near t=2.4 s).
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    core.dispatch(Command::MessageSend {
        session: session_id,
        text: "second".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let events = collected.lock().unwrap().clone();
    let starts = events
        .iter()
        .filter(|e| matches!(e, Event::StreamStart { .. }))
        .count();
    assert_eq!(
        starts, 1,
        "a mid-turn send spawned a second turn ({starts} stream starts): {events:?}"
    );
    // The second message sits in the GUI's queue state, waiting for the
    // in-flight process() to deliver it — it was not consumed by a
    // second turn (the starter stays listed until the post-turn
    // reconciliation, so both are present).
    let last_queue = events
        .iter()
        .rev()
        .find(|e| matches!(e, Event::Queue { .. }))
        .cloned();
    let Some(Event::Queue { items, .. }) = last_queue else {
        panic!("no queue state after the second send: {events:?}")
    };
    assert!(
        items.iter().any(|i| i.text == "second"),
        "the mid-turn message must remain queued: {items:?}"
    );
}

/// A canned SSE stream forwarded through the real event pipe —
/// deterministic, no network, runs in CI.
#[tokio::test]
#[allow(clippy::too_many_lines)] // one end-to-end turn flow; splitting is refactoring
async fn a_live_om_run_emits_om_status_events() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, tmp.path()).await;
    let cwd = PathBuf::from(&workspace.cwd);
    let mut store = SessionStore::for_workspace(&cwd, &SessionStore::new_session_id());
    store.create().unwrap();
    let sid = store.id().to_owned();
    let created = store.created();
    let om = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1,
        reflect_threshold: 40_000,
        buffer_increment: 1,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let body = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}}}}\n\n",
        text = "<observations>observed</observations>"
    );
    let provider = Arc::new(ForwardingProvider {
        inner: provider::canned(&body),
        tx: core.events_tx.clone(),
        pipe: core.pipe.clone(),
        workspace: workspace.id.clone(),
        session: sid.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        call_seq: AtomicU64::new(0),
        calls: Arc::new(Mutex::new(Vec::new())),
        completed: Arc::new(Mutex::new(HashMap::new())),
    });
    let agent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "model".into(),
        tools: tools::agent_tool_specs(),
        cwd: cwd.clone(),
        provider: provider.clone(),
        tool_batch_on_force: crate::config::ToolBatchPolicy::default(),
        turn: TurnConfig::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &om,
            crate::om::OmRecord::default(),
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    {
        let tx = core.events_tx.clone();
        let ws = workspace.id.clone();
        let csid = sid.clone();
        agent.set_om_status_hook(Some(Arc::new(move |kind: &str| {
            let _ = tx.try_send(Event::OmStatus {
                workspace: ws.clone(),
                session: csid.clone(),
                kind: match kind {
                    "observing" => OmStatusKind::Observing,
                    "reflecting" => OmStatusKind::Reflecting,
                    _ => OmStatusKind::Idle,
                },
            });
        })));
    }
    core.sessions.lock().unwrap().insert(
        sid.clone(),
        Arc::new(LiveSession {
            meta: Mutex::new(SessionMeta {
                id: sid.clone(),
                workspace: workspace.id.clone(),
                title: None,
                parent: None,
                created,
                model: Some("model".into()),
                leaf: None,
                usage: None,
                archived: false,
            }),
            agent,
            stop: Arc::new(AtomicBool::new(false)),
            turn: AtomicBool::new(false),
            provider,
            cwd,
        }),
    );
    core.dispatch(Command::MessageSend {
        session: sid,
        text: "hello".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let mut kinds = Vec::new();
    let mut events = core.events();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    while kinds.len() < 2 {
        let left = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .expect("the om_status events did not arrive in time");
        let ev = tokio::time::timeout(left, events.recv())
            .await
            .expect("the om_status events did not arrive in time")
            .expect("the event pipe closed");
        if let Event::OmStatus { kind, .. } = ev {
            kinds.push(kind);
        }
    }
    assert_eq!(kinds, vec![OmStatusKind::Observing, OmStatusKind::Idle]);
}

/// A send to a session whose file no longer opens (corrupted) must not
/// vanish: the turn start fails visibly (a system error), and the
/// accepted message is kept in the GUI's queue — it also stays in the
/// agent's queue, so the next successful turn delivers it and the
/// post-turn reconciliation removes it by text.
#[tokio::test]
async fn a_send_to_a_corrupted_session_fails_visibly_and_keeps_the_message() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    // Corrupt the file: open() now refuses it.
    std::fs::write(
        Path::new(&workspace.cwd)
            .join(".tau")
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
        "{\"type\":\"not-a-session\"}\n",
    )
    .unwrap();
    let collected = collect_events(&core);
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "do the thing".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let events = collected.lock().unwrap().clone();
        let surfaced = events
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::System {
                    kind: SystemEventKind::Error { message },
                    ..
                } => Some(message.clone()),
                _ => None,
            })
            .is_some_and(|m| m.contains("cannot be opened"));
        if surfaced {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no open-failure error surfaced"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // The accepted message stays in the agent's queue — the GUI's queue
    // is its projection, so the projection shows it.
    let items = live.agent.queued_items();
    assert!(
        items.iter().any(|i| i.text == "do the thing"),
        "the accepted message must stay queued: {items:?}"
    );
    // The turn flag is clear: the session is not stuck running.
    assert!(!live.turn.load(Ordering::SeqCst));
    drop(core);
}

/// A pipe overflow is never silent: the drop is counted, and a summary
/// system error reaches the GUI. The summary cannot ride the same full
/// channel, so it is owed and delivered by the next successful send —
/// the first moment the pipe has room again.
#[tokio::test]
async fn a_pipe_overflow_is_counted_and_surfaced() {
    let core = CoreBuilder::custom(providers()).build();
    let mut rx = core.events();
    // Flood the bounded pipe (capacity 1024) with no reader: every
    // event past the capacity is a drop.
    for i in 0..2048 {
        core.emit(Event::System {
            workspace: "w".into(),
            session: None,
            kind: SystemEventKind::WorkspaceOpened {
                name: "w".into(),
                cwd: format!("/w/{i}"),
            },
        });
    }
    let dropped = core.pipe.dropped.load(Ordering::Relaxed);
    assert!(dropped > 0, "overflows must be counted, got {dropped}");
    // Drain the pipe, then let the next successful send deliver the
    // owed summary.
    while rx.try_recv().is_ok() {}
    core.emit(Event::System {
        workspace: "w".into(),
        session: None,
        kind: SystemEventKind::WorkspaceOpened {
            name: "w".into(),
            cwd: "/w/after".into(),
        },
    });
    let mut surfaced = false;
    while let Ok(ev) = rx.try_recv() {
        surfaced |= matches!(
            &ev,
            Event::System {
                kind: SystemEventKind::Error { message },
                ..
            } if message.contains("event pipe overflow")
        );
    }
    assert!(surfaced, "an overflow must emit a visible summary");
    drop(core);
}

/// Issue #82: the post-turn reconciliation must not emit an interrupted
/// `StreamEnd` for a call that completed without producing an entry.
///
/// The verified symptom: the OM observer call rides the session's
/// forwarding provider (so it registers a call id) but its response never
/// becomes a journal entry. Pre-fix, the empty-partial reconciliation arm
/// treated every entry-less call as cut and emitted an interrupted
/// `StreamEnd` that made the ACP settle rule answer `Cancelled` for
/// completed runs. The fix
/// distinguishes cut (no Completed frame) from completed-empty (Completed
/// frame seen) via the `completed` map, and emits each call's end once,
/// at turn settlement. This test pins both halves: a continuation send
/// after the first `StreamEnd` starts its own turn, and no event stream in
/// the run carries an interrupted end.
#[allow(clippy::too_many_lines)] // one end-to-end scenario; splitting is refactoring
#[tokio::test]
async fn a_send_after_the_final_stream_end_starts_a_new_turn() {
    use std::fmt::Write as _;
    // A text-only body long enough to cross the observe threshold in one
    // turn (4000 chars >> 100 tokens under any counting heuristic).
    let text = "x".repeat(4000);
    let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    let _ = write!(
        body,
        "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n"
    );
    let _ = write!(
        body,
        "data: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":500,\"output_tokens\":200,\"total_tokens\":700}}}}}}\n\n"
    );

    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    // Observe fires on the first turn (the 4000-char reply is ~1000
    // tokens at the chars/4 estimator); reflect stays below the
    // observation count so the pass is one observer round-trip per
    // settle — the window the fix closes. 60 ms between events keeps the
    // window wide enough to land a send in it.
    let om = crate::om_integration::OmState::from_config(
        &crate::config::Om {
            om_model: String::new(),
            observe_threshold: 100,
            reflect_threshold: 200,
            buffer_increment: 50,
            buffer_tokens: 0,
            retries: crate::config::OmRetries::default(),
        },
        crate::om::OmRecord::default(),
    );
    let live = manual_session_om(
        &core,
        &workspace,
        provider::canned_slow(&body, 60),
        TurnConfig::default(),
        Some(om),
    );

    // The collector stands in for the ACP pump: everything it sees is
    // what the settle rule acts on.
    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });

    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "do the thing".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // The settle rule fires on the first `StreamEnd`: dispatch the
    // continuation at that moment, exactly as the headless loop does.
    for _ in 0..1000 {
        let seen_end = collected
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::StreamEnd { .. }));
        if seen_end {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "continue".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();

    // Wait for the nudge turn to complete (2 StreamEnds total — one per
    // turn; the OM call no longer crosses the forwarding seam, ticket
    // #86) so the full event stream is in hand.
    for _ in 0..2000 {
        let ends = collected
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, Event::StreamEnd { .. }))
            .count();
        if ends >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = collected.lock().unwrap().clone();
    let assistants = events
        .iter()
        .filter_map(|e| match e {
            Event::EntryUpsert { entry, .. } if entry.kind == "assistant" => Some(entry.id.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        assistants.len() >= 2,
        "the continuation turn produced no assistant entry (force-absorbed)"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::StreamEnd {
                interrupted: true,
                ..
            }
        )),
        "an interrupted StreamEnd: the continuation was cut, not a new turn"
    );
}

/// Serves its scripted body for the first `ok` calls, then fails every
/// later call with `make_error` — the shape of a transient provider
/// failure hitting the turn-end OM call (ticket #86 P1). `served` is
/// shared with the test for attempt counting.
struct FailingAfter {
    inner: provider::TurnProviderRef,
    ok: usize,
    served: Arc<std::sync::atomic::AtomicUsize>,
    make_error: fn() -> provider::ProviderError,
}

impl provider::TurnProvider for FailingAfter {
    fn call<'a>(
        &self,
        request: &provider::ResponseRequest,
        sink: &'a mut dyn provider::TurnSink,
    ) -> provider::ProviderTurn<'a> {
        let n = self
            .served
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < self.ok {
            return self.inner.call(request, sink);
        }
        let make_error = self.make_error;
        Box::pin(async move { Err(make_error()) })
    }
}

/// The turn-end OM pass fails transiently and exhausts its configured
/// retry: the turn must still settle cleanly — no `AgentError`, and no
/// interrupted `StreamEnd` anywhere in the event stream (the #82 class,
/// which the inner-provider routing structurally excludes; ticket #86 P1).
#[tokio::test]
async fn a_failed_om_pass_does_not_kill_the_turn() {
    // A text-only body long enough to cross the observe threshold in one
    // turn (4000 chars >> 100 tokens under any counting heuristic).
    let text = "x".repeat(4000);
    let body = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":500,\"output_tokens\":200,\"total_tokens\":700}}}}}}\n\n"
    );
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    // The reply (~1000 tokens) crosses the 100-token observe threshold,
    // so the turn-end pass fires one observer round-trip: the wrapper's
    // second call, which idle-times-out and exhausts its single retry.
    let om = crate::om_integration::OmState::from_config(
        &crate::config::Om {
            om_model: String::new(),
            observe_threshold: 100,
            reflect_threshold: 200,
            buffer_increment: 50,
            buffer_tokens: 0,
            retries: crate::config::OmRetries {
                observe: 1,
                buffer: 0,
                reflect: 0,
            },
        },
        crate::om::OmRecord::default(),
    );
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failing: provider::TurnProviderRef = Arc::new(FailingAfter {
        inner: provider::canned(&body),
        ok: 1,
        served: served.clone(),
        make_error: || provider::ProviderError::IdleTimeout,
    });
    let live = manual_session_om(&core, &workspace, failing, TurnConfig::default(), Some(om));

    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });

    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "do the thing".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // The turn settles (one `StreamEnd`; the OM call no longer crosses
    // the forwarding seam, so it adds no end of its own).
    for _ in 0..3000 {
        let done = collected
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::StreamEnd { .. }));
        if done {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = collected.lock().unwrap().clone();

    // Three calls: the main turn call, then the two OM attempts
    // (first failure + the single configured retry).
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 3);
    // The turn completed: its assistant entry is in the stream, and no
    // event anywhere is an interrupted end.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::EntryUpsert { entry, .. }
            if entry.kind == "assistant")),
        "the turn's assistant entry is missing: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::StreamEnd {
                interrupted: true,
                ..
            }
        )),
        "a failed OM pass surfaced as an interrupted turn: {events:?}"
    );
}

/// A non-transient OM failure (a 4xx) makes exactly one attempt — the
/// configured 8 retries do not apply — and the turn settles the same
/// way (ticket #86 P1).
#[tokio::test]
async fn a_non_transient_om_failure_makes_no_retry_attempts() {
    let text = "x".repeat(4000);
    let body = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":500,\"output_tokens\":200,\"total_tokens\":700}}}}}}\n\n"
    );
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let om = crate::om_integration::OmState::from_config(
        &crate::config::Om {
            om_model: String::new(),
            observe_threshold: 100,
            reflect_threshold: 200,
            buffer_increment: 50,
            buffer_tokens: 0,
            retries: crate::config::OmRetries {
                observe: 8,
                buffer: 0,
                reflect: 0,
            },
        },
        crate::om::OmRecord::default(),
    );
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failing: provider::TurnProviderRef = Arc::new(FailingAfter {
        inner: provider::canned(&body),
        ok: 1,
        served: served.clone(),
        make_error: || provider::ProviderError::Status {
            status: 400,
            body: String::new(),
        },
    });
    let live = manual_session_om(&core, &workspace, failing, TurnConfig::default(), Some(om));

    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });

    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "do the thing".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    for _ in 0..3000 {
        let done = collected
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::StreamEnd { .. }));
        if done {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = collected.lock().unwrap().clone();

    // Exactly one OM attempt: a 4xx is permanent, retries do not apply.
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::StreamEnd {
                interrupted: true,
                ..
            }
        )),
        "a failed OM pass surfaced as an interrupted turn: {events:?}"
    );
}
