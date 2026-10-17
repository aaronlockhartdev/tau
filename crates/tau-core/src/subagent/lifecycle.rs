use super::{
    Arc, Child, ChildState, Ordering, StoppedBy, Supervisor, TurnOrigin, WaitingOn, WakeKind,
};
use crate::agent::Lane;

impl Supervisor {
    /// `subagent_message` (and the GUI's send to a child): running → the
    /// text queues on the child's lane; non-running → resume (text omitted
    /// = a pure resume). One tool, state decides (ADR-0006). The reference
    /// is the child's name (its displayed title), with the session id and
    /// the internal handle accepted as fallbacks.
    pub fn message(
        self: &Arc<Self>,
        name_or_id: &str,
        text: Option<String>,
        lane: Lane,
    ) -> Result<String, String> {
        let child = self.resolve(name_or_id).ok_or_else(|| {
            format!("subagent_message: no sub-agent {name_or_id} in this session")
        })?;
        if child.state() == ChildState::Running {
            match &text {
                Some(text) => {
                    child.agent.send(text.clone(), lane);
                    child.set_last_message(text);
                    child.wake.notify_one();
                    Ok(format!("delivered to {}", child.name))
                }
                None => Ok(format!("sub-agent {} is already running", child.name)),
            }
        } else {
            // Resume from any non-running state (ADR-0001 supplement:
            // all non-running states are deliberately resumable; the
            // only asymmetry is the provenance the stop recorded).
            let was_quiescent = matches!(
                *child
                    .state
                    .lock()
                    .expect("child state: no panic while the lock is held"),
                ChildState::Stopped { .. } | ChildState::Failed { .. } | ChildState::Done { .. }
            );
            // A quiescent child holds no slot; the resume re-acquires
            // one against the cap.
            if was_quiescent {
                self.check_cap(true)?;
            }
            // A resume is a reply to a direct message: the work period it
            // starts is message-origin — a rest at its end is silent to the
            // parent (spec §5.2 turn-origin wake routing).
            child.set_origin(TurnOrigin::Message);
            child.reset_hints();
            let text = text.unwrap_or_else(|| "Continue from where you stopped.".to_owned());
            child.set_state(&ChildState::Running)?;
            child.wake.notify_one();
            self.emit_state(&child, ChildState::Running, Some("resumed".into()), None);
            child.agent.send(text, Lane::FollowUp);
            // A drive ended by stop/failure/done is gone: this resume
            // starts a fresh one (only an idle/awaiting drive persists and
            // wakes on its own).
            if was_quiescent {
                let drive_gen = child.drive_gen.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::spawn(Self::drive_loop(Arc::clone(self), child.clone(), drive_gen));
            }
            Ok(format!("resumed sub-agent {}", child.name))
        }
    }

    /// `subagent_stop` (and the GUI's stop): a running child is stopped —
    /// the in-flight stream is cut, the partial kept, a terminal `Stopped{by}`
    /// recorded (ADR-0001: deliberately resumable), its concurrency slot
    /// freed, and the parent woken. A non-running child is a no-op with a
    /// clear message — it already rests in the state that records how it
    /// got there, and a second stop would overwrite it.
    pub fn stop(self: &Arc<Self>, name_or_id: &str, by: StoppedBy) -> Result<String, String> {
        let child = self
            .resolve(name_or_id)
            .ok_or_else(|| format!("subagent_stop: no sub-agent {name_or_id} in this session"))?;
        if !matches!(child.state(), ChildState::Running) {
            return match child.state() {
                ChildState::Idle { .. } | ChildState::Awaiting => Ok(format!(
                    "sub-agent {} is resting — a message resumes it",
                    child.name
                )),
                other => Ok(format!(
                    "sub-agent {} is already {}",
                    child.name,
                    other.kind()
                )),
            };
        }
        // Cuts the child's stream at the next delta; the loop records
        // the partial as an interrupted entry.
        child.agent.stop();
        self.finish_stop(&child, by)
    }

    /// The stop's shared tail (the user stop and the parent's close/archive
    /// both end a drive this way): the terminal Stopped record, the
    /// drive's wake, and the state event (with the plan's resume contract).
    /// The parent gets the state event, not a wake (spec §5.2 wake rules:
    /// `stopped` → no parent wake — the parent already knows, or can ask
    /// with `subagent_state`).
    fn finish_stop(&self, child: &Arc<Child>, by: StoppedBy) -> Result<String, String> {
        let name = child.name.clone();
        let note = format!("stopped by {}", by.as_str());
        child.set_state(&ChildState::Stopped { by })?;
        // The drive ends at its loop head (running: after the interrupted
        // entry lands; parked: it wakes from its sleep and exits).
        child.wake.notify_one();
        // A stopped child with an assigned task carries the task's resume
        // contract in the stop event (spec §5.2) — the task lives in the
        // parent's file (the single source of truth).
        let resume_contract = self
            .child_task(child)
            .filter(|t| {
                t.status == crate::task::STATUS_IN_PROGRESS
                    || t.status == crate::task::STATUS_BLOCKED
            })
            .map(|t| crate::task::resume_contract(&t));
        self.emit_state(
            child,
            ChildState::Stopped { by },
            Some(note.clone()),
            resume_contract.clone(),
        );
        match resume_contract {
            Some(rc) => Ok(format!(
                "stopped sub-agent {name}; its assigned task stays live (resume contract: {})",
                serde_json::to_string(&rc).unwrap_or_default()
            )),
            None => Ok(format!("stopped sub-agent {name}")),
        }
    }

