use super::*;

impl Supervisor {
    /// All registered children (the snapshot's live-state handles).
    pub fn handles(&self) -> Vec<String> {
        self.children.lock().unwrap().keys().cloned().collect()
    }

    /// A child's structured inspection (the `subagent_state` tool and the
    /// protocol's `subagent_state` command).
    pub fn state_info(&self, name_or_id: &str) -> Option<SubagentInfo> {
        let child = self.resolve(name_or_id)?.clone();
        let (state, waiting_on) = match child.state() {
            ChildState::Idle { waiting_on } => (ChildState::Idle { waiting_on }, Some(waiting_on)),
            other => (other, None),
        };
        let usage = {
            let mut store = SessionStore::for_workspace(&self.cwd, &child.session_id);
            store
                .open()
                .ok()
                .and_then(|_| store.leaf().ok().flatten())
                .and_then(|leaf| {
                    let entries = store.entries_range(0, usize::MAX).ok()?;
                    let branch = om_integration::branch_entries(&entries, Some(&leaf.id));
                    branch
                        .iter()
                        .rev()
                        .find(|e| e.kind == crate::agent::KIND_ASSISTANT)
                        .and_then(|e| e.payload.get("usage"))
                        .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok())
                })
        };
        // The child's assigned task + its resume contract (spec §5.1):
        // read from the child's own entries (read-only — the writer stays
        // the child's session).
        let (task, resume_contract) = {
            let mut store = SessionStore::for_workspace(&self.cwd, &child.session_id);
            store
                .open()
                .ok()
                .and_then(|_| store.entries_range(0, usize::MAX).ok())
                .and_then(|entries| {
                    let tasks = crate::task::fold_entries(&entries);
                    let t = tasks.into_iter().find(|t| t.created_in.is_some())?;
                    Some((Some(t.clone()), Some(crate::task::resume_contract(&t))))
                })
                .unwrap_or((None, None))
        };
        Some(SubagentInfo {
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            agent_type: child.agent_type.clone(),
            context_mode: child.context_mode,
            state,
            waiting_on,
            last_message: child.last_message.lock().unwrap().clone(),
            usage,
            task,
            resume_contract,
        })
    }
}
