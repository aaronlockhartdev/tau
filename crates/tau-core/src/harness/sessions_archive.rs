//! The archive for a session not in the live map (ADR-0005): no turn and no
//! supervisor to quiesce, so it is a pure file operation — and the live
//! child-quiesce half of a top-level archive (detach, stop the children's
//! drives, move the files), which the dispatch arm routes to once every
//! refusal has been ruled out.

use std::collections::BTreeSet;

use super::*;

impl Core {
    /// The live child-quiesce half of a top-level archive (ADR-0005),
    /// reached only after every refusal was ruled out (a refused request
    /// must mutate nothing): detach, stop and quiesce the children, then
    /// move the files — aborting and re-attaching if a turn starts in any
    /// of the windows.
    pub(super) fn archive_live(
        &self,
        live: Arc<LiveSession>,
        session: &str,
        children: &BTreeSet<String>,
    ) -> Result<CommandOutput, ProtocolError> {
        // Detach from the live map while the children are stopped:
        // a stop's parent-wake must not start a turn on the file
        // being archived (the wake looks the parent up by id).
        self.sessions.lock().unwrap().remove(session);
        // A wake that looked the parent up before the detach holds
        // the shared session and will CAS the turn and spawn a turn
        // (its map read predates the removal): re-check after the
        // removal and abort if a turn started in the window — the
        // file must not move mid-write.
        self.archive_turn_recheck(session, &live)?;
        // This session's children: a running one is stopped and its
        // drive quiesced before any file moves (its wake would
        // write the parent file mid-archive), and a parked one's
        // sleeping drive ends with the parent.
        if let Some(sup) = live.agent.subagents() {
            sup.stop_all(StoppedBy::User);
            for handle in sup.handles() {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !sup.drive_quiescent(&handle) && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                // The deadline is a window: a drive that never
                // exited is still running (a stop is terminal, so
                // it will not exit), and the child's live-shell
                // turn flag only tracks GUI sends — the
                // supervisor's state is the source of truth here.
                if sup.child_running(&handle) {
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(session.to_owned(), live.clone());
                    let child = sup
                        .state_info(&handle)
                        .map(|i| i.child)
                        .unwrap_or_else(|| handle.clone());
                    return Err(ProtocolError::Other {
                        message: format!(
                            "child session {child} is still running — stop it and retry"
                        ),
                    });
                }
            }
        }
        // The quiesce waits are a window (up to 5 s per child): a
        // child outside the supervisor's reach — stop_all drives
        // only the supervisor's children — may have started a turn
        // in it; abort, as the parent's own wake check does, so no
        // file moves mid-write.
        for child in children {
            if let Some(cl) = self.sessions.lock().unwrap().get(child)
                && cl.turn.load(Ordering::SeqCst)
            {
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(session.to_owned(), live.clone());
                return Err(ProtocolError::Other {
                    message: format!(
                        "child session {child} started a turn while archiving — stop it and retry"
                    ),
                });
            }
        }
        for child in children {
            let cstore = SessionStore::for_workspace(&live.cwd, child);
            if !cstore.path().exists() {
                continue;
            }
            // The flag goes in before the child's own move, like
            // the parent's: a child's turn that starts in the
            // window dies at its first append. A failed move
            // reverts it — a refused archive mutates nothing.
            let live_child = self.sessions.lock().unwrap().get(child).cloned();
            if let Some(cl) = &live_child {
                cl.meta.lock().unwrap().archived = true;
            }
            // I/O failure: the parent's in-memory session (queue,
            // supervisor) comes back into the live map; children
            // already archived stay archived — the next
            // archive/restore converges on the file state.
            if let Err(e) = cstore.archive() {
                if let Some(cl) = &live_child {
                    cl.meta.lock().unwrap().archived = false;
                }
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(session.to_owned(), live.clone());
                return Err(ProtocolError::Other {
                    message: e.to_string(),
                });
            }
        }
        // The last possible moment before the parent's file moves:
        // the window above is wide enough for a wake that grabbed
        // the session's Arc before the detach to CAS a turn here —
        // and append_line opens with create(true), so a deleted
        // live file would be recreated headerless (unopenable).
        self.archive_turn_recheck(session, &live)?;
        // The commit: from here the archive only moves files, so
        // the flag goes in before the parent's move — a turn that
        // starts in the remaining window reads it at its first
        // append (run_turn) and dies clean: no headerless file, no
        // partial turn in the archive.
        let was_archived = live.meta.lock().unwrap().archived;
        live.meta.lock().unwrap().archived = true;
        let mut store = SessionStore::for_workspace(&live.cwd, session);
        if let Err(e) = store.open().and_then(|()| store.archive()) {
            live.meta.lock().unwrap().archived = was_archived;
            self.sessions
                .lock()
                .unwrap()
                .insert(session.to_owned(), live.clone());
            return Err(ProtocolError::Other {
                message: e.to_string(),
            });
        }
        self.sessions
            .lock()
            .unwrap()
            .insert(session.to_owned(), live.clone());
        Ok(CommandOutput::Session {
            session: live.meta.lock().unwrap().clone(),
        })
    }