    /// The observed rest (spec §5.2): the drive drained and the child is
    /// still `Running` — the turn ended without a declaration the
    /// observation confirms. The state is *read* from the observed
    /// signals, in order: `gate_failed` → `failed` (auto-notifying the
    /// parent); a `QUESTION:` marker in the final output → `awaiting`;
    /// a complete plan — the gate having passed is the absence of
    /// `gate_failed` — → `done`, carrying the declared `parent_notify`
    /// output; else → `idle`, riding the declared `waiting_on` as its
    /// badge. The wake routes by turn origin: `done`/`failed` wake the
    /// parent always; an autonomous (spawn-origin) rest informs it; a
    /// message-origin rest is silent (the state event is the GUI badge).
    pub(super) fn settle_rest(&self, child: &Arc<Child>) {
        let final_text = child.agent.last_assistant_text().unwrap_or_default();
        if child.agent.gate_failed() {
            // The gate's repair retries are exhausted (spec §5.3): the
            // observed `failed`.
            self.mark_failed(
                child,
                "gate_failed: the completion gate did not pass".into(),
            );
            return;
        }
        // The declared target (a `parent_notify` hint) rides the observed
        // rest's wake as the badge — `None` when the child declared none.
        let declared_waiting = child.take_declared_waiting();
        let state = if crate::gate::is_question(&final_text) {
            ChildState::Awaiting
        } else if self.child_plan(child).complete() {
            ChildState::Done {
                output: child.take_declared_output(),
            }
        } else {
            ChildState::Idle {
                waiting_on: declared_waiting.unwrap_or(WaitingOn::Parent),
            }
        };
        child.set_rest_requested(false);
        if let Err(e) = child.set_state(&state) {
            tracing::warn!("subagent state entry failed: {e}");
        }
        self.emit_state(child, state.clone(), Some(final_text.clone()), None);
        let origin = child.origin();
        match &state {
            ChildState::Done { output } => {
                self.emit_wake(child, WakeKind::Done, final_text, None, output.clone());
            }
            ChildState::Failed { reason } => {
                self.emit_wake(child, WakeKind::Failed, reason.clone(), None, None);
            }
            // An autonomous pause wakes the delegating parent (the child
            // names what it has / needs in the final output); a reply to a
            // direct message stays silent — the sender is engaged.
            ChildState::Idle { .. } if origin == TurnOrigin::Spawn => {
                self.emit_wake(child, WakeKind::Waiting, final_text, declared_waiting, None);
            }
            ChildState::Awaiting if origin == TurnOrigin::Spawn => {
                self.emit_wake(child, WakeKind::Waiting, final_text, None, None);
            }
            _ => {}
        }
    }

    /// The child's plan file, parsed (the observed completion's source,
    /// spec §5.2): a missing or unreadable file is an empty plan — never
    /// complete (absence of observation is not proof of completion).
    fn child_plan(&self, child: &Child) -> crate::plan::Plan {
        let file = crate::plan::path(&self.cwd, &child.session_id);
        std::fs::read_to_string(file)
            .map(|raw| crate::plan::parse(&raw))
            .unwrap_or_default()
    }

    /// Stop one child by handle (the GUI closing the child session): the
    /// same terminal path as `stop` — a running child's stream is cut, and
    /// every child gets the terminal Stopped record, the drive's wake, and
    /// the state event. The parent gets the state event, not a wake (spec
    /// §5.2: `stopped` → no parent wake).
    pub fn stop_handle(self: &Arc<Self>, handle: &str, by: StoppedBy) -> Result<String, String> {
        let child = self
            .live_children()
            .into_iter()
            .find(|c| c.handle == handle)
            .ok_or_else(|| format!("no child {handle} in this session's supervisor"))?;
        if matches!(child.state(), ChildState::Running) {
            // Cuts the child's stream at the next delta; the loop records
            // the partial as an interrupted entry.
            child.agent.stop();
        }
        self.finish_stop(&child, by)
    }
    /// Soft-stop every child that still holds a drive (the parent session
    /// was closed, deleted, or archived: no work runs for a session that
    /// no longer exists): a running child is stopped as `stop` does, and
    /// a parked (idle) child's sleeping drive is ended — `stop` leaves a
    /// parked child alone, but a dead parent must not leave a drive behind.
    /// Quiescent children (done/stopped/failed) are left as-is — their
    /// record is complete and they stay resumable as standalone sessions.
    pub fn stop_all(self: &Arc<Self>, by: StoppedBy) {
        for child in self.live_children() {
            let _ = self.stop_handle(&child.handle, by);
        }
    }

    /// Whether the child's newest drive has exited (the archive's settle
    /// check: a stopped child's drive must be gone before its file moves).
    pub fn drive_quiescent(&self, handle: &str) -> bool {
        let children = self
            .children
            .lock()
            .expect("children map: no panic while the lock is held");
        let Some(child) = children.get(handle) else {
            return true;
        };
        child.last_ended_gen.load(Ordering::SeqCst) == child.drive_gen.load(Ordering::SeqCst)
    }
}
