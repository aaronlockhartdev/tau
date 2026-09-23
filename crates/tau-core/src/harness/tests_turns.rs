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
                    | Event::StreamDelta { session, .. }
                    | Event::StreamEnd { session, .. }
                    | Event::ToolStart { session, .. }
                    | Event::ToolEnd { session, .. }
                    if *session == session_id
            )
        })
        .collect();
    // The stream was cut, not completed: the last stream event for the
    // session is an interrupted end, with no live activity after it.
    let Some(Event::StreamEnd { interrupted, .. }) = session_events.last().cloned() else {
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

/// A task command emits a task_changed event carrying the file's
/// folded task list — the GUI's tasks tab is event-driven (spec §8),
/// never polled; the payload is a projection of the file.
#[tokio::test]

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
                .filter(|e| matches!(e, Event::ToolEnd { .. }))
                .count()
                >= 1
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let events = collected.lock().unwrap().clone();

    // The stream: one start, deltas coalesced into one, one end with
    // the usage; the tool batch: a start/end pair; a starter send while
    // idle is the turn itself and never sits in the queue.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StreamStart { .. })),
        "no stream start: {events:?}"
    );
    let deltas = events
        .iter()
        .filter(|e| matches!(e, Event::StreamDelta { .. }))
        .count();
    assert!(
        deltas >= 1,
        "no stream deltas (coalesced burst): {events:?}"
    );
    let Some(Event::StreamDelta {
        text, reasoning, ..
    }) = events
        .iter()
        .find(|e| matches!(e, Event::StreamDelta { .. }))
    else {
        panic!("missing delta: {events:?}");
    };
    assert_eq!(text, "partial");
    assert_eq!(reasoning.as_deref(), Some("thinking"));
    let Some(Event::StreamEnd {
        interrupted, usage, ..
    }) = events.iter().find(|e| matches!(e, Event::StreamEnd { .. }))
    else {
        panic!("missing stream end: {events:?}");
    };
    assert!(!interrupted);
    assert_eq!(usage.map(|u| u.total_tokens), Some(15));
    assert!(
        events.iter().any(|e| matches!(e, Event::ToolStart { .. })),
        "no tool start: {events:?}"
    );
    let Some(Event::ToolEnd { name, .. }) =
        events.iter().find(|e| matches!(e, Event::ToolEnd { .. }))
    else {
        panic!("no tool end: {events:?}");
    };
    assert_eq!(name, "bash");
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
/// concurrent process(): the message is queued and the in-flight turn
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

/// The one live run (acceptance): a short session against the hosted
/// vLLM whose stream reaches the event pipe. Gated on TAU_LIVE and
/// skipped cleanly when the endpoint is unreachable (CI-safe).
#[tokio::test]
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
    };
    let body = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}}}}\n\n",
        text = "<observations>observed</observations>"
    );
    let provider = Arc::new(ForwardingProvider {
        inner: provider::canned(&body),
        tx: core.events_tx.clone(),
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
        tool_batch_on_force: Default::default(),
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
            queue: Mutex::new(Vec::new()),
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
