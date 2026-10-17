use super::{ChildState, SessionStore, SubagentInfo, Supervisor, Usage, WaitingOn};
use crate::om_integration;

impl Supervisor {
    /// All registered children (the snapshot's live-state handles).
    pub fn handles(&self) -> Vec<String> {
        self.children
            .lock()
            .expect("children map: no panic while the lock is held")
            .keys()
            .cloned()
            .collect()
    }

    /// A child's structured inspection (the `subagent_state` tool and the
    /// protocol's `subagent_state` command).
    pub fn state_info(&self, name_or_id: &str) -> Option<SubagentInfo> {
        let child = self.resolve(name_or_id)?.clone();
        let (state, waiting_on) = match child.state() {
            ChildState::Idle { waiting_on } => (ChildState::Idle { waiting_on }, Some(waiting_on)),
            // The `QUESTION:` marker: the child is asking the user.
            ChildState::Awaiting => (ChildState::Awaiting, Some(WaitingOn::User)),
            other => (other, None),
        };
        let usage = {
            let mut store = SessionStore::for_workspace(&self.cwd, &child.session_id);
            store
                .open()
                .ok()
                .and_then(|()| store.leaf().ok().flatten())
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
        // The child's plan file (spec §5.3): its open-step resume contract.
        let resume_contract =
            std::fs::read_to_string(crate::plan::path(&self.cwd, &child.session_id))
                .ok()
                .and_then(|raw| crate::plan::resume_contract(&crate::plan::parse(&raw)));
        Some(SubagentInfo {
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            agent_type: child.agent_type.clone(),
            context_mode: child.context_mode,
            state,
            waiting_on,
            last_message: child
                .last_message
                .lock()
                .expect("child last message: no panic while the lock is held")
                .clone(),
            usage,
            resume_contract,
        })
    }
}
