use super::*;

use crate::harness::testkit::*;

/// A spawned child's observed `done` wakes the parent: the Notified
/// event reaches the stream, and the wake's message lands on the
/// parent's branch tagged with the child's session id.
#[tokio::test]
async fn a_spawned_child_finishes_and_wakes_the_parent() {
    // One scripted turn: parent_notify done with a structured output.
    let call_id = "c1".to_string();
    let args = json!({
        "text": "the work is done",
        "done": true,
        "output": { "result": "ok" },
    });
    let item = json!({
        "id": call_id,
        "type": "function_call",
        "name": "parent_notify",
        "call_id": call_id,
        "arguments": args.to_string(),
    });
    let data = format!("data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}");
    let body = format!("{data}\n\ndata: [DONE]\n\n");
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(CannedChildFactory { body }))
        .build();
    let mut rx = core.events_rx.lock().unwrap().take().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session: m } => m,
        other => panic!("expected a session: {other:?}"),
    };
    let info = match core
        .dispatch(Command::SubagentSpawn {
            session: session.id.clone(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent: i } => i,
        other => panic!("expected a subagent: {other:?}"),
    };
    // The observed lifecycle derives `done` from the child's plan file
    // (spec §5.2): seed a complete plan while the one-time pre-delay
    // holds the child's first call, so the declaration is confirmed.
    std::fs::write(
        crate::plan::path(tmp.path(), &info.child),
        "# Plan\n\n## Steps\n\n- [x] the work\n",
    )
    .unwrap();
    // The child's scripted done-notify must wake the parent: the
    // Notified event reaches the stream, and the wake's message lands
    // on the parent's branch tagged with the child's session id.
    let mut notified = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
            && let Event::SubagentEvent {
                kind: SubagentEventKind::Notified { wake, .. },
                ..
            } = ev
            && wake == "done"
        {
            notified = true;
            break;
        }
    }
    assert!(notified, "the parent was never woken by the child's done");
    let mut store = SessionStore::for_workspace(tmp.path(), &session.id);
    store.open().unwrap();
    let entries = store.entries_range(0, usize::MAX).unwrap();
    let woke = entries
        .iter()
        .any(|e| e.payload.get("source") == Some(&json!(info.child)));
    assert!(woke, "no notification entry on the parent's branch");
    // The child registered as an ordinary live session (the GUI can
    // open it like any session).
    assert!(
        core.sessions.lock().unwrap().get(&info.child).is_some(),
        "the child's live session was not registered"
    );
    drop(core);
}

/// #148: a child's done report landing while the parent's call is in flight
/// must not invert the parent's file. The report's entry is minted at queue
/// time (its upsert fires then, so the GUI shows the report now) but its
/// line is written at the turn loop's dequeue — after the in-flight
/// assistant line lands. File order stays equal to id order.
#[tokio::test]
async fn a_notified_steer_lands_in_id_order_while_the_parent_call_is_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    // The parent's first call hangs in prefill for 2 s (the in-flight
    // window); the child finishes in ~0.8 s, inside it.
    let mut providers = BTreeMap::new();
    providers.insert(
        "dev".into(),
        crate::config::Provider::with_model("canned://slow-first", "model"),
    );
    let core = CoreBuilder::custom(providers)
        .with_child_factory(Arc::new(SlowChildFactory {
            body: done_body(),
            delay_ms: 400,
        }))
        .build();
    let collected = collect_events(&core);
    let workspace = open_ws(&core, tmp.path()).await;
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session: m } => m,
        other => panic!("expected a session: {other:?}"),
    };
    let sub = match core
        .dispatch(Command::SubagentSpawn {
            session: session.id.clone(),
            agent_type: "general".into(),
            brief: "do it".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent,
        other => panic!("expected a subagent: {other:?}"),
    };
    core.dispatch(Command::MessageSend {
        session: session.id.clone(),
        text: "go".into(),
        lane: MessageLane::FollowUp,
    })
    .unwrap();
    // The steer is drained mid-turn: wait for the turn to start, then settle.
    let is_running = || {
        core.sessions
            .lock()
            .unwrap()
            .get(&session.id)
            .is_some_and(|l| l.turn.load(Ordering::SeqCst))
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !is_running() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(is_running(), "the parent's turn never started");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while is_running() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!is_running(), "the parent's turn never settled");

    // The parent's file: every entry line in strict id order.
    let mut store = SessionStore::for_workspace(tmp.path(), &session.id);
    store.open().unwrap();
    let entries = store.entries_range(0, usize::MAX).unwrap();
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "file line order diverges from id order: {ids:?}"
    );
    // The report is present, attributed to the child.
    assert!(
        entries.iter().any(|e| {
            e.kind == crate::agent::KIND_USER
                && e.payload["lane"] == "steering"
                && e.payload["source"] == json!(sub.child)
        }),
        "the steering report is missing from the file: {entries:?}"
    );
    // Its upsert fired at queue time (the GUI shows the report while the
    // parent's turn is in flight).
    let sid = session.id.clone();
    assert!(
        collected.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                Event::EntryUpsert { session, entry, .. }
                    if *session == sid
                        && entry.payload["source"] == json!(sub.child)
            )
        }),
        "no queue-time upsert for the steering report"
    );
    drop(core);
}
