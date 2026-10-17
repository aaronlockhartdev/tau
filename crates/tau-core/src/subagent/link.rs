use super::Supervisor;
use super::states::StoppedBy;
use crate::agent::Lane;
use serde_json::Value;
use std::sync::Arc;

/// The child-side link a child's loop carries: routes `parent_notify` to
/// the child's supervisor.
pub struct ChildLink {
    pub(crate) supervisor: Arc<Supervisor>,
    pub(crate) handle: String,
}

impl ChildLink {
    #[must_use]
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// The parent's session id: the supervisor's own — the handle is
    /// opaque (R3), never parsed for its parent.
    #[must_use]
    pub fn parent_session(&self) -> &str {
        self.supervisor.parent_session()
    }

    /// The `parent_notify` tool handler (ticket #23): a secondary hint the
    /// observed rest confirms or overrides (spec §5.2, ADR-0001 revision).
    /// `done:true` requires a structured output and requests the turn's end;
    /// a note records the declared wait target for the rest's badge.
    #[must_use]
    pub fn notify(&self, args: &Value) -> String {
        self.supervisor.notify(&self.handle, args)
    }

    /// A message to this child, routed through the parent's supervisor
    /// (the GUI's send to a child, ADR-0001): a running child takes it
    /// on its lane; a non-running one is resumed with it.
    pub fn send(&self, text: String, lane: Lane) -> Result<String, String> {
        self.supervisor.message(&self.handle, Some(text), lane)
    }

    /// Stop this child through the parent's supervisor (the GUI's stop):
    /// the terminal `Stopped{by: User}` record, the freed slot, and the
    /// state event (no parent wake — spec §5.2).
    pub fn stop(&self) -> Result<String, String> {
        self.supervisor.stop(&self.handle, StoppedBy::User)
    }

    /// Whether the child is still `Running` (the loop's quiescence check,
    /// ADR-0001: the core auto-terminates a child's loop on done).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.supervisor.child_running(&self.handle)
    }

    /// A `parent_notify {done}` rest request pending the turn's end (the
    /// loop's quiescence check, spec §5.2's fast path).
    #[must_use]
    pub fn rest_requested(&self) -> bool {
        self.supervisor
            .children
            .lock()
            .expect("children map: no panic while the lock is held")
            .get(&self.handle)
            .is_some_and(|c| c.rest_requested())
    }
}
