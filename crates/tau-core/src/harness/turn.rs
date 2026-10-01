//! The post-turn reconciliation and the derived events it emits: the session file is the record; the events it implies are derived here, after `process()` drained the queue.

use super::{
    Arc, Core, Entry, Event, LiveSession, Ordering, SessionStore, SystemEventKind, Value,
    entry_to_view, usage_of,
};
use crate::subagent::ChildLink;

/// The post-turn reconciliation (spec §8 idempotent updates): the session
/// file is the record; the events it implies are derived here, after
/// `process()` drained the queue.
#[allow(clippy::too_many_lines)] // one reconciliation pass over the turn's record; splitting is refactoring
pub(crate) async fn run_turn(core: Arc<Core>, live: Arc<LiveSession>) {
    let mut store = SessionStore::for_workspace(
        &live.cwd,
        &live
            .meta
            .lock()
            .expect("session meta: no panic while the lock is held")
            .id,
    );
    if let Err(e) = store.open() {
        // The send is already accepted (the GUI shows it as the turn):
        // a failed open must surface, or every send to a corrupted
        // session vanishes without a trace. The turn-starting message
        // also stays in the agent's queue, so the next successful turn
        // delivers it, and the queue's projection shows it — nothing is
        // re-queued anywhere.
        let (workspace, id) = {
            let meta = live
                .meta
                .lock()
                .expect("session meta: no panic while the lock is held");
            (meta.workspace.clone(), meta.id.clone())
        };
        core.emit(Event::System {
            workspace: workspace.clone(),
            session: Some(id.clone()),
            kind: SystemEventKind::Error {
                message: format!("session {id} cannot be opened — the send was not run: {e}"),
            },
        });
        if live.agent.first_pending().is_some() {
            core.emit_queue(&live);
        }
        live.turn.store(false, Ordering::SeqCst);
        return;
    }
    // The archive sets the meta flag before its file moves (ADR-0005):
    // a turn that started in that window dies here, at its first write —
    // no headerless file, no partial turn in the archive.
    if live
        .meta
        .lock()
        .expect("session meta: no panic while the lock is held")
        .archived
    {
        let (workspace, id) = {
            let meta = live
                .meta
                .lock()
                .expect("session meta: no panic while the lock is held");
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
    let calls_before = live
        .provider
        .calls
        .lock()
        .expect("provider call ids: no panic while the lock is held")
        .len();

    if let Err(e) = live.agent.process().await {
        // One locked scope: two meta locks in one expression would be a
        // same-thread re-entrant deadlock (the first guard lives until the
        // statement ends).
        let (workspace, id) = {
            let meta = live
                .meta
                .lock()
                .expect("session meta: no panic while the lock is held");
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

    let workspace = live
        .meta
        .lock()
        .expect("session meta: no panic while the lock is held")
        .workspace
        .clone();
    let session = live
        .meta
        .lock()
        .expect("session meta: no panic while the lock is held")
        .id
        .clone();
    let mut assistant_index = 0usize;
    let mut queue_changed = false;
    let mut task_touched = false;
    for entry in &new {
        match entry.kind.as_str() {
            crate::agent::KIND_USER => {
                // A delivered message has left the agent's queue (the
                // process loop drained it): the post-turn Queue event is
                // projected from the queue, so the GUI converges without a
                // second ledger to remove from.
                queue_changed = true;
            }
            crate::agent::KIND_ASSISTANT => {
                let call_id = live
                    .provider
                    .calls
                    .lock()
                    .expect("provider call ids: no panic while the lock is held")
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
                    let map = live
                        .provider
                        .completed
                        .lock()
                        .expect("completed-call map: no panic while the lock is held");
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
                    live.meta
                        .lock()
                        .expect("session meta: no panic while the lock is held")
                        .usage = Some(u);
                }
            }
            crate::agent::KIND_TOOL => {
                // The live hook already upserted this entry; the post-turn
                // pass re-emits the file line (a lossy-channel dedup,
                // idempotent by construction).
                core.emit(Event::EntryUpsert {
                    workspace: workspace.clone(),
                    session: session.clone(),
                    entry: entry_to_view(entry),
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
        let calls = live
            .provider
            .calls
            .lock()
            .expect("provider call ids: no panic while the lock is held");
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
        live.meta
            .lock()
            .expect("session meta: no panic while the lock is held")
            .leaf = Some(leaf.id);
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
            let tasks = crate::task::fold_entries(&all);
            core.emit(Event::TaskChanged {
                workspace: workspace.clone(),
                session: session.clone(),
                tasks: tasks.clone(),
            });
            core.emit_child_projections(&live, &workspace, &tasks);
        }
    }
    // A child's task tools write the parent's file (the shared task
    // model): the child's own fold would report no change, so the
    // projection is re-emitted from the parent — the parent's list and
    // the child's slice.
    if let Some(link) = live.agent.child_link() {
        core.emit_task_projection(&workspace, &live.cwd, &session, &link);
    }
    live.turn.store(false, Ordering::SeqCst);
}

impl Core {
    pub(crate) fn emit_queue(&self, live: &LiveSession) {
        // One locked scope: the event's fields are cloned out before the
        // guard drops, so a long emit cannot hold the locks. The items are
        // a projection of the agent's lane queue (the single source of
        // truth), not a second ledger.
        let (workspace, session, items) = {
            let meta = live
                .meta
                .lock()
                .expect("session meta: no panic while the lock is held");
            (
                meta.workspace.clone(),
                meta.id.clone(),
                live.agent.queued_items(),
            )
        };
        self.emit(Event::Queue {
            workspace,
            session,
            items,
        });
    }

    /// The session's task list, folded from its file, as a `task_changed`
    /// event (spec §8): the payload is a projection of the file, never a
    /// second source of truth; the store replaces on receive, so the GUI
    /// converges on the file's state without polling. A child session is
    /// projected from the parent's file (the shared task model) — its own
    /// file carries no task entries.
    pub(crate) fn emit_task_changed(&self, live: &LiveSession, session: &str) {
        let (workspace, cwd) = {
            let meta = live
                .meta
                .lock()
                .expect("session meta: no panic while the lock is held");
            (meta.workspace.clone(), live.cwd.clone())
        };
        if let Some(link) = live.agent.child_link() {
            self.emit_task_projection(&workspace, &cwd, session, &link);
            return;
        }
        let mut store = SessionStore::for_workspace(&cwd, session);
        if store.open().is_err() {
            return; // the file is gone; the next open rebuilds from nothing
        }
        let Ok(entries) = store.entries_range(0, usize::MAX) else {
            return; // a failed read is not an empty task list (torn-append race)
        };
        let tasks = crate::task::fold_entries(&entries);
        self.emit(Event::TaskChanged {
            workspace: workspace.clone(),
            session: session.to_owned(),
            tasks: tasks.clone(),
        });
        self.emit_child_projections(live, &workspace, &tasks);
    }

    /// A task change in the parent (the single source of truth) is emitted
    /// twice: the parent's own full list, and — for the named child — that
    /// child's projection (worker == the child), which fills the child's
    /// task pane. The link owns the parent's read (R3).
    pub(crate) fn emit_task_projection(
        &self,
        workspace: &str,
        cwd: &std::path::Path,
        child: &str,
        link: &ChildLink,
    ) {
        // The link's parent read: None = the parent's file can't be read —
        // a failed read is not an empty task list (torn-append race).
        let Some(tasks) = link.parent_tasks(cwd) else {
            return;
        };
        self.emit(Event::TaskChanged {
            workspace: workspace.to_owned(),
            session: link.parent_session().to_owned(),
            tasks: tasks.clone(),
        });
        let projected: Vec<crate::task::Task> = tasks
            .into_iter()
            .filter(|t| t.worker.as_ref().is_some_and(|w| w.session == child))
            .collect();
        if !projected.is_empty() {
            self.emit(Event::TaskChanged {
                workspace: workspace.to_owned(),
                session: child.to_owned(),
                tasks: projected,
            });
        }
    }
    /// Child-targeted `TaskChanged` for each of this session's children that
    /// owns a task: the child's pane is a projection of this session's
    /// list, filtered to worker == the child.
    fn emit_child_projections(
        &self,
        live: &LiveSession,
        workspace: &str,
        tasks: &[crate::task::Task],
    ) {
        let Some(sup) = live.agent.subagents() else {
            return;
        };
        for sid in sup.child_sessions() {
            let projected: Vec<crate::task::Task> = tasks
                .iter()
                .filter(|t| t.worker.as_ref().is_some_and(|w| w.session == sid))
                .cloned()
                .collect();
            if !projected.is_empty() {
                self.emit(Event::TaskChanged {
                    workspace: workspace.to_owned(),
                    session: sid,
                    tasks: projected,
                });
            }
        }
    }
}
