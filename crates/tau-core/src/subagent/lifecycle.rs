use super::*;

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
        match child.state() {
            ChildState::Running => match &text {
                Some(text) => {
                    child.agent.send(text.clone(), lane);
                    child.set_last_message(text);
                    child.wake.notify_one();
                    Ok(format!("delivered to {}", child.name))
                }
                None => Ok(format!("sub-agent {} is already running", child.name)),
            },
            _ => {
                // Resume from any non-running state (ADR-0001 supplement:
                // all non-running states are deliberately resumable; the
                // only asymmetry is the provenance the stop recorded).
                let was_quiescent = matches!(
                    *child.state.lock().unwrap(),
                    ChildState::Stopped { .. }
                        | ChildState::Failed { .. }
                        | ChildState::Done { .. }
                );
                // A quiescent child holds no slot; the resume re-acquires
                // one against the cap.
                if was_quiescent {
                    self.check_cap(true)?;
                }
                let text = text.unwrap_or_else(|| "Continue from where you stopped.".to_owned());
                child.set_state(ChildState::Running)?;
                child.wake.notify_one();
                self.bridge.state(&StateNotice {
                    parent: self.parent_session.clone(),
                    handle: child.handle.clone(),
                    child: child.session_id.clone(),
                    state: ChildState::Running,
                    note: Some("resumed".into()),
                    resume_contract: None,
                });
                child.agent.send(text, Lane::FollowUp);
                // A drive ended by stop/failure/done is gone: this resume
                // starts a fresh one (only an idle drive persists and
                // wakes on its own). The nudge budget resets with the new
                // work period.
                if was_quiescent {
                    let drive_gen = child.drive_gen.fetch_add(1, Ordering::SeqCst) + 1;
                    tokio::spawn(Self::drive_loop(Arc::clone(self), child.clone(), drive_gen));
                }
                child.nudge_sent.store(false, Ordering::SeqCst);
                Ok(format!("resumed sub-agent {}", child.name))
            }
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
                ChildState::Idle { .. } => Ok(format!(
                    "sub-agent {} is parked — a message resumes it",
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
    /// drive's wake, the state event (with the task's resume contract),
    /// and the parent's wake.
    fn finish_stop(&self, child: &Arc<Child>, by: StoppedBy) -> Result<String, String> {
        let name = child.name.clone();
        let note = format!("stopped by {}", by.as_str());
        child.set_state(ChildState::Stopped { by })?;
        // The drive ends at its loop head (running: after the interrupted
        // entry lands; parked: it wakes from its sleep and exits).
        child.wake.notify_one();
        // A stopped child with an assigned task carries the task's resume
        // contract in the stop event (spec §5.2).
        let resume_contract = child.agent.with_task_store(|store| {
            let entries = store.entries_range(0, usize::MAX).ok()?;
            crate::task::fold_entries(&entries)
                .into_iter()
                .find(|t| t.created_in.is_some())
                .filter(|t| {
                    t.status == crate::task::STATUS_IN_PROGRESS
                        || t.status == crate::task::STATUS_BLOCKED
                })
                .map(|t| crate::task::resume_contract(&t))
        });
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state: ChildState::Stopped { by },
            note: Some(note.clone()),
            resume_contract: resume_contract.clone(),
        });
        // The parent is told the child is stopped: its model's view of the
        // delegation must not keep the child running.
        self.bridge.wake(&WakeNotice {
            parent: self.parent_session.clone(),
            child: child.session_id.clone(),
            kind: WakeKind::Stopped,
            text: note,
            waiting_on: None,
            output: None,
        });
        match resume_contract {
            Some(rc) => Ok(format!(
                "stopped sub-agent {name}; its assigned task stays live (resume contract: {})",
                serde_json::to_string(&rc).unwrap_or_default()
            )),
            None => Ok(format!("stopped sub-agent {name}")),
        }
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
            if matches!(child.state(), ChildState::Running) {
                child.agent.stop();
            }
            let _ = self.finish_stop(&child, by);
        }
    }

    /// Whether the child's newest drive has exited (the archive's settle
    /// check: a stopped child's drive must be gone before its file moves).
    pub fn drive_quiescent(&self, handle: &str) -> bool {
        let children = self.children.lock().unwrap();
        let Some(child) = children.get(handle) else {
            return true;
        };
        child.last_ended_gen.load(Ordering::SeqCst) == child.drive_gen.load(Ordering::SeqCst)
    }
}