    /// The archive for a session not in the live map (never opened, or
    /// closed since): no turn, no supervisor, no in-memory state to
    /// quiesce — a pure file operation (ADR-0005). The meta comes from
    /// the live file, the children from the workspace's disk scan, and
    /// both move to `archive/`.
    pub(super) fn archive_closed(&self, session: &str) -> Result<CommandOutput, ProtocolError> {
        // The GUI archives from an open workspace's tree: find the
        // workspace whose live directory holds the file.
        let workspace = {
            let wss = self.workspaces.lock().unwrap();
            wss.values()
                .find(|w| {
                    SessionStore::for_workspace(Path::new(&w.cwd), session)
                        .path()
                        .exists()
                })
                .cloned()
        }
        .ok_or_else(|| ProtocolError::NotFound {
            what: format!("session {session} is not open"),
        })?;
        let cwd = PathBuf::from(&workspace.cwd);
        let mut store = SessionStore::for_workspace(&cwd, session);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // A child archives with its parent, never alone (ADR-0005) — the
        // on-disk meta carries the parent link.
        if let Some(parent) = store.parent() {
            return Err(ProtocolError::Other {
                message: self.archive_refusal_for_child(
                    session,
                    &workspace.id,
                    &Some(parent.to_string()),
                ),
            });
        }
        // The children that archive with it: the workspace's disk scan is
        // the whole truth here (there is no supervisor in memory). One
        // already archived stays archived.
        let children: Vec<String> = self
            .session_access(&workspace)
            .into_iter()
            .filter(|m| !m.archived && m.parent.as_deref() == Some(session))
            .map(|m| m.id)
            .collect();
        for id in &children {
            let mut cstore = SessionStore::for_workspace(&cwd, id);
            cstore.open().map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
            // The flag goes in before the child's move, like the open
            // path: a child still in the live map (opened on its own,
            // parent closed) has its next write refused at the first
            // append instead of recreating the file headerless.
            if let Some(cl) = self.sessions.lock().unwrap().get(id) {
                cl.meta.lock().unwrap().archived = true;
            }
            cstore.archive().map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        }
        store.archive().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // The response is a fresh meta from the file (the list and any
        // snapshot converge on it), like restore.
        Ok(CommandOutput::Session {
            session: SessionMeta {
                id: session.to_string(),
                workspace: workspace.id,
                title: store.title().map(str::to_string),
                parent: None,
                created: store.created(),
                leaf: store.leaf().ok().flatten().map(|e| e.id),
                model: None,
                usage: None,
                archived: true,
            },
        })
    }

    /// Delete a session that is not in the live map — a pure file op (the
    /// disk-only twin of `archive_closed`). Archived sessions only ever reach
    /// this path: their file sits in the archive dir, never in memory. The
    /// target and every descendant are removed (a delete orphans its
    /// children, so they go with it).
    pub(super) fn delete_closed(&self, session: &str) -> Result<CommandOutput, ProtocolError> {
        // The GUI deletes from an open workspace's tree: find the workspace
        // whose dir holds the file, live or archived.
        let workspace = {
            let wss = self.workspaces.lock().unwrap();
            wss.values()
                .find(|w| {
                    let s = SessionStore::for_workspace(Path::new(&w.cwd), session);
                    s.path().exists() || s.archive_path().exists()
                })
                .cloned()
        }
        .ok_or_else(|| ProtocolError::NotFound {
            what: format!("session {session} is not open"),
        })?;
        let cwd = PathBuf::from(&workspace.cwd);
        // The workspace's disk scan is the whole truth here (no supervisor in
        // memory): BFS the parent links to collect the target and all
        // descendants.
        let all = self.session_access(&workspace);
        let to_delete = descendants(&all, session);
        for id in &to_delete {
            // A child may still be open on its own: drop it from the live map
            // so a late write can't resurrect the file.
            self.sessions.lock().unwrap().remove(id);
            delete_session_files(&cwd, id);
        }
        Ok(CommandOutput::None)
    }
}

/// The target and every descendant in a disk listing: BFS over the parent
/// links (a delete orphans its children, so they go with it).
pub(super) fn descendants(all: &[SessionMeta], root: &str) -> Vec<String> {
    let mut out = vec![root.to_string()];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i].clone();
        i += 1;
        for m in all
            .iter()
            .filter(|m| m.parent.as_deref() == Some(parent.as_str()))
        {
            if !out.contains(&m.id) {
                out.push(m.id.clone());
            }
        }
    }
    out
}
