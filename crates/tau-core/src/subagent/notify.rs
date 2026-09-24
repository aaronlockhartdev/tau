use super::*;

impl Supervisor {
    /// Assign one of this session's tasks to a child (spec §5.3): the
    /// record copies into the child's session, which becomes the live
    /// record; the creator's copy becomes the status pointer. Both sides
    /// run through the sessions' own stores — one writer per session,
    /// never a second store on a live file (review B3). The copy is then
    /// delivered through the child's message path: a running child takes
    /// it as a steering round on its next call, a non-running child
    /// resumes with it (ADR-0001) — a bare copy races a running loop.
    pub fn assign_task(
        self: &Arc<Self>,
        task_id: &str,
        worker_session: &str,
    ) -> Result<(), String> {
        if task_id.is_empty() {
            return Err("task_assign: missing \"task\"".into());
        }
        let child = self
            .children
            .lock()
            .unwrap()
            .values()
            .find(|c| c.session_id == worker_session)
            .cloned()
            .ok_or_else(|| {
                format!("task_assign: {worker_session} is not a sub-agent of this session")
            })?;
        let parent = self
            .parent
            .lock()
            .unwrap()
            .clone()
            .ok_or("task_assign: no parent attached".to_owned())?;
        parent
            .with_task_store(|cstore| {
                child.agent.with_task_store(|wstore| {
                    crate::task::assign(
                        cstore,
                        wstore,
                        task_id,
                        worker_session,
                        &self.parent_session,
                    )
                })
            })
            .map(|_| ())?;
        let title = parent
            .with_task_store(|store| {
                crate::task::fold_entries(&store.entries_range(0, usize::MAX).unwrap_or_default())
                    .into_iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.title)
            })
            .unwrap_or_default();
        let note = if title.is_empty() {
            format!("Assigned task {task_id}")
        } else {
            format!("Assigned {task_id}: {title}")
        };
        self.message(&child.handle, Some(note), Lane::Steering)
            .map(|_| ())
    }

    pub(super) fn mark_failed(&self, child: &Arc<Child>, reason: String) {
        // The state entry itself can fail (storage down): nothing further
        // to do — the in-memory state already says failed.
        if let Err(e) = child.set_state(ChildState::Failed {
            reason: reason.clone(),
        }) {
            eprintln!("subagent state entry failed: {e}");
        }
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state: ChildState::Failed {
                reason: reason.clone(),
            },
            note: Some(reason.clone()),
            resume_contract: None,
        });
        // A failed child auto-notifies the parent to investigate (ADR-0001
        // wake rules: failed is always woken).
        self.bridge.wake(&WakeNotice {
            parent: self.parent_session.clone(),
            child: child.session_id.clone(),
            kind: WakeKind::Failed,
            text: reason,
            waiting_on: None,
            output: None,
        });
    }

    /// The child-side `parent_notify` (routed here from the child's loop).
    pub(super) fn notify(&self, handle: &str, args: &Value) -> String {
        // Shape-only validation (v0, review N9): the keys and their types
        // are checked, not a full schema — a semantic schema would be
        // speculative until the task gate lands. A malformed call is
        // rejected before the child lookup, even for an unknown handle.
        let Some(text) = args.get("text").and_then(Value::as_str) else {
            return "parent_notify: missing \"text\"".into();
        };
        let done = args.get("done").and_then(Value::as_bool).unwrap_or(false);
        let output = args.get("output").cloned();
        let waiting_on = match args.get("waiting_on").and_then(Value::as_str) {
            None => None,
            Some("parent") => Some(WaitingOn::Parent),
            Some("user") => Some(WaitingOn::User),
            Some("subagent") => Some(WaitingOn::Subagent),
            Some(other) => {
                return format!(
                    "parent_notify: waiting_on must be parent | user | subagent, got {other:?}"
                );
            }
        };

        if done && !matches!(output, Some(Value::Object(_))) {
            // done:true requires a structured output (the handoff-out,
            // ADR-0001: the child can never finish without one).
            return "parent_notify: done:true requires an object output".into();
        }
        if !done && output.is_some() {
            return "parent_notify: output requires done:true".into();
        }

        let Some(child) = self.children.lock().unwrap().get(handle).cloned() else {
            return format!("parent_notify: unknown child {handle}");
        };
        // A repeated `done` is a no-op (ADR-0001 quiescence): the child has
        // already terminated — no second state entry, no second wake.
        if matches!(child.state(), ChildState::Done { .. }) {
            return "already done: the parent has your result".into();
        }

        let mut payload = json!({ "event": "notify", "text": text, "done": done });
        if let Some(output) = &output {
            payload["output"] = output.clone();
        }
        if let Some(w) = waiting_on {
            payload["waiting_on"] = json!(w.as_str());
        }
        if let Err(e) = child.agent.append_entry(KIND_SUBAGENT, payload) {
            return format!("parent_notify: recording the note failed: {e}");
        }
        child.set_last_message(text);

        if done {
            // The task gate (spec §5.3): a done child cannot leave its
            // assigned task dangling in_progress — the resolution is
            // forced (completed / handed_off / blocked), and the
            // creator's pointer is mirrored. Runs before the state set so
            // the Done record and the wake see the final task state.
            self.resolve_assigned_task(&child, &output);
            // Quiescence, not death (ADR-0001): the loop ends, the
            // concurrency slot frees, and the parent is woken always.
            if let Err(e) = child.set_state(ChildState::Done {
                output: output.clone(),
            }) {
                return e;
            }
            self.bridge.state(&StateNotice {
                parent: self.parent_session.clone(),
                handle: child.handle.clone(),
                child: child.session_id.clone(),
                state: ChildState::Done {
                    output: output.clone(),
                },
                note: None,
                resume_contract: None,
            });
            self.bridge.wake(&WakeNotice {
                parent: self.parent_session.clone(),
                child: child.session_id.clone(),
                kind: WakeKind::Done,
                text: text.to_owned(),
                waiting_on: None,
                output,
            });
            "done: the parent has your result".into()
        } else {
            // A note parks the child. The declared (or default) wait
            // target decides the wake (ADR-0001 wake rules).
            let waiting_on = waiting_on.unwrap_or(WaitingOn::Parent);
            if let Err(e) = child.set_state(ChildState::Idle { waiting_on }) {
                return e;
            }
            self.bridge.state(&StateNotice {
                parent: self.parent_session.clone(),
                handle: child.handle.clone(),
                child: child.session_id.clone(),
                state: ChildState::Idle { waiting_on },
                note: Some(text.to_owned()),
                resume_contract: None,
            });
            if waiting_on == WaitingOn::Parent {
                // A parked parent (including a user-stopped one) is woken
                // with the child's stated need; user/subagent waits are
                // GUI-badge only.
                self.bridge.wake(&WakeNotice {
                    parent: self.parent_session.clone(),
                    child: child.session_id.clone(),
                    kind: WakeKind::Waiting,
                    text: text.to_owned(),
                    waiting_on: Some(WaitingOn::Parent),
                    output: None,
                });
            }
            "noted: you are parked".into()
        }
    }

    /// The child's assigned task, resolved on done (spec §5.3): the
    /// completion gate runs on the child's own store (the live record),
    /// and the creator's pointer is mirrored on the parent's own store —
    /// one store per session, never a second store on a live file.
    ///
    /// - all criteria satisfied → `done`
    /// - not satisfied, not blocked → `handed_off` (stays in_progress;
    ///   the output becomes the resume contract, the parent decides)
    /// - already blocked (the child called `task_block`) → stays blocked
    fn resolve_assigned_task(&self, child: &Child, output: &Option<Value>) {
        let resolved = child.agent.with_task_store(|store| {
            let entries = store.entries_range(0, usize::MAX).ok()?;
            let tasks = crate::task::fold_entries(&entries);
            // The live record is the task with a creator link (in v0 a
            // child carries at most one assigned task).
            let task = tasks.into_iter().find(|t| t.created_in.is_some())?;
            let output = output.as_ref().unwrap_or(&Value::Null);
            let status = if task.status == crate::task::STATUS_IN_PROGRESS {
                let gate = task
                    .criteria
                    .iter()
                    .all(|c| c.status == crate::task::CriterionStatus::Satisfied);
                if gate {
                    crate::task::finish(store, &task.id, false, None)
                } else {
                    crate::task::handoff(store, &task.id, output)
                }
                .ok()?
                .status
            } else {
                task.status.clone()
            };
            Some((task.id, status))
        });
        if let Some((id, status)) = resolved
            && let Some(parent) = self.parent.lock().unwrap().clone()
        {
            parent.with_task_store(|store| {
                // The creator's copy tracks the worker's state; a creator
                // without the copy is a no-op (mirror_status handles it).
                let _ = crate::task::mirror_status(store, &id, &status);
            });
        }
    }
}
