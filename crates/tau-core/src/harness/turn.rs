//! The turn boundary and the post-turn reconciliation: the session file is
//! the record, and the events it implies are derived here after `process()`
//! drains the queue.

use super::{
    Arc, Core, Entry, Event, LiveSession, Ordering, SessionStore, SystemEventKind, Value,
    derive_turn, entry_to_view, owner_of, usage_of,
};
use crate::agent::ContextGate;
use tau_protocol::{Diagnostic, EntryKind};

/// The settlement guarantee (#267): the one mechanism that clears
/// `live.turn` on *any* exit of a turn — normal completion, early
/// return, or a task that panics and is dropped. Without it a panicked
/// `run_turn` left the flag `true` forever: the session showed "running"
/// and every send steered into a turn that would never end.
///
/// It is `RefUnwindSafe` on purpose: the guard must be allowed to unwind
/// out of the panicked task, because its `Drop` is what clears the flag
/// on that task before the panic is caught at the `JoinHandle`.
struct TurnSettleGuard {
    live: Arc<LiveSession>,
}

impl Drop for TurnSettleGuard {
    fn drop(&mut self) {
        self.live.turn.store(false, Ordering::SeqCst);
    }
}

impl std::panic::RefUnwindSafe for TurnSettleGuard {}

/// The post-turn reconciliation (spec §8 idempotent updates): the session
/// file is the record; the events it implies are derived here, after
/// `process()` drained the queue.
#[allow(clippy::too_many_lines)] // one reconciliation pass over the turn's record; splitting is refactoring
pub(crate) async fn run_turn(core: Arc<Core>, live: Arc<LiveSession>) {
    // #261: mark the turn running for every session here, not just the
    // dispatch/wake paths that check-and-set before spawning. A child's turn
    // is driven straight from the supervisor (`TurnChildDriver`) with no
    // prior set, so its `turn` flag was never true and its snapshot always
    // reported `Idle` — the GUI's `openSession` then clobbered a running
    // child's badge. For a parent session the flag is already true from that
    // check-and-set, so this is an idempotent no-op there.
    let settle = TurnSettleGuard {
        live: Arc::clone(&live),
    };
    live.turn.store(true, Ordering::SeqCst);
    core.rederive_turn(&live);
    let mut store = SessionStore::for_workspace(&live.cwd, &live.meta.lock().id);
    if let Err(e) = store.open() {
        // The send is already accepted (the GUI shows it as the turn):
        // a failed open must surface, or every send to a corrupted
        // session vanishes without a trace. The turn-starting message
        // also stays in the agent's queue, so the next successful turn
        // delivers it, and the queue's projection shows it — nothing is
        // re-queued anywhere.
        let (workspace, id) = {
            let meta = live.meta.lock();
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
        drop(settle);
        return;
    }
    // The archive sets the meta flag before its file moves (ADR-0005):
    // a turn that started in that window dies here, at its first write —
    // no headerless file, no partial turn in the archive.
    if live.meta.lock().archived {
        let (workspace, id) = {
            let meta = live.meta.lock();
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
        drop(settle);
        return;
    }
    let start = store.leaf().ok().flatten().map(|e| e.id);
    let calls_before = live.provider.calls.lock().len();

    if let Err(e) = live.agent.process().await {
        // One locked scope: two meta locks in one expression would be a
        // same-thread re-entrant deadlock (the first guard lives until the
        // statement ends).
        let (workspace, id) = {
            let meta = live.meta.lock();
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
        // No cursor means the session's first turn: the whole log is its
        // delta, and the store has no read wider than a cursor and narrower
        // than a position range (#302, spec §3).
        None => store.entries_range(0, usize::MAX).unwrap_or_default(),
    };

    let workspace = live.meta.lock().workspace.clone();
    let session = live.meta.lock().id.clone();
    let mut assistant_index = 0usize;
    let mut queue_changed = false;
    for entry in &new {
        match entry.kind {
            EntryKind::User => {
                // A delivered message has left the agent's queue (the
                // process loop drained it): the post-turn Queue event is
                // projected from the queue, so the GUI converges without a
                // second ledger to remove from.
                queue_changed = true;
            }
            EntryKind::Assistant => {
                let call_id = live
                    .provider
                    .calls
                    .lock()
                    .get(calls_before + assistant_index)
                    .cloned();
                assistant_index += 1;
                let interrupted = entry
                    .payload
                    .get("interrupted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let usage = entry.payload.get("usage").and_then(usage_of);
                // #224: the server's decode-window duration, recorded with the
                // entry at turn settlement, rides the StreamEnd.
                let duration_secs = entry.payload.get("duration_secs").and_then(Value::as_f64);
                // A call's end is emitted here, at turn settlement — after
                // the turn's om_turn_end pass — so `StreamEnd` means the
                // turn is over and the session accepts a new turn (issue
                // #82). A stream cut before its Completed frame has the
                // same shape: the partial stands as an interrupted end
                // (spec §6/§7).
                if let Some(call_id) = call_id {
                    core.emit(Event::StreamEnd {
                        workspace: workspace.clone(),
                        session: session.clone(),
                        call_id,
                        interrupted,
                        usage,
                        duration_secs,
                    });
                }
                if let Some(u) = usage {
                    live.meta.lock().usage = Some(u);
                }
            }
            EntryKind::Tool => {
                // The live hook already upserted this entry; the post-turn
                // pass re-emits the file line (a lossy-channel dedup,
                // idempotent by construction).
                core.emit(Event::EntryUpsert {
                    workspace: workspace.clone(),
                    session: session.clone(),
                    entry: entry_to_view(entry),
                });
            }
            // Record kinds imply no event here; listed so a new kind forces a
            // decision rather than defaulting silently.
            EntryKind::System
            | EntryKind::Om
            | EntryKind::Subagent
            | EntryKind::SpawnSnapshot
            | EntryKind::Diagnostic
            | EntryKind::Other(_) => {}
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

    // A call that produced no entry gets its end here: a cut stream is an
    // interrupted end, a completed empty call a clean one — the GUI's live
    // bubble must close either way (N10).
    {
        let (calls, completed) = {
            let calls = live.provider.calls.lock();
            let completed = live.provider.completed.lock();
            (calls, completed)
        };
        let new_calls = calls.len() - calls_before;
        for i in assistant_index..new_calls {
            let call_id = calls[calls_before + i].clone();
            let completed = completed.get(&call_id).copied().unwrap_or(false);
            core.emit(Event::StreamEnd {
                workspace: workspace.clone(),
                session: session.clone(),
                call_id,
                interrupted: !completed,
                usage: None,
                duration_secs: None,
            });
        }
    }

    // The active branch is what the loop appended against.
    if let Ok(leaf) = store.leaf()
        && let Some(leaf) = leaf
    {
        live.meta.lock().leaf = Some(leaf.id);
    }
    if queue_changed {
        core.emit_queue(&live);
    }
    drop(settle);
}

impl Core {
    /// The turn boundary's config re-derivation (spec §12, #337): a session takes
    /// the live config at the start of its next turn, never mid-stream. A model an
    /// edit removes is the ADR-0034 case — the gate goes missing and the record
    /// says so once (the ready→missing edge), while the session keeps running on
    /// the model it names.
    pub(crate) fn rederive_turn(&self, live: &LiveSession) {
        let (workspace, session, model) = {
            let meta = live.meta.lock();
            (meta.workspace.clone(), meta.id.clone(), live.agent.model())
        };
        let Ok(ws) = self.workspace(&workspace) else {
            return;
        };
        let config = self.workspace_config(&ws);
        // #336's route, read from the reloaded config: taking the config's first
        // provider (the pre-#336 shape) read a second provider's model out of the
        // first provider's model table, where its facts are absent — every reload
        // dropped a gate the model's own provider declares.
        let Ok(owner) = owner_of(&config, &model) else {
            // Refused here as at `SessionSetModel`: the turn keeps the facts it has.
            return;
        };
        let turn = derive_turn(&config, owner.provider, &model, &session);
        let gate = turn.context_gate();
        let was_ready = live.agent.current_turn().context_gate().is_ready();
        live.agent.set_turn_config(turn.clone());
        // The memory passes follow the config too (#348, #337): a `[om].om_model`
        // edit reaches the next pass, and a route the edit killed is recorded once
        // as `om_failed { stage: "route" }`.
        live.agent.set_om_route(self.om_route(&config, owner));
        // A child inherits the model its parent has, so the config's move reaches
        // the pair the supervisor hands a model-less child (#336).
        if let Some(sup) = live.agent.subagents() {
            sup.set_inherited(model.clone(), turn);
        }
        if let ContextGate::Missing { reasons } = gate {
            // One record per transition, not one per turn: a config that keeps
            // the model bare must not append to the session file every turn.
            if was_ready
                && let Err(e) = live.agent.diagnostic(&Diagnostic::GateUnavailable {
                    model,
                    missing: reasons,
                })
            {
                tracing::warn!("record the gate loss: {e}");
            }
        }
    }
    pub(crate) fn emit_queue(&self, live: &LiveSession) {
        // One locked scope: the event's fields are cloned out before the
        // guard drops, so a long emit cannot hold the locks. The items are
        // a projection of the agent's lane queue (the single source of
        // truth), not a second ledger.
        let (workspace, session, items) = {
            let meta = live.meta.lock();
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
}
