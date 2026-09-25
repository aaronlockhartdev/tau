use super::*;

use crate::harness::testkit::*;

/// A refused archive must not mutate: with a running child, the
/// refusal fires before any quiescing, so the child stays running
/// (a stop is terminal — the old order left it stopped, review N4).
#[tokio::test]
async fn a_running_child_refusal_leaves_the_child_running() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(SlowChildFactory {
            body: plain_body(),
            delay_ms: 1200,
        }))
        .build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let info = match core
        .dispatch(Command::SubagentSpawn {
            session: parent.id.clone(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent,
        other => panic!("expected a subagent: {other:?}"),
    };
    // Wait for the child to be running in its supervisor.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let out = core
            .dispatch(Command::SubagentState {
                handle: info.handle.clone(),
            })
            .unwrap();
        let state = match out {
            CommandOutput::Subagent { subagent } => subagent.state,
            _ => String::new(),
        };
        if state == "running" {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("the child never started running (last: {state})");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let err = core
        .dispatch(Command::SessionArchive {
            session: parent.id.clone(),
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("is running") && msg.contains(&info.child),
        "the refusal names the running child: {msg}"
    );
    // The refusal fired before any quiescing: the child is still
    // running (a stop is terminal — the old order left it stopped).
    let out = core
        .dispatch(Command::SubagentState {
            handle: info.handle.clone(),
        })
        .unwrap();
    let state = match out {
        CommandOutput::Subagent { subagent } => subagent.state,
        other => panic!("expected a subagent: {other:?}"),
    };
    assert_eq!(
        state, "running",
        "the refusal must not have stopped the child"
    );
    // The parent is untouched: still live, file in place.
    assert!(
        core.sessions.lock().unwrap().contains_key(&parent.id),
        "the parent must not be left detached from the live map"
    );
    let root = Path::new(&workspace.cwd);
    assert!(
        root.join(".tau")
            .join("sessions")
            .join(format!("{}.jsonl", parent.id))
            .exists(),
        "the parent's live file is untouched"
    );
    // Let the child's scripted life end (the nudge exhausts and it
    // fails, waking the parent) before the core drops.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let out = core
            .dispatch(Command::SubagentState {
                handle: info.handle.clone(),
            })
            .unwrap();
        let state = match out {
            CommandOutput::Subagent { subagent } => subagent.state,
            _ => String::new(),
        };
        if state != "running" {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("the child never left running (last: {state})");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    drop(core);
}

/// A rename over the cap is refused (the header is read back bounded,
/// so an oversized title would silently drop the entry from the
/// archive list, review N3); the exact cap is accepted, and the
/// refusal writes nothing to the header.
#[tokio::test]

async fn an_archive_io_failure_keeps_the_session_live() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    // Kill the live file under the session: the archive's read fails.
    std::fs::remove_file(
        cwd.path()
            .join(".tau/sessions")
            .join(format!("{}.jsonl", session.id)),
    )
    .unwrap();
    let err = core
        .dispatch(Command::SessionArchive {
            session: session.id.clone(),
        })
        .unwrap_err();
    assert!(matches!(err, ProtocolError::Other { .. }), "{err:?}");
    assert!(
        core.sessions.lock().unwrap().contains_key(&session.id),
        "the session must not be left detached from the live map"
    );
    drop(core);
}

/// The archive's post-detach re-check (ADR-0005): a turn that CAS'd in
/// the window aborts the archive — the exact live session re-inserts,
/// and a refused archive leaves the archived flag unset. The check is
/// separable, so it runs directly on the preconditions the archive
/// leaves at that point (turn set, session detached by the archive).
#[tokio::test]

async fn an_archive_turn_started_in_the_window_aborts_with_reinsertion() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let id = live.meta.lock().unwrap().id.clone();
    // A wake that grabbed the Arc before the detach set the turn; the
    // detach removed the session from the live map.
    live.turn.store(true, Ordering::SeqCst);
    core.sessions.lock().unwrap().remove(&id);
    let err = core.archive_turn_recheck(&id, &live).unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("started a turn while archiving"),
        "the refusal names the race: {msg}"
    );
    // Exact re-insertion: the very Arc is back in the live map, and a
    // refused archive never sets the flag.
    {
        let map = core.sessions.lock().unwrap();
        assert!(
            Arc::ptr_eq(&map[&id], &live),
            "the same live session re-inserts"
        );
    }
    assert!(!live.meta.lock().unwrap().archived, "the flag stays unset");
}

/// A send to an archived session is refused before it appends
/// (ADR-0005): the live file stays gone — an append would have
/// recreated it headerless, and the restore would then have refused
/// it.
#[tokio::test]

async fn a_send_to_an_archived_session_is_refused() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    core.dispatch(Command::SessionArchive {
        session: session.id.clone(),
    })
    .unwrap();
    let err = core
        .dispatch(Command::MessageSend {
            session: session.id.clone(),
            text: "hello".into(),
            lane: MessageLane::Steering,
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("is archived") && msg.contains("restore"),
        "the refusal names the route: {msg}"
    );
    // The refusal mutated nothing: the live file is still absent, the
    // archive file is in place.
    let root = cwd.path();
    assert!(
        !root
            .join(".tau/sessions")
            .join(format!("{}.jsonl", session.id))
            .exists(),
        "a refused send does not recreate the live file"
    );
    assert!(
        root.join(".tau/archive")
            .join(format!("{}.jsonl.zst", session.id))
            .exists()
    );
    drop(core);
}

/// A child-archive I/O failure mid-archive leaves the half-archived
/// state live and consistent — the parent un-flagged in the live map,
/// the first child archived, the failed child untouched — and the next
/// archive + restore converges everything (review N2). The failure is
/// forced deterministically: the second child's archive target already
/// exists (a junk file that lists nowhere — no session header).
#[tokio::test]

async fn a_half_archived_session_converges_on_the_next_archive() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(CannedChildFactory { body: done_body() }))
        .build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let child_a = spawn_done_child(&core, &parent.id).await;
    let child_b = spawn_done_child(&core, &parent.id).await;
    // The children set iterates in id order: the junk target lands on
    // the later id, so the earlier child archives first.
    let (first, second) = if child_a.child < child_b.child {
        (child_a.child.clone(), child_b.child.clone())
    } else {
        (child_b.child.clone(), child_a.child.clone())
    };
    let archive_dir = cwd.path().join(".tau").join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(
        archive_dir.join(format!("{second}.jsonl.zst")),
        "not a session archive",
    )
    .unwrap();

    let err = core
        .dispatch(Command::SessionArchive {
            session: parent.id.clone(),
        })
        .unwrap_err();
    assert!(matches!(err, ProtocolError::Other { .. }), "{err:?}");
    // The half-archived state: the parent is live and un-flagged, the
    // first child archived, the failed child untouched.
    let flag = |id: &str| {
        core.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|l| l.meta.lock().unwrap().archived)
            .unwrap_or(false)
    };
    assert!(
        core.sessions.lock().unwrap().contains_key(&parent.id),
        "the parent is back in the live map"
    );
    assert!(
        !flag(&parent.id),
        "a refused archive leaves the parent's flag unset"
    );
    let root = cwd.path();
    assert!(
        root.join(".tau/sessions")
            .join(format!("{}.jsonl", parent.id))
            .exists(),
        "the parent's live file stays"
    );
    assert!(
        !root
            .join(".tau/sessions")
            .join(format!("{}.jsonl", first))
            .exists(),
        "the first child's file moved"
    );
    assert!(flag(&first), "the archived child stays archived");
    assert!(
        root.join(".tau/sessions")
            .join(format!("{}.jsonl", second))
            .exists(),
        "the failed child's file stays"
    );
    assert!(!flag(&second), "the failed child's flag reverts");

    // The next archive: clear the forced failure and it converges —
    // both children and the parent move.
    std::fs::remove_file(archive_dir.join(format!("{second}.jsonl.zst"))).unwrap();
    core.dispatch(Command::SessionArchive {
        session: parent.id.clone(),
    })
    .unwrap();
    for id in [&parent.id, &first, &second] {
        assert!(flag(id), "the archive flags {id}");
    }

    // And the restore brings the whole set back to the live path, every
    // flag cleared.
    core.dispatch(Command::SessionRestore {
        workspace: workspace.id.clone(),
        session: parent.id.clone(),
    })
    .unwrap();
    for id in [&parent.id, &first, &second] {
        assert!(
            root.join(".tau/sessions")
                .join(format!("{}.jsonl", id))
                .exists(),
            "the file of {id} is back"
        );
        assert!(
            !root
                .join(".tau/archive")
                .join(format!("{id}.jsonl.zst"))
                .exists(),
            "the archive of {id} is gone"
        );
        assert!(!flag(id), "the flag of {id} is cleared");
    }
    drop(core);
}

