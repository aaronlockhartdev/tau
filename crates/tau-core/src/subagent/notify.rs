use super::{
    Arc, Child, ChildState, KIND_SUBAGENT, ResumeContract, StateNotice, Supervisor, Value,
    WaitingOn, WakeKind, WakeNotice,
};
use serde_json::json;

impl Supervisor {
    /// Emit a child state transition: the identity (parent, handle, child)
    /// comes from the supervisor + the child; the caller supplies the new
    /// state and its note / resume contract.
    pub(super) fn emit_state(
        &self,
        child: &Arc<Child>,
        state: ChildState,
        note: Option<String>,
        resume_contract: Option<ResumeContract>,
    ) {
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state,
            note,
            resume_contract,
        });
    }

    /// Emit a parent wake: the identity (parent, child) comes from the
    /// supervisor + the child; the caller supplies the wake kind + payload.
    pub(super) fn emit_wake(
        &self,
        child: &Arc<Child>,
        kind: WakeKind,
        text: String,
        waiting_on: Option<WaitingOn>,
        output: Option<Value>,
    ) {
        self.bridge.wake(&WakeNotice {
            parent: self.parent_session.clone(),
            child: child.session_id.clone(),
            kind,
            text,
            waiting_on,
            output,
        });
    }

    pub(super) fn mark_failed(&self, child: &Arc<Child>, reason: String) {
        // The state entry itself can fail (storage down): nothing further
        // to do — the in-memory state already says failed.
        if let Err(e) = child.set_state(&ChildState::Failed {
            reason: reason.clone(),
        }) {
            tracing::warn!("subagent state entry failed: {e}");
        }
        self.emit_state(
            child,
            ChildState::Failed {
                reason: reason.clone(),
            },
            Some(reason.clone()),
            None,
        );
        // A failed child auto-notifies the parent to investigate (ADR-0001
        // wake rules: failed is always woken).
        self.emit_wake(child, WakeKind::Failed, reason, None, None);
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

        let Some(child) = self
            .children
            .lock()
            .expect("children map: no panic while the lock is held")
            .get(handle)
            .cloned()
        else {
            return format!("parent_notify: unknown child {handle}");
        };

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
            // Demoted to a secondary hint (spec §5.2, ADR-0001 revision):
            // the declaration no longer drives the state. It records the
            // output an observed `done` will carry, and requests the turn's
            // end (the loop's quiescence check) so no further provider
            // calls are spent — the observation at the turn end confirms a
            // complete plan (gate passed) as `done`, or overrides it.
            // No state change and no wake here: the child→parent effect is
            // the observed rest's value, applied by the parent.
            child.declare_output(output.clone());
            child.set_rest_requested(true);
            "noted: your result is recorded; finish the turn".into()
        } else {
            // A note: the declared wait target (when given) rides the
            // observed rest as the `idle` state's badge. The `QUESTION:`
            // marker in the final output is what makes it `awaiting`.
            if let Some(w) = waiting_on {
                child.declare_waiting(w);
            }
            "noted".into()
        }
    }
}
