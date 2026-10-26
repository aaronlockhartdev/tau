use super::{AgentError, AgentSession, OmStatusHook, PlanStateHook, QueueEventHook, TurnConfig};
use crate::provider::ToolSpec;
use crate::session::{EntryEventHook, SessionStore};
use serde_json::Value;
use tau_protocol::{Diagnostic, EntryKind};

impl AgentSession {
    /// The session store's header timestamp (epoch ms).
    pub(crate) fn store_created(&self) -> u64 {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .store
            .created()
    }

    /// Run one operation against this session's own store — the
    /// single-writer surface for other components (review B3).
    pub(crate) fn with_store<R>(&self, f: impl FnOnce(&mut SessionStore) -> R) -> R {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        f(&mut inner.store)
    }

    #[cfg(test)]
    /// Test-only OM state injection (the sub-agent module's tests).
    pub(crate) fn set_om(&self, om: Option<crate::om_integration::OmState>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .om = om;
    }

    /// The OM-run observer (the app's `om_status` emitter); `None` clears it.
    pub(crate) fn set_om_status_hook(&self, hook: Option<OmStatusHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .om_status_hook = hook;
    }

    pub(crate) fn set_queue_event_hook(&self, hook: Option<QueueEventHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .queue_event_hook = hook;
    }

    /// The plan-state reader (design: the plan file is the source of
    /// truth); `None` clears it.
    pub(crate) fn set_plan_hook(&self, hook: Option<PlanStateHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .plan_hook = hook;
    }

    /// The job-completion hook (#53): the harness wires it to the session
    /// wake (steering-lane notification + turn start), the way it wires the
    /// sub-agent bridge. The core stays transport-free: the hook is the seam.
    pub(crate) fn set_job_notify_hook(
        &self,
        hook: Option<crate::tools::bgjobs::JobCompletionHook>,
    ) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .job_notify_hook = hook;
    }

    /// The live entry sink, set on this session's store (the append choke
    /// point); `None` clears it.
    pub(crate) fn set_entry_event_hook(&self, hook: Option<EntryEventHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .store
            .set_entry_event_hook(hook);
    }

    /// The wire-only upsert sink (ADR-0008); `None` clears it.
    pub(crate) fn set_entry_upsert_hook(&self, hook: Option<EntryEventHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .entry_upsert_hook = hook;
    }

    /// The session's model for the next turn's calls (the `session_set_model`
    /// command's live half; provider resolution happens at the call).
    pub(crate) fn set_model(&self, model: String) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .model = model;
    }

    /// The turn's provider options (spec §12, #35): re-derived when the
    /// session's model changes, so clamping and reasoning levels track the
    /// active model.
    pub(crate) fn set_turn_config(&self, turn: TurnConfig) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .turn = turn;
    }

    /// Append a record entry against the active leaf (the sub-agent
    /// lifecycle records, ticket #23).
    pub(crate) fn append_entry(&self, kind: EntryKind, payload: Value) -> Result<(), AgentError> {
        self.append(kind, payload)
    }
    /// Record a diagnostic fact about this session (ADR-0034). This is the
    /// only writer of the channel: the entry is the durable record the GUI,
    /// a closed session, and a restarted harness all read, and the log line
    /// beside it is a mirror, never the source.
    pub(crate) fn diagnostic(&self, fact: &Diagnostic) -> Result<(), AgentError> {
        match fact {
            Diagnostic::ModelChanged { .. } => {
                tracing::info!(code = fact.code(), "session diagnostic");
            }
            other => tracing::warn!(code = other.code(), "session diagnostic"),
        }
        self.append_entry(EntryKind::Diagnostic, fact.to_value())
    }

    pub(crate) fn model(&self) -> String {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .model
            .clone()
    }

    pub(crate) fn tools(&self) -> Vec<ToolSpec> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .tools
            .clone()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn system_prompt(&self) -> String {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .system_prompt
            .clone()
    }

    /// The session's OM state (ticket #22); `None` = OM disabled.
    pub(crate) fn om_state(&self) -> Option<crate::om_integration::OmState> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .om
            .clone()
    }

    /// The live gate state (spec §5.3): `true` when the most recent
    /// gate-qualified turn failed after exhausting `max_retries` repairs.
    #[must_use]
    pub(crate) fn gate_failed(&self) -> bool {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .gate_failed
    }

    /// The final assistant text on the active branch (spec §5.2: the
    /// observed `QUESTION:` marker lives in the child's final output).
    /// `None` when the branch has no assistant entry or the read fails.
    #[must_use]
    pub(crate) fn last_assistant_text(&self) -> Option<String> {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        let branch = inner.store.branch().ok()?;
        branch
            .iter()
            .rev()
            .find(|e| e.kind == EntryKind::Assistant)
            .and_then(|e| e.payload.get("text"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
}
