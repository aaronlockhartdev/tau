//! The session-lifecycle arms: list / new / rename / model / open / close /
//! delete / archive / restore / branch / snapshot / paged reads.

use super::*;

impl Core {
    pub(crate) fn dispatch_session(
        self: &Arc<Self>,
        cmd: Command,
    ) -> Result<CommandOutput, ProtocolError> {
        match cmd {
            Command::SessionList { workspace } => {
                let workspace = self.workspace(&workspace)?;
                let mut sessions = self
                    .sessions
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|s| s.meta.lock().unwrap().workspace == workspace.id)
                    .map(|s| s.meta.lock().unwrap().clone())
                    .collect::<Vec<_>>();
                // The file is the record: sessions closed since boot (or
                // from a previous run) still list, `archive/` included
                // (flagged); the live copy wins.
                for m in self.session_access(&workspace) {
                    if !sessions.iter().any(|s| s.id == m.id) {
                        sessions.push(m);
                    }
                }
                sessions.sort_by_key(|s| std::cmp::Reverse(s.created));
                Ok(CommandOutput::Sessions { sessions })
            }
            Command::SessionNew { workspace, title } => {
                let workspace = self.workspace(&workspace)?;
                Ok(CommandOutput::Session {
                    session: self.session_new(&workspace, title)?,
                })
            }
            Command::SessionRename { session, title } => {
                if title.chars().count() > MAX_TITLE_LEN {
                    return Err(ProtocolError::Other {
                        message: format!(
                            "title is {} characters — the maximum is {MAX_TITLE_LEN}",
                            title.chars().count()
                        ),
                    });
                }
                // The title lives in the file header, so the same write
                // works for a live and a closed session alike.
                let cwd = match self.live(&session) {
                    Ok(l) => {
                        // Keep the in-memory meta in sync; snapshot() and
                        // session_list() serve it, so a re-open must not
                        // revert the rename.
                        l.meta.lock().unwrap().title = Some(title.clone());
                        l.cwd.clone()
                    }
                    Err(_) => self
                        .workspaces
                        .lock()
                        .unwrap()
                        .values()
                        .find(|w| {
                            self.session_access(w)
                                .iter()
                                .any(|m| m.id == session && !m.archived)
                        })
                        .map(|w| PathBuf::from(w.cwd.clone()))
                        .ok_or_else(|| ProtocolError::Other {
                            message: "unknown session".into(),
                        })?,
                };
                let mut store = SessionStore::for_workspace(&cwd, &session);
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                store.set_title(&title).map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                Ok(CommandOutput::None)
            }
            Command::SessionSetModel { session, model } => {
                if model.trim().is_empty() {
                    return Err(ProtocolError::Other {
                        message: "model is empty".into(),
                    });
                }
                match self.live(&session) {
                    Ok(live) => {
                        let old = live.meta.lock().unwrap().model.clone();
                        if old.as_deref() == Some(model.as_str()) {
                            return Ok(CommandOutput::None);
                        }
                        // The same meta-mutation path as a rename: the
                        // in-memory meta is what snapshot() and
                        // session_list() serve, so a re-open must not
                        // revert the change; the agent's own model drives
                        // the next turn's calls.
                        live.meta.lock().unwrap().model = Some(model.clone());
                        live.agent.set_model(model.clone());
                        live.agent
                            .append_entry(
                                crate::agent::KIND_SYSTEM,
                                json!({ "note": model_note(&old, &model) }),
                            )
                            .map_err(|e| ProtocolError::Other {
                                message: e.to_string(),
                            })?;
                        Ok(CommandOutput::None)
                    }
                    // A closed session is a file: the header carries no
                    // model, so the quiet entries are the record — the
                    // last `model:` note on the branch is the current one.
                    Err(_) => {
                        let cwd = self
                            .workspaces
                            .lock()
                            .unwrap()
                            .values()
                            .find(|w| {
                                self.session_access(w)
                                    .iter()
                                    .any(|m| m.id == session && !m.archived)
                            })
                            .map(|w| PathBuf::from(w.cwd.clone()))
                            .ok_or_else(|| ProtocolError::Other {
                                message: "unknown session".into(),
                            })?;
                        let mut store = SessionStore::for_workspace(&cwd, &session);
                        store.open().map_err(|e| ProtocolError::Other {
                            message: e.to_string(),
                        })?;
                        let old = last_model_note(&mut store);
                        if old.as_deref() == Some(model.as_str()) {
                            return Ok(CommandOutput::None);
                        }
                        let leaf = store.leaf().ok().flatten().map(|e| e.id);
                        store
                            .append(
                                crate::agent::KIND_SYSTEM,
                                json!({ "note": model_note(&old, &model) }),
                                leaf.as_deref(),
                            )
                            .map_err(|e| ProtocolError::Other {
                                message: e.to_string(),
                            })?;
                        Ok(CommandOutput::None)
                    }
                }
            }
            Command::SessionOpen { session } => {
                match self.live(&session) {
                    Ok(live) => Ok(CommandOutput::Snapshot {
                        snapshot: self.snapshot(&live)?,
                    }),
                    // A closed session is a file: re-register it as live so
                    // it can be resumed (a restart drops the in-memory live
                    // map); if no known workspace owns it, serve read-only.
                    Err(_) => {
                        let ws = self
                            .workspaces
                            .lock()
                            .unwrap()
                            .values()
                            .find(|w| {
                                Path::new(&w.cwd)
                                    .join(".tau")
                                    .join("sessions")
                                    .join(format!("{session}.jsonl"))
                                    .exists()
                            })
                            .cloned();
                        if let Some(ws) = ws
                            && let Ok(_meta) = self.session_reopen(&ws, &session)
                            && let Ok(live) = self.live(&session)
                        {
                            return Ok(CommandOutput::Snapshot {
                                snapshot: self.snapshot(&live)?,
                            });
                        }
                        Ok(CommandOutput::Snapshot {
                            snapshot: self.snapshot_from_disk(&session)?,
                        })
                    }
                }
            }
            Command::SessionClose { session } => {
                // Closing stops any in-flight turn: a closed session must
                // not keep streaming (review N7) — and its children keep no
                // work running for a session that no longer exists (their
                // files stay; they remain resumable as standalone sessions).
                if let Some(live) = self.sessions.lock().unwrap().remove(&session) {
                    live.stop.store(true, Ordering::SeqCst);
                    if let Some(sup) = live.agent.subagents() {
                        sup.stop_all(StoppedBy::User);
                    }
                }
                Ok(CommandOutput::None)
            }
            Command::SessionDelete { session } => {
                if let Some(live) = self.sessions.lock().unwrap().remove(&session) {
                    live.stop.store(true, Ordering::SeqCst);
                    if let Some(sup) = live.agent.subagents() {
                        sup.stop_all(StoppedBy::User);
                    }
                    delete_session_files(&live.cwd, &session);
                }
                Ok(CommandOutput::None)
            }
            Command::SessionArchive { session } => {
                let live = match self.live(&session) {
                    Ok(live) => live,
                    // A session not in the live map (never opened, or
                    // closed since) has no turn or supervisor to
                    // quiesce: its archive is a pure file operation
                    // (ADR-0005).
                    Err(ProtocolError::NotFound { .. }) => {
                        return self.archive_closed(&session);
                    }
                    Err(e) => return Err(e),
                };
                // Only top-level sessions archive: a sub-agent archives with
                // its parent (ADR-0005), so a child id is refused outright.
                if live.agent.child_link().is_some() {
                    let meta = live.meta.lock().unwrap();
                    return Err(ProtocolError::Other {
                        message: self.archive_refusal_for_child(
                            &session,
                            &meta.workspace,
                            &meta.parent,
                        ),
                    });
                }
                // A running session's file is being written: archiving
                // mid-turn would tear it (ADR-0005 keeps the archive off
                // the live path).
                if live.turn.load(Ordering::SeqCst) {
                    return Err(ProtocolError::Other {
                        message: format!("session {session} is running — stop it first"),
                    });
                }
                // The children that archive with this session (ADR-0005):
                // the supervisor's, the live, and the ones closed on
                // disk. One already archived stays archived. A running
                // one is refused BEFORE anything is quiesced: a stop is
                // terminal, so a refused request must mutate nothing.
                let workspace = self.workspace(&live.meta.lock().unwrap().workspace)?;
                let mut children = std::collections::BTreeSet::new();
                if let Some(sup) = live.agent.subagents() {
                    for handle in sup.handles() {
                        let Some(info) = sup.state_info(&handle) else {
                            continue;
                        };
                        // A supervisor child's run is the supervisor's
                        // state (its live shell's turn flag only tracks
                        // GUI sends): it blocks the archive too, before
                        // anything is quiesced.
                        if matches!(info.state, crate::subagent::ChildState::Running) {
                            return Err(ProtocolError::Other {
                                message: format!(
                                    "child session {} is running — stop it first",
                                    info.child
                                ),
                            });
                        }
                        children.insert(info.child);
                    }
                }
                for (id, s) in self.sessions.lock().unwrap().iter() {
                    if s.meta.lock().unwrap().parent.as_deref() == Some(session.as_str()) {
                        children.insert(id.clone());
                    }
                }
                for m in self
                    .session_access(&workspace)
                    .into_iter()
                    .filter(|m| m.parent.as_deref() == Some(session.as_str()))
                {
                    if m.archived {
                        children.remove(&m.id);
                    } else {
                        children.insert(m.id);
                    }
                }
                for child in &children {
                    if let Some(cl) = self.sessions.lock().unwrap().get(child)
                        && cl.turn.load(Ordering::SeqCst)
                    {
                        return Err(ProtocolError::Other {
                            message: format!("child session {child} is running — stop it first"),
                        });
                    }
                }
                // Detach from the live map while the children are stopped:
                // a stop's parent-wake must not start a turn on the file
                // being archived (the wake looks the parent up by id).
                self.sessions.lock().unwrap().remove(&session);
                // A wake that looked the parent up before the detach holds
                // the shared session and will CAS the turn and spawn a turn
                // (its map read predates the removal): re-check after the
                // removal and abort if a turn started in the window — the
                // file must not move mid-write.
                self.archive_turn_recheck(&session, &live)?;
                // This session's children: a running one is stopped and its
                // drive quiesced before any file moves (its wake would
                // write the parent file mid-archive), and a parked one's
                // sleeping drive ends with the parent.
                if let Some(sup) = live.agent.subagents() {
                    sup.stop_all(StoppedBy::User);
                    for handle in sup.handles() {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(5);
                        while !sup.drive_quiescent(&handle) && std::time::Instant::now() < deadline
                        {
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
                                .insert(session.clone(), live.clone());
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
                for child in &children {
                    if let Some(cl) = self.sessions.lock().unwrap().get(child)
                        && cl.turn.load(Ordering::SeqCst)
                    {
                        self.sessions
                            .lock()
                            .unwrap()
                            .insert(session.clone(), live.clone());
                        return Err(ProtocolError::Other {
                            message: format!(
                                "child session {child} started a turn while archiving — stop it and retry"
                            ),
                        });
                    }
                }
                for child in &children {
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
                            .insert(session.clone(), live.clone());
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
                self.archive_turn_recheck(&session, &live)?;
                // The commit: from here the archive only moves files, so
                // the flag goes in before the parent's move — a turn that
                // starts in the remaining window reads it at its first
                // append (run_turn) and dies clean: no headerless file, no
                // partial turn in the archive.
                let was_archived = live.meta.lock().unwrap().archived;
                live.meta.lock().unwrap().archived = true;
                let mut store = SessionStore::for_workspace(&live.cwd, &session);
                if let Err(e) = store.open().and_then(|()| store.archive()) {
                    live.meta.lock().unwrap().archived = was_archived;
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(session.clone(), live.clone());
                    return Err(ProtocolError::Other {
                        message: e.to_string(),
                    });
                }
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(session.clone(), live.clone());
                Ok(CommandOutput::Session {
                    session: live.meta.lock().unwrap().clone(),
                })
            }
            Command::SessionRestore { workspace, session } => {
                let workspace = self.workspace(&workspace)?;
                let cwd = PathBuf::from(&workspace.cwd);
                let mut store = SessionStore::for_workspace(&cwd, &session);
                if !store.archive_path().exists() {
                    return Err(ProtocolError::Other {
                        message: format!("no archive for session {session}"),
                    });
                }
                if store.path().exists() {
                    return Err(ProtocolError::Other {
                        message: format!("session {session} already has a live file"),
                    });
                }
                store.unarchive().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                // The session's sub-agent children, archived with it, restore
                // with it (their files are stable — off the live path, no
                // turn can write them).
                for m in self
                    .session_access(&workspace)
                    .into_iter()
                    .filter(|m| m.archived && m.parent.as_deref() == Some(session.as_str()))
                {
                    let cstore = SessionStore::for_workspace(&cwd, &m.id);
                    if cstore.path().exists() {
                        continue;
                    }
                    cstore.unarchive().map_err(|e| ProtocolError::Other {
                        message: e.to_string(),
                    })?;
                    if let Some(cl) = self.sessions.lock().unwrap().get(&m.id) {
                        cl.meta.lock().unwrap().archived = false;
                    }
                }
                if let Some(pl) = self.sessions.lock().unwrap().get(&session) {
                    pl.meta.lock().unwrap().archived = false;
                }
                // The response is a fresh meta from the restored file (the
                // list and any snapshot converge on it).
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                let meta = SessionMeta {
                    id: session,
                    workspace: workspace.id.clone(),
                    title: store.title().map(str::to_string),
                    parent: store.parent().map(str::to_string),
                    created: store.created(),
                    leaf: store.leaf().ok().flatten().map(|e| e.id),
                    model: None,
                    usage: None,
                    archived: false,
                };
                Ok(CommandOutput::Session { session: meta })
            }
            Command::SessionFork { session, at } | Command::SessionBranch { session, at } => {
                let live = self.live(&session)?;
                let mut store = SessionStore::for_workspace(&live.cwd, &session);
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                store.set_leaf(&at).map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                live.meta.lock().unwrap().leaf = Some(at.clone());
                self.emit(Event::SessionEvent {
                    workspace: live.meta.lock().unwrap().workspace.clone(),
                    session: session.clone(),
                    kind: tau_protocol::SessionEventKind::BranchMove { leaf: at },
                });
                Ok(CommandOutput::Session {
                    session: live.meta.lock().unwrap().clone(),
                })
            }
            Command::SessionSnapshot { session } => {
                let live = self.live(&session)?;
                Ok(CommandOutput::Snapshot {
                    snapshot: self.snapshot(&live)?,
                })
            }
            Command::SessionEntries {
                session,
                since,
                range,
            } => {
                let entries = match self.live(&session) {
                    Ok(live) => self.entries(&live, since, range)?,
                    Err(_) => self.entries_from_disk(&session, since, range)?,
                };
                Ok(CommandOutput::Entries { entries })
            }

            _ => unreachable!("dispatch routes the arm"),
        }
    }

    /// The archive for a session not in the live map (never opened, or
    /// closed since): no turn, no supervisor, no in-memory state to
    /// quiesce — a pure file operation (ADR-0005). The meta comes from
    /// the live file, the children from the workspace's disk scan, and
    /// both move to `archive/`.
    fn archive_closed(&self, session: &str) -> Result<CommandOutput, ProtocolError> {
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
}
