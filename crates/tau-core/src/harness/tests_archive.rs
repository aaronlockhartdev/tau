use super::*;

use crate::harness::testkit::*;

/// The archive command: the file moves to `archive/` (ADR-0005), the
/// metadata carries the flag, `session_list` lists it flagged, and a
/// running session refuses (the archive is off the live write path).
#[tokio::test]
async fn a_session_archives_and_lists_with_the_flag() {
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
    // A session in flight refuses: the archive would tear the file.
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let err = core
        .dispatch(Command::SessionArchive {
            session: session_id.clone(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "a running session must refuse: {err:?}"
    );
    // Let the turn end, then archive.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while core
        .sessions
        .lock()
        .unwrap()
        .get(&session_id)
        .is_some_and(|l| l.turn.load(Ordering::SeqCst))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let out = core
        .dispatch(Command::SessionArchive {
            session: session_id.clone(),
        })
        .unwrap();
    let meta = match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(meta.archived, "the archive flag rides the metadata");
    let root = Path::new(&workspace.cwd);
    assert!(
        !root
            .join(".tau")
            .join("sessions")
            .join(format!("{session_id}.jsonl"))
            .exists(),
        "the live file is gone"
    );
    assert!(
        root.join(".tau")
            .join("archive")
            .join(format!("{session_id}.jsonl.zst"))
            .exists(),
        "the file is in the archive dir"
    );
    // The list carries the flagged session (the live copy wins over
    // the disk listing).
    let list = match core
        .dispatch(Command::SessionList {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Sessions { sessions } => sessions,
        other => panic!("expected sessions: {other:?}"),
    };
    let listed = list
        .iter()
        .find(|m| m.id == session_id)
        .expect("the archived session still lists");
    assert!(listed.archived, "the list carries the flag");
    // A second archive is refused: the archive already exists.
    let err = core
        .dispatch(Command::SessionArchive {
            session: session_id,
        })
        .unwrap_err();
    assert!(matches!(err, ProtocolError::Other { .. }), "{err:?}");
    // The fresh (idle) session archives on the same path.
    let out = core
        .dispatch(Command::SessionArchive {
            session: session.id,
        })
        .unwrap();
    match out {
        CommandOutput::Session { session } => assert!(session.archived),
        other => panic!("expected a session: {other:?}"),
    }
}

/// One scripted turn: parent_notify done with a structured output.
#[tokio::test]
async fn a_child_session_refuses_a_direct_archive() {
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
    let info = spawn_done_child(&core, &parent.id).await;
    let err = core
        .dispatch(Command::SessionArchive {
            session: info.child.clone(),
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("sub-agent") && msg.contains("archive its parent"),
        "the refusal points at the parent: {msg}"
    );
    assert!(
        Path::new(&workspace.cwd)
            .join(".tau")
            .join("sessions")
            .join(format!("{}.jsonl", info.child))
            .exists(),
        "the child's live file is untouched"
    );
    drop(core);
}

/// Archiving the parent archives all its sub-agent children with it
/// (ADR-0005): each child's file moves to archive/ and the list flags
/// it, the parent link kept so the GUI groups it under the parent.
#[tokio::test]

async fn a_parent_archive_archives_its_children() {
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
    let info = spawn_done_child(&core, &parent.id).await;
    let out = core
        .dispatch(Command::SessionArchive {
            session: parent.id.clone(),
        })
        .unwrap();
    let meta = match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(
        meta.archived,
        "the parent's archive flag rides its metadata"
    );
    let root = Path::new(&workspace.cwd);
    for id in [&parent.id, &info.child] {
        assert!(
            !root
                .join(".tau")
                .join("sessions")
                .join(format!("{id}.jsonl"))
                .exists(),
            "the live file of {id} is gone"
        );
        assert!(
            root.join(".tau")
                .join("archive")
                .join(format!("{id}.jsonl.zst"))
                .exists(),
            "the file of {id} is in the archive dir"
        );
    }
    let list = match core
        .dispatch(Command::SessionList {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Sessions { sessions } => sessions,
        other => panic!("expected sessions: {other:?}"),
    };
    let listed = list
        .iter()
        .find(|m| m.id == info.child)
        .expect("the archived child still lists");
    assert!(listed.archived, "the list carries the child's flag");
    assert_eq!(
        listed.parent.as_deref(),
        Some(parent.id.as_str()),
        "the child's parent link survives the archive"
    );
    drop(core);
}

/// A child already closed at archive time has a stable file: the
/// parent's archive moves it to archive/ too (the old hard-fail path —
/// archiving a child whose parent was closed — no longer exists).
#[tokio::test]

async fn a_parent_archive_archives_a_closed_child() {
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
    let info = spawn_done_child(&core, &parent.id).await;
    core.dispatch(Command::SessionClose {
        session: info.child.clone(),
    })
    .unwrap();
    let out = core
        .dispatch(Command::SessionArchive {
            session: parent.id.clone(),
        })
        .unwrap();
    let meta = match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(
        meta.archived,
        "the parent's archive flag rides its metadata"
    );
    let root = Path::new(&workspace.cwd);
    assert!(
        !root
            .join(".tau")
            .join("sessions")
            .join(format!("{}.jsonl", info.child))
            .exists(),
        "the closed child's live file is gone"
    );
    assert!(
        root.join(".tau")
            .join("archive")
            .join(format!("{}.jsonl.zst", info.child))
            .exists(),
        "the closed child's file is in the archive dir"
    );
    drop(core);
}

/// Restore round-trip: a parent and the children archived with it come
/// back to sessions/ (decompressed, flags cleared, parent link kept),
/// and the parent can archive again — the cycle is repeatable.
#[tokio::test]

async fn a_restore_round_trips_a_parent_and_its_children() {
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
    let info = spawn_done_child(&core, &parent.id).await;
    core.dispatch(Command::SessionArchive {
        session: parent.id.clone(),
    })
    .unwrap();
    // Restore: the parent's file and the child's come back, and the
    // response meta carries the cleared flag.
    let out = core
        .dispatch(Command::SessionRestore {
            workspace: workspace.id.clone(),
            session: parent.id.clone(),
        })
        .unwrap();
    let meta = match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(!meta.archived, "the restore clears the parent's flag");
    let root = Path::new(&workspace.cwd);
    for id in [&parent.id, &info.child] {
        assert!(
            root.join(".tau")
                .join("sessions")
                .join(format!("{id}.jsonl"))
                .exists(),
            "the file of {id} is back in sessions/"
        );
        assert!(
            !root
                .join(".tau")
                .join("archive")
                .join(format!("{id}.jsonl.zst"))
                .exists(),
            "the archive of {id} is removed"
        );
    }
    let list = match core
        .dispatch(Command::SessionList {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Sessions { sessions } => sessions,
        other => panic!("expected sessions: {other:?}"),
    };
    let listed = list
        .iter()
        .find(|m| m.id == info.child)
        .expect("the restored child still lists");
    assert!(!listed.archived, "the list clears the child's flag");
    assert_eq!(
        listed.parent.as_deref(),
        Some(parent.id.as_str()),
        "the child's parent link survives the round trip"
    );
    // The cycle repeats: archive the parent again (the child goes with
    // it), then restore again.
    core.dispatch(Command::SessionArchive {
        session: parent.id.clone(),
    })
    .unwrap();
    assert!(
        root.join(".tau")
            .join("archive")
            .join(format!("{}.jsonl.zst", info.child))
            .exists(),
        "the child archives with the parent on the second cycle"
    );
    core.dispatch(Command::SessionRestore {
        workspace: workspace.id.clone(),
        session: parent.id.clone(),
    })
    .unwrap();
    // Restoring a session that has no archive is refused.
    let err = core
        .dispatch(Command::SessionRestore {
            workspace: workspace.id.clone(),
            session: parent.id,
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(msg.contains("no archive"), "{msg}");
    drop(core);
}

/// The archive for a session not in the live map (never opened): a pure
/// file operation (ADR-0005) — the file, its children's files, and the
/// list flag all converge; a child id is still refused outright.
#[tokio::test]
async fn a_never_opened_session_archives_from_disk() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    // Two roots, the second with a child — all on disk only, none in
    // the live map.
    let parent = SessionStore::new_session_id();
    let child = SessionStore::new_session_id();
    let second = SessionStore::new_session_id();
    let second_child = SessionStore::new_session_id();
    {
        let mut p = SessionStore::for_workspace(Path::new(&workspace.cwd), &parent);
        p.create().unwrap();
        let mut c = SessionStore::for_workspace(Path::new(&workspace.cwd), &child);
        c.create().unwrap();
        c.set_parent(&parent).unwrap();
        let mut s = SessionStore::for_workspace(Path::new(&workspace.cwd), &second);
        s.create().unwrap();
        let mut sc = SessionStore::for_workspace(Path::new(&workspace.cwd), &second_child);
        sc.create().unwrap();
        sc.set_parent(&second).unwrap();
    }
    // The child is refused: it archives with its parent (ADR-0005).
    let err = core
        .dispatch(Command::SessionArchive {
            session: child.clone(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "a child archive is refused: {err:?}"
    );
    // The root archives from disk, the child with it.
    let out = core
        .dispatch(Command::SessionArchive {
            session: parent.clone(),
        })
        .unwrap();
    let meta = match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(meta.archived, "the archive flag rides the response meta");
    let root = Path::new(&workspace.cwd);
    for (id, role) in [(parent.as_str(), "the root"), (child.as_str(), "the child")] {
        assert!(
            !root
                .join(".tau")
                .join("sessions")
                .join(format!("{id}.jsonl"))
                .exists(),
            "{role}: the live file is gone"
        );
        assert!(
            root.join(".tau")
                .join("archive")
                .join(format!("{id}.jsonl.zst"))
                .exists(),
            "{role}: the file is in the archive"
        );
    }
    // The list converges: both flagged, the child's parent link intact.
    let list = match core
        .dispatch(Command::SessionList {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Sessions { sessions } => sessions,
        other => panic!("expected sessions: {other:?}"),
    };
    let m = list
        .iter()
        .find(|m| m.id == child)
        .expect("the child is listed");
    assert!(m.archived, "the child archives with its parent");
    assert!(m.parent.as_deref() == Some(parent.as_str()));
    // The other root, also never opened, archives the same way.
    let out = core
        .dispatch(Command::SessionArchive {
            session: second.clone(),
        })
        .unwrap();
    let meta = match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    assert!(meta.archived);
    assert!(
        root.join(".tau")
            .join("archive")
            .join(format!("{second_child}.jsonl.zst"))
            .exists(),
        "the second child archives with its parent"
    );
    drop(core);
}

/// A delete for a session not in the live map (archived, disk-only): a pure
/// file op that removes the target and its descendants from the archive dir,
/// and the list converges (the disk-only twin of the archive test above).
#[tokio::test]
async fn a_never_opened_session_deletes_from_disk() {
    let core = CoreBuilder::custom(providers()).build();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = SessionStore::new_session_id();
    let child = SessionStore::new_session_id();
    {
        let mut p = SessionStore::for_workspace(Path::new(&workspace.cwd), &parent);
        p.create().unwrap();
        let mut c = SessionStore::for_workspace(Path::new(&workspace.cwd), &child);
        c.create().unwrap();
        c.set_parent(&parent).unwrap();
    }
    // Archive the root first so both files sit in the archive dir.
    core.dispatch(Command::SessionArchive {
        session: parent.clone(),
    })
    .unwrap();
    // Delete the archived root: the file and its child's file are removed.
    core.dispatch(Command::SessionDelete {
        session: parent.clone(),
    })
    .unwrap();
    let root = Path::new(&workspace.cwd);
    for (id, role) in [(parent.as_str(), "the root"), (child.as_str(), "the child")] {
        assert!(
            !root
                .join(".tau")
                .join("archive")
                .join(format!("{id}.jsonl.zst"))
                .exists(),
            "{role}: the archived file is gone"
        );
        assert!(
            !root
                .join(".tau")
                .join("sessions")
                .join(format!("{id}.jsonl"))
                .exists(),
            "{role}: no live file left"
        );
    }
    let list = match core
        .dispatch(Command::SessionList {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Sessions { sessions } => sessions,
        other => panic!("expected sessions: {other:?}"),
    };
    assert!(
        !list.iter().any(|m| m.id == parent || m.id == child),
        "the deleted sessions leave the list"
    );
    drop(core);
}
