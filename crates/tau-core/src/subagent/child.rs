use super::KIND_SUBAGENT;
use super::states::{ChildState, ContextMode, TurnOrigin, WaitingOn};
use crate::agent::AgentSession;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One child's live record.
pub struct Child {
    pub handle: String,
    /// The child's displayed name (its session header title): the name
    /// the agent and the GUI address it by; the session id stays the
    /// machine key.
    pub name: String,
    pub session_id: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
    pub agent: Arc<AgentSession>,
    /// The child session's created timestamp (epoch ms): the snapshot's
    /// spawn-order sort key (#226 variant).
    pub created: u64,
    pub(super) state: Mutex<ChildState>,
    /// The turn origin (spec §5.2 wake rules): set at each drive start
    /// (spawn = `Spawn`, a resume = `Message`); a rest's wake routing
    /// reads it. Per-drive, not sticky.
    pub(super) origin: Mutex<TurnOrigin>,
    /// A `parent_notify {done: true}` pending the observed rest (spec
    /// §5.2): the fast path ends the turn after the declaring tool round
    /// so the loop spends no further provider calls; the observation at
    /// the turn end confirms or overrides. Cleared when consumed.
    pub(super) rest_requested: AtomicBool,
    /// The declared `done` output (the `parent_notify` hint, spec §5.2):
    /// the payload an observed `done` carries. Per-drive: reset at each
    /// drive start so a stale declaration can't ride a later rest.
    pub(super) declared_output: Mutex<Option<Value>>,
    /// The declared `waiting_on` of a resting note (the hint an observed
    /// `idle` rides as its badge). Per-drive, like the output.
    pub(super) declared_waiting: Mutex<Option<WaitingOn>>,
    pub(super) last_message: Mutex<Option<String>>,
    /// Drive generation: each drive spawn bumps it; a superseded drive's
    /// loop sees the mismatch and bails before running another round (the
    /// stop-then-resume window between `drive()` returning and the loop's
    /// state check — two drives must never run the child concurrently).
    pub(super) drive_gen: AtomicUsize,
    /// The generation of the last drive whose loop exited: the archive
    /// path quiesces a stopped child on this before moving its file (the
    /// stop's interrupted entry must land in the file, not a recreated
    /// fragment).
    pub(super) last_ended_gen: AtomicUsize,
    /// The idle drive sleeps on this (review N8): a message or a stop
    /// wakes it, so an idle child costs no polling. `Notify`'s permit
    /// makes a wake that lands before the drive arms its future unlost.
    pub(super) wake: tokio::sync::Notify,
}

impl Child {
    pub(super) fn state(&self) -> ChildState {
        self.state
            .lock()
            .expect("child state: no panic while the lock is held")
            .clone()
    }

    pub(super) fn set_state(&self, state: &ChildState) -> Result<(), String> {
        *self
            .state
            .lock()
            .expect("child state: no panic while the lock is held") = state.clone();
        // The record carries the discriminant plus the non-output variant
        // fields only: done's output lives in the notify record (the
        // report), not twice.
        let payload = match state {
            ChildState::Running => json!({ "event": "state", "state": "running" }),
            ChildState::Idle { waiting_on } => json!({
                "event": "state",
                "state": "idle",
                "waiting_on": waiting_on.as_str(),
            }),
            ChildState::Awaiting => json!({ "event": "state", "state": "awaiting" }),
            ChildState::Done { .. } => json!({ "event": "state", "state": "done" }),
            ChildState::Failed { reason } => json!({
                "event": "state",
                "state": "failed",
                "reason": reason,
            }),
            ChildState::Stopped { by } => json!({
                "event": "state",
                "state": "stopped",
                "by": by.as_str(),
            }),
        };
        self.agent
            .append_entry(KIND_SUBAGENT, payload)
            .map_err(|e| format!("subagent state entry failed: {e}"))
    }

    pub(super) fn set_last_message(&self, text: &str) {
        *self
            .last_message
            .lock()
            .expect("child last message: no panic while the lock is held") = Some(text.to_owned());
    }

    /// The turn origin (the wake routing's input, spec §5.2).
    #[must_use]
    pub(crate) fn origin(&self) -> TurnOrigin {
        *self
            .origin
            .lock()
            .expect("child origin: no panic while the lock is held")
    }

    /// Set the turn origin (each drive start: spawn or resume).
    pub(crate) fn set_origin(&self, origin: TurnOrigin) {
        *self
            .origin
            .lock()
            .expect("child origin: no panic while the lock is held") = origin;
    }

    /// A `done` declaration requests the turn's end (the fast path, spec
    /// §5.2): the loop breaks after the declaring tool round, and the
    /// observed rest settles the state. `true` when a rest is pending.
    #[must_use]
    pub(crate) fn rest_requested(&self) -> bool {
        self.rest_requested.load(Ordering::SeqCst)
    }

    pub(crate) fn set_rest_requested(&self, requested: bool) {
        self.rest_requested.store(requested, Ordering::SeqCst);
    }

    /// Record the declared `done` output (the hint the observed rest
    /// consumes).
    pub(crate) fn declare_output(&self, output: Option<Value>) {
        *self
            .declared_output
            .lock()
            .expect("child declared output: no panic while the lock is held") = output;
    }

    /// Take the declared output (the observed `done`'s payload): `None`
    /// when the rest is consumed or no declaration was made.
    #[must_use]
    pub(crate) fn take_declared_output(&self) -> Option<Value> {
        self.declared_output
            .lock()
            .expect("child declared output: no panic while the lock is held")
            .take()
    }

    /// Record the declared `waiting_on` of a resting note.
    pub(crate) fn declare_waiting(&self, waiting_on: WaitingOn) {
        *self
            .declared_waiting
            .lock()
            .expect("child declared waiting: no panic while the lock is held") = Some(waiting_on);
    }

    /// Take the declared `waiting_on` (the observed `idle`'s badge hint).
    #[must_use]
    pub(crate) fn take_declared_waiting(&self) -> Option<WaitingOn> {
        self.declared_waiting
            .lock()
            .expect("child declared waiting: no panic while the lock is held")
            .take()
    }

    /// A fresh work period (each drive start): the prior period's hints
    /// are gone — a stale declaration must not ride a later rest.
    pub(crate) fn reset_hints(&self) {
        self.rest_requested.store(false, Ordering::SeqCst);
        *self
            .declared_output
            .lock()
            .expect("child declared output: no panic while the lock is held") = None;
        *self
            .declared_waiting
            .lock()
            .expect("child declared waiting: no panic while the lock is held") = None;
    }
}
