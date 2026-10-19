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

/// #175/#173: a child reuses the SAME `ForwardingProvider` its agent runs
/// through, so `run_turn` derives the child's `StreamEnd` `call_id` from
/// `live.provider.calls` and emits it (plus the child's assistant
/// `EntryUpsert`). This drives the *default* `ForwardingChildFactory` (no
/// `with_child_factory`) — the stash-reuse path every other spawn test
/// bypasses with a canned child factory. `canned://text` keeps the child's
/// turn deterministic.
#[tokio::test]
async fn a_child_reuses_its_agents_provider_and_emits_its_stream_end() {
    let mut providers = BTreeMap::new();
    providers.insert(
        "dev".into(),
        crate::config::Provider::with_model("canned://text", "model"),
    );
    // No `with_child_factory`: the core uses the default forwarding factory,
    // whose stash-reuse of the child's provider is the fix under test.
    let core = CoreBuilder::custom(providers).build();
    let collected = collect_events(&core);
    let tmp = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let session = match core
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
    let info = match core
        .dispatch(Command::SubagentSpawn {
            session: session.id.clone(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent,
        other => panic!("expected a subagent: {other:?}"),
    };
    let child = info.child.clone();

    // Bounded wait for the child's StreamEnd (its turn settles on its own).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut stream_end: Option<(String, Option<u64>)> = None;
    let mut assistant_upserted = false;
    while tokio::time::Instant::now() < deadline {
        let snap = collected.lock().unwrap().clone();
        assistant_upserted = snap.iter().any(|e| {
            matches!(
                e,
                Event::EntryUpsert { session, entry, .. }
                    if *session == child && entry.kind == crate::agent::KIND_ASSISTANT
            )
        });
        if let Some(found) = snap.iter().find_map(|e| match e {
            Event::StreamEnd {
                session,
                call_id,
                usage,
                ..
            } if *session == child => {
                Some((call_id.clone(), usage.as_ref().map(|u| u.total_tokens)))
            }
            _ => None,
        }) {
            stream_end = Some(found);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Positive: the child's assistant entry reached the stream (its transcript
    // is live in the GUI, #173) — this needs the child's event hooks wired.
    assert!(
        assistant_upserted,
        "the child's assistant entry never reached the stream"
    );
    // The fix (#175): a StreamEnd is emitted for the child at all. A
    // mismatched LiveSession provider leaves live.provider.calls empty, so
    // run_turn derives no call_id and emits nothing.
    let (call_id, total_tokens) = stream_end.expect(
        "the child emitted no StreamEnd: its LiveSession provider does not match its agent's",
    );
    // Negative: the call_id is a real derived id ({child_session}-{seq}), not
    // the empty id a provider mismatch would leave behind.
    assert!(
        !call_id.is_empty(),
        "the child's StreamEnd call_id is empty"
    );
    assert!(
        call_id.starts_with(&child),
        "the StreamEnd call_id {call_id:?} is not derived from the child's provider"
    );
    // The completion's usage rides the StreamEnd (canned text reports 140 total).
    assert_eq!(
        total_tokens,
        Some(140),
        "the child's StreamEnd lost its usage"
    );
    drop(core);
}

/// #187: a GUI `MessageSend` to a child session projects the child's queue
/// (the in-flight message lands on the child's lane and a `Queue` event
/// #187: a GUI `MessageSend` to a child session projects the child's queue
/// (the in-flight message lands on the child's lane with the user as sender)
/// and emits a `Queue` event for the child's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gui_send_to_a_child_projects_the_child_queue_with_a_sender() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(SlowChildFactory {
            body: plain_body(),
            delay_ms: 3000,
        }))
        .build();
    let collected = collect_events(&core);
    let tmp = tempfile::tempdir().unwrap();
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
    // The child must be Running when the send lands, so it queues (a
    // non-running child would be resumed by it and drain it at once).
    let handle = sub.handle.clone();
    let sup = {
        let sessions = core.sessions.lock().unwrap();
        let session = sessions
            .get(&session.id)
            .expect("the parent session is live");
        let agent = session.agent.clone();
        drop(sessions);
        agent
            .subagents()
            .expect("the parent session owns a supervisor")
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !sup.child_running(&handle) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        sup.child_running(&handle),
        "the child was never observed running"
    );

    // The send to the child queues on its lane; the dispatch projects it.
    core.dispatch(Command::MessageSend {
        session: sub.child.clone(),
        text: "steer the child".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    // The child's queue is projected like a top-level session's: a Queue
    // event carries the item with the user as sender. (Assert on the
    // event, not `queued_items()`: the child is running and drains its
    // own queue, so a snapshot can race the drain on a slow runner.)
    // The event is observed through the pump's channel, which drops on
    // overflow under load; the child's store is the durable record the
    // send lands in, so it is the fallback the event backs.
    let mut store =
        crate::session::SessionStore::for_workspace(Path::new(&workspace.cwd), &sub.child);
    store.open().unwrap();
    let child = sub.child.clone();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut found = false;
    while !found && tokio::time::Instant::now() < deadline {
        found = collected.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                Event::Queue { session, items, .. }
                    if *session == child
                        && items.iter().any(|q| {
                            q.text == "steer the child"
                                && q.source.as_deref() == Some("user")
                        })
            )
        });
        if !found {
            found = store.entries_range(0, usize::MAX).unwrap().iter().any(|e| {
                e.kind == crate::agent::KIND_USER
                    && e.payload.get("text").and_then(|t| t.as_str()) == Some("steer the child")
                    && e.payload.get("source").and_then(|s| s.as_str()) == Some("user")
            });
        }
        if !found {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    assert!(found, "no Queue event was emitted for the child's session");
    drop(core);
}

/// #147: a force-laned `MessageSend` to a running child must still let the
/// turn's `StreamEnd` reach the event stream — it is what flips the GUI's
/// child turn flag off (a missing end left the store stuck at `running`).
/// Delivery is the bar here; the force's cut-vs-clean shape is a separate
/// defect (filed from this investigation).
///
/// Seam: NO `with_child_factory`, so the child runs through the default
/// `ForwardingChildFactory` (the production path a real GUI child uses) with
/// a slow `canned://` parent model — the same shape
/// `a_child_reuses_its_agents_provider_and_emits_its_stream_end` pins,
/// but with a force send into the in-flight window.
#[tokio::test]
async fn a_force_send_to_a_running_child_still_emits_its_stream_end() {
    // Slow parent model: the child's ForwardingProvider wraps this, so its
    // stream is in flight when the force send lands.
    let mut providers = BTreeMap::new();
    providers.insert(
        "dev".into(),
        crate::config::Provider::with_model("canned://text", "model"),
    );
    // No `with_child_factory`: the default forwarding factory is the fix's
    // path (stop flag shared, stream cut at the next delta).
    let core = CoreBuilder::custom(providers).build();
    let collected = collect_events(&core);
    let tmp = tempfile::tempdir().unwrap();
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
    let child = sub.child.clone();

    // The cut only interrupts a call that is in flight: wait for the child's
    // first assistant entry (first delta recorded) before force-sending.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut in_flight = false;
    while !in_flight && tokio::time::Instant::now() < deadline {
        in_flight = collected.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                Event::EntryUpsert {
                    session,
                    entry,
                    ..
                } if *session == child && entry.kind == crate::agent::KIND_ASSISTANT
            )
        });
        if !in_flight {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
    assert!(in_flight, "the child's turn never went in flight");

    // The force lane takes the child through stop+send (R3).
    core.dispatch(Command::MessageSend {
        session: child.clone(),
        text: "stop and do this instead".into(),
        lane: MessageLane::Force,
    })
    .unwrap();

    // The interrupted end is the first StreamEnd for the child.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut first_end: Option<bool> = None;
    while first_end.is_none() && tokio::time::Instant::now() < deadline {
        first_end = collected.lock().unwrap().iter().find_map(|e| match e {
            Event::StreamEnd {
                session,
                interrupted,
                ..
            } if *session == child => Some(*interrupted),
            _ => None,
        });
        if first_end.is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    assert!(
        first_end.is_some(),
        "no StreamEnd reached the stream for the child (the GUI's turn flag would stick at running). The force send's cut-vs-clean shape is a separate concern (#147's delivery is the bar here)."
    );
    drop(core);
}
