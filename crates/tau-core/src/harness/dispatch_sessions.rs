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
                let (live, cwd) = match self.live(&session) {
                    Ok(l) => {
                        // The archive's live-turn guard (ADR-0005): the
                        // rename is a read-modify-write header rewrite
                        // racing the in-flight turn's append_line — an
                        // entry appended in the window would be clobbered
                        // by the rename.
                        if l.turn.load(Ordering::SeqCst) {
                            return Err(running_turn_refusal(&session));
                        }
                        // Keep the in-memory meta in sync; snapshot() and
                        // session_list() serve it, so a re-open must not
                        // revert the rename.
                        l.meta.lock().unwrap().title = Some(title.clone());
                        let cwd = l.cwd.clone();
                        (Some(l), cwd)
                    }
                    Err(_) => (
                        None,
                        self.workspaces
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
                    ),
                };
                let mut store = SessionStore::for_workspace(&cwd, &session);
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                // The archive's recheck: a wake can CAS a turn between the first
                // check and the header rewrite.
                if let Some(l) = &live
                    && l.turn.load(Ordering::SeqCst)
                {
                    return Err(ProtocolError::Other {
                        message: format!(
                            "session {session} started a turn while renaming — stop it and retry"
                        ),
                    });
                }
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
                        // #35: the model-specific options (output-cap clamp,
                        // reasoning level) track the active model —
                        // re-derived from the workspace config.
                        let ws_id = live.meta.lock().unwrap().workspace.clone();
                        if let Ok(ws) = self.workspace(&ws_id) {
                            let config = self.workspace_config(&ws);
                            if let Some((_, p)) = config.providers.iter().next() {
                                live.agent
                                    .set_turn_config(derive_turn(&config, p, &model, &session));
                            }
                        }
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
                // The remove's guard is scoped to the statement: a re-entrant sessions
                // lock in the block below would deadlock (the mutex is
                // non-reentrant).
                let closed = self.sessions.lock().unwrap().remove(&session);
                if let Some(live) = closed {
                    // Closing a running CHILD session routes through the
                    // parent's supervisor (the terminal Stopped record, the
                    // state event, the parent's wake) — a bare stop flag
                    // would let the child's drive burn its one-shot nudge
                    // into a bogus `failed`.
                    if let Some(link) = live.agent.child_link() {
                        let parent = link
                            .handle()
                            .rsplit_once('-')
                            .map(|(s, _)| s.to_owned())
                            .unwrap_or_default();
                        if let Ok(parent_live) = self.live(&parent)
                            && let Some(sup) = parent_live.agent.subagents()
                        {
                            let _ = sup.stop_handle(link.handle(), StoppedBy::User);
                        }
                    }
                    live.stop.store(true, Ordering::SeqCst);
                    if let Some(sup) = live.agent.subagents() {
                        sup.stop_all(StoppedBy::User);
                    }
                }
                Ok(CommandOutput::None)
            }
            Command::SessionDelete { session } => {
                // The archive guard's twin (ADR-0005 keeps file moves off the live
                // path): a running session's file is being written, so
                // deleting it mid-turn tears it — and the turn's next
                // write hits the gone file (the append refusal) instead
                // of recording.
                if let Some(live) = self.sessions.lock().unwrap().get(&session)
                    && live.turn.load(Ordering::SeqCst)
                {
                    return Err(ProtocolError::Other {
                        message: format!("session {session} is running — stop it first"),
                    });
                }
                let Some(live) = self.sessions.lock().unwrap().remove(&session) else {
                    // Not in the live map (never opened, or closed since —
                    // includes archived sessions): delete is a pure file op.
                    return self.delete_closed(&session);
                };
                // A wake that looked the session up before the detach holds
                // the shared session and will CAS the turn (its map read
                // predates the removal): re-check after the removal and
                // abort if a turn started in the window — the file must
                // not move mid-write.
                if live.turn.load(Ordering::SeqCst) {
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(session.to_owned(), live.clone());
                    return Err(ProtocolError::Other {
                        message: format!(
                            "session {session} started a turn while deleting — stop it and retry"
                        ),
                    });
                }
                live.stop.store(true, Ordering::SeqCst);
                if let Some(sup) = live.agent.subagents() {
                    sup.stop_all(StoppedBy::User);
                }
                delete_session_files(&live.cwd, &session);
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
                self.archive_live(live, &session, &children)
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
                // The archive's live-turn guard (ADR-0005): set_leaf
                // rewrites the header racing the in-flight turn's
                // append_line, and it would desync the agent's in-memory
                // writer leaf from the file's — the next entry would
                // parent to the old branch.
                if live.turn.load(Ordering::SeqCst) {
                    return Err(running_turn_refusal(&session));
                }
                let mut store = SessionStore::for_workspace(&live.cwd, &session);
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                // The archive's recheck: a wake can CAS a turn between the
                // first check and the header rewrite.
                if live.turn.load(Ordering::SeqCst) {
                    return Err(ProtocolError::Other {
                        message: format!(
                            "session {session} started a turn while branching — stop it and retry"
                        ),
                    });
                }
                store.set_leaf(&at).map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                // The live session's store is now the snapshot's source
                // (its in-memory log): it must see the new leaf too, or the
                // next snapshot's cursor would lag the branch move.
                live.agent.with_task_store(|s| s.adopt_leaf(&at));
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
}
