use super::*;

use crate::harness::testkit::*;

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
            base_prompt: None,
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

/// Child-targeted `TaskChanged` (the shared task model): a task assigned to
/// a child is emitted for the child's session as well — the child's pane
/// is a projection of the parent's store, and the child's own file is
/// task-free.
#[tokio::test]
async fn a_task_assigned_to_a_child_is_emitted_for_the_child_session() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(CannedChildFactory { body: done_body() }))
        .build();
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
    let collected = collect_events(&core);
    core.dispatch(Command::TaskCreate {
        session: session.id.clone(),
        title: "work".into(),
    })
    .unwrap();
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
    core.dispatch(Command::TaskAssign {
        session: session.id.clone(),
        task: "task-1".into(),
        worker: sub.handle.clone(),
    })
    .unwrap();
    // Both the parent's full list (worker set) and the child's projection
    // reach the pipe.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let evs = collected.lock().unwrap().clone();
        let for_parent = evs.iter().any(|e| {
            matches!(
                e,
                Event::TaskChanged {
                    session: s,
                    tasks,
                    ..
                }
                if *s == session.id
                    && tasks.iter().any(|t| t.id == "task-1"
                        && t.worker.as_ref().is_some_and(|w| w.session == sub.child))
            )
        });
        let for_child = evs.iter().any(|e| {
            matches!(
                e,
                Event::TaskChanged {
                    session: s,
                    tasks,
                    ..
                }
                if *s == sub.child
                    && tasks.len() == 1
                    && tasks[0].id == "task-1"
                    && tasks[0].worker.as_ref().is_some_and(|w| w.session == sub.child)
            )
        });
        if for_parent && for_child {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child-targeted task_changed never arrived: {evs:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