/// Closing a RUNNING child session ends it `Stopped` through the
/// parent's supervisor — not the bogus `failed` a bare stop flag would
/// leave (the child has no supervisor of its own, and its drive would
/// burn the one-shot nudge).
#[tokio::test]
async fn a_close_of_a_running_child_records_stopped_not_failed() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(SlowChildFactory {
            body: plain_body(),
            delay_ms: 1200,
        }))
        .build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let info = match core
        .dispatch(Command::SubagentSpawn {
            session: parent.id.clone(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent,
        other => panic!("expected a subagent: {other:?}"),
    };
    // Wait for the child to be running in its supervisor.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let out = core
            .dispatch(Command::SubagentState {
                handle: info.handle.clone(),
            })
            .unwrap();
        let state = match out {
            CommandOutput::Subagent { subagent } => subagent.state,
            _ => String::new(),
        };
        if state == "running" {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("the child never started running (last: {state})");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // Close the child's session directly.
    core.dispatch(Command::SessionClose {
        session: info.child.clone(),
    })
    .unwrap();
    // The child ends Stopped — never the bogus `failed`.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let out = core
            .dispatch(Command::SubagentState {
                handle: info.handle.clone(),
            })
            .unwrap();
        let state = match out {
            CommandOutput::Subagent { subagent } => subagent.state,
            _ => String::new(),
        };
        if matches!(state.as_str(), "stopped" | "failed" | "done") {
            assert_eq!(
                state, "stopped",
                "closing a running child records its stop, not a failure"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never reached a terminal state (last: {state})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    drop(core);
}
