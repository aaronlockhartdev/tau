use super::*;

use crate::harness::testkit::*;

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
