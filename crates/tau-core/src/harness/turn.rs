//! The post-turn reconciliation and the derived events it emits: the session file is the record; the events it implies are derived here, after `process()` drained the queue.

use super::*;

/// The post-turn reconciliation (spec §8 idempotent updates): the session
/// file is the record; the events it implies are derived here, after
/// `process()` drained the queue.
pub(crate) async fn run_turn(core: Arc<Core>, live: Arc<LiveSession>) {
    let mut store = SessionStore::for_workspace(&live.cwd, &live.meta.lock().unwrap().id);
    if let Err(e) = store.open() {
        // The send is already accepted (the GUI shows it as the turn):
        // a failed open must surface, or every send to a corrupted
        // session vanishes without a trace. The turn-starting message is
        // re-queued in the GUI's queue — it also stays in the agent's
        // queue, so the next successful turn delivers it, and the
        // post-turn reconciliation removes it by text.
        let (workspace, id) = {
            let meta = live.meta.lock().unwrap();
            (meta.workspace.clone(), meta.id.clone())
        };
        core.emit(Event::System {
            workspace: workspace.clone(),
            session: Some(id.clone()),
            kind: SystemEventKind::Error {
                message: format!("session {id} cannot be opened — the send was not run: {e}"),
            },
        });
        if let Some((text, lane)) = live.agent.first_pending() {
            live.queue.lock().unwrap().push(QueuedItem {
                text,
                lane: lane_to_message_lane(lane),
            });
            core.emit_queue(&live);
        }
        live.turn.store(false, Ordering::SeqCst);
        return;
    }
    // The archive sets the meta flag before its file moves (ADR-0005):
    // a turn that started in that window dies here, at its first write —
    // no headerless file, no partial turn in the archive.
    if live.meta.lock().unwrap().archived {
        let (workspace, id) = {
            let meta = live.meta.lock().unwrap();
            (meta.workspace.clone(), meta.id.clone())
        };
        core.emit(Event::System {
            workspace,
            session: Some(id.clone()),
            kind: SystemEventKind::Error {
                message: format!(
                    "session {id} was archived while this turn was starting — the turn was not run"
                ),
            },
        });
        live.turn.store(false, Ordering::SeqCst);
        return;
    }
    let start = store.leaf().ok().flatten().map(|e| e.id);
    let calls_before = live.provider.calls.lock().unwrap().len();

    if let Err(e) = live.agent.process().await {
        // One locked scope: two meta locks in one expression would be a
        // same-thread re-entrant deadlock (the first guard lives until the
        // statement ends).
        let (workspace, id) = {
            let meta = live.meta.lock().unwrap();
            (meta.workspace.clone(), meta.id.clone())
        };
        core.emit(Event::System {
            workspace,
            session: Some(id),
            kind: SystemEventKind::Error {
                message: e.to_string(),
            },
        });
    }

    let new: Vec<Entry> = match start {
        Some(cursor) => store.entries_since(&cursor).unwrap_or_default(),
        None => store.entries_range(0, usize::MAX).unwrap_or_default(),
    };

    let workspace = live.meta.lock().unwrap().workspace.clone();
    let session = live.meta.lock().unwrap().id.clone();
    let mut assistant_index = 0usize;
    let mut queue_changed = false;
    let mut task_touched = false;
    for entry in &new {
        match entry.kind.as_str() {
            // A delivered message leaves the GUI's queue (full-state
            // replacement keeps the GUI and the core in agreement).
            crate::agent::KIND_USER => {
                let text = entry.payload.get("text").and_then(Value::as_str);
                // The position lookup and the removal are separate locked
                // scopes: a let-chain would keep the first guard alive
                // across the second lock on the same (non-reentrant) mutex.
                let pos = text.and_then(|t| {
                    live.queue
                        .lock()
                        .unwrap()
                        .iter()
                        .position(|item| item.text == t)
                });
                if let Some(pos) = pos {
                    live.queue.lock().unwrap().remove(pos);
                    queue_changed = true;
                }
            }
            crate::agent::KIND_ASSISTANT => {
                let call_id = live
                    .provider
                    .calls
                    .lock()
                    .unwrap()
                    .get(calls_before + assistant_index)
                    .cloned();
                assistant_index += 1;
                let interrupted = entry
                    .payload
                    .get("interrupted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let usage = entry.payload.get("usage").and_then(usage_of);
                let completed = {
                    let map = live.provider.completed.lock().unwrap();
                    call_id
                        .as_ref()
                        .and_then(|id| map.get(id))
                        .copied()
                        .unwrap_or(false)
                };
                if let Some(call_id) = call_id
                    && !completed
                {
                    // The stream was cut before its Completed frame: the
                    // partial stands as an interrupted end (spec §6/§7).
                    core.emit(Event::StreamEnd {
                        workspace: workspace.clone(),
                        session: session.clone(),
                        call_id,
                        interrupted,
                        usage,
                    });
                }
                if let Some(u) = usage {
                    live.meta.lock().unwrap().usage = Some(u);
                }
            }
            crate::agent::KIND_TOOL => {
                let call_id = live
                    .provider
                    .calls
                    .lock()
                    .unwrap()
                    .get(calls_before + assistant_index.saturating_sub(1))
                    .cloned()
                    .unwrap_or_default();
                let tool_call_id = entry
                    .payload
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = entry
                    .payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                core.emit(Event::ToolStart {
                    workspace: workspace.clone(),
                    session: session.clone(),
                    call_id: call_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                });
                core.emit(Event::ToolEnd {
                    workspace: workspace.clone(),
                    session: session.clone(),
                    call_id,
                    tool_call_id,
                    name,
                    output: entry.payload.get("output").cloned().unwrap_or(Value::Null),
                });
            }
            crate::task::KIND_TASK => {
                task_touched = true;
            }
            _ => {}
        }
        if let Some(first_kept) = &entry.first_kept_entry_id {
            core.emit(Event::SessionEvent {
                workspace: workspace.clone(),
                session: session.clone(),
                kind: tau_protocol::SessionEventKind::Compaction {
                    entry: entry.id.clone(),
                    first_kept: first_kept.clone(),
                },
            });
        }
    }

    // A call cut before it produced an entry (an empty partial, N10) still
    // gets its interrupted end — the GUI's live bubble must close.
    {
        let calls = live.provider.calls.lock().unwrap();
        let new_calls = calls.len() - calls_before;
        for i in assistant_index..new_calls {
            core.emit(Event::StreamEnd {
                workspace: workspace.clone(),
                session: session.clone(),
                call_id: calls[calls_before + i].clone(),
                interrupted: true,
                usage: None,
            });
        }
    }

    // The active branch is what the loop appended against.
    if let Ok(leaf) = store.leaf()
        && let Some(leaf) = leaf
    {
        live.meta.lock().unwrap().leaf = Some(leaf.id);
    }
    if queue_changed {
        core.emit_queue(&live);
    }
    // Task entries fold into the session's task list (spec §8): the event
    // carries the full list derived from the file — a projection, never a
    // second source of truth; the store replaces on receive.
    if task_touched {
        // A read that fails (a torn-append race) must not emit an
        // authoritative empty list — skip; the next emission or the
        // open/switch snapshot converges.
        if let Ok(all) = store.entries_range(0, usize::MAX) {
            core.emit(Event::TaskChanged {
                workspace: workspace.clone(),
                session: session.clone(),
                tasks: crate::task::fold_entries(&all),
            });
        }
    }
    live.turn.store(false, Ordering::SeqCst);
}

impl Core {
    pub(crate) fn emit_queue(&self, live: &LiveSession) {
        // One locked scope: the event's fields are cloned out before the
        // guard drops, so a long emit cannot hold the locks.
        let (workspace, session, items) = {
            let meta = live.meta.lock().unwrap();
            (
                meta.workspace.clone(),
                meta.id.clone(),
                live.queue.lock().unwrap().clone(),
            )
        };
        self.emit(Event::Queue {
            workspace,
            session,
            items,
        });
    }

    /// The session's task list, folded from its file, as a task_changed
    /// event (spec §8): the payload is a projection of the file, never a
    /// second source of truth; the store replaces on receive, so the GUI
    /// converges on the file's state without polling.
    pub(crate) fn emit_task_changed(&self, live: &LiveSession, session: &str) {
        let (workspace, cwd) = {
            let meta = live.meta.lock().unwrap();
            (meta.workspace.clone(), live.cwd.clone())
        };
        let mut store = SessionStore::for_workspace(&cwd, session);
        if store.open().is_err() {
            return; // the file is gone; the next open rebuilds from nothing
        }
        let Ok(entries) = store.entries_range(0, usize::MAX) else {
            return; // a failed read is not an empty task list (torn-append race)
        };
        self.emit(Event::TaskChanged {
            workspace,
            session: session.to_owned(),
            tasks: crate::task::fold_entries(&entries),
        });
    }
}
