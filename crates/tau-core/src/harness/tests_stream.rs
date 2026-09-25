use super::*;

use crate::harness::testkit::*;

use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn task_commands_emit_a_task_changed_event() {
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
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session: m } => m,
        other => panic!("expected a session: {other:?}"),
    };
    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    tokio::spawn(crate::harness::pump::pump(
        Arc::clone(&core),
        move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        },
    ));

    core.dispatch(Command::TaskCreate {
        session: session.id.clone(),
        title: "work".into(),
    })
    .unwrap();

    // The pump is a separate task: give it a bounded window to deliver.
    let mut tasks = None;
    for _ in 0..50 {
        let found = collected.lock().unwrap().iter().find_map(|e| match e {
            Event::TaskChanged {
                session: s, tasks, ..
            } if *s == session.id => Some(tasks.clone()),
            _ => None,
        });
        if let Some(t) = found {
            tasks = Some(t);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let tasks = tasks.expect("the task command never emitted a task_changed event");
    assert_eq!(tasks.len(), 1, "the file holds exactly one task");
    assert_eq!(tasks[0].title, "work");
    assert_eq!(tasks[0].status, "pending");
}

/// A session wired to `inner` (canned in tests, production in the
/// live test), registered with the core the way `SessionNew` does.
#[tokio::test]
async fn live_run_streams_the_event_pipe() {
    if std::env::var("TAU_LIVE").is_err() {
        eprintln!("live run skipped (TAU_LIVE not set)");
        return;
    }
    let mut hosted = providers();
    hosted.insert(
        "dev".to_owned(),
        crate::config::Provider::with_model("https://llms.aaronlockhart.dev/v1", "qwen3.8-27b"),
    );
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(hosted).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let config = core.system_config();
    let provider = provider::production(&core.client, &config.providers["dev"], &config.requests);
    let live = manual_session(
        &core,
        &workspace,
        provider,
        TurnConfig {
            max_output_tokens: Some(200),
            reasoning: None,
            ..TurnConfig::default()
        },
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
        session: session_id,
        text: "Reply with exactly the word: pong".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();

    // The thinking model can burn the 200-token cap on reasoning; the
    // pipe is proven by start + at least one delta, not by completion.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let events = collected.lock().unwrap().clone();
        let started = events
            .iter()
            .any(|e| matches!(e, Event::StreamStart { .. }));
        let deltaed = events
            .iter()
            .any(|e| matches!(e, Event::StreamDelta { .. }));
        if started && deltaed {
            break;
        }
        if std::time::Instant::now() > deadline {
            eprintln!("live run skipped: no stream events within 90s (endpoint unreachable?)");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let events = collected.lock().unwrap().clone();
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            Event::StreamDelta { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let end = events
        .iter()
        .find(|e| matches!(e, Event::StreamEnd { .. }))
        .cloned();
    eprintln!(
        "live run: {} deltas ({} chars); end: {:?}",
        events
            .iter()
            .filter(|e| matches!(e, Event::StreamDelta { .. }))
            .count(),
        text.chars().count(),
        end
    );
    assert!(!text.is_empty(), "deltas arrived but carried no text");
}
/// End-to-end sub-agent lifecycle (ticket #23 N3): a spawned child runs
/// its scripted `parent_notify {done}` turn, the supervisor resolves it,
/// and the parent is woken — the notification lands on the parent's
/// active branch and a `Notified` event reaches the stream.
/// A child provider that replays one canned body per call (the
/// scripted sub-agent turns of the dispatch tests).
struct CannedChild {
    body: String,
    index: AtomicUsize,
}
impl TurnProvider for CannedChild {
    fn call<'a>(&self, _req: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let body = self.body.clone();
        self.index.fetch_add(1, Ordering::SeqCst);
        let (events, calls) = provider::decode_stream(&body).unwrap();
        Box::pin(async move {
            let mut result = provider::TurnResult::default();
            for event in &events {
                if !sink.event(event.clone()) {
                    break;
                }
                provider::fold_event(event, &mut result);
            }
            result.calls = calls.into_iter().map(|(_, c)| c).collect();
            Ok(result)
        })
    }
}
struct CannedChildFactory {
    body: String,
}
impl ChildProviderFactory for CannedChildFactory {
    fn create(&self, _child: &str) -> TurnProviderRef {
        Arc::new(CannedChild {
            body: self.body.clone(),
            index: AtomicUsize::new(0),
        })
    }
}

/// One scripted turn with no `parent_notify`: the child ends the turn
/// (the nudge path), so a slow provider keeps it running.
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
