//! The turn-end completion-gate stop-hook, in its own module so `turn.rs`
//! stays under the 1000-LOC file-size limit (AGENTS.md).

use super::{AgentError, AgentSession, KIND_SYSTEM, Lane};

impl AgentSession {
    /// The turn-end stop-hook (spec §5.3): run the completion gate on a
    /// qualifying turn-end. Returns `true` when the turn ends here (a pass,
    /// a `gate_failed`, a `QUESTION:` skip, a no-tool turn, or no gate) and
    /// `false` when a failed check bounces a repair turn (the caller
    /// continues the loop).
    pub(super) async fn gate_turn_end(
        &self,
        text: &str,
        tool_calls: usize,
        retries: &mut u32,
    ) -> Result<bool, AgentError> {
        // The gate config + cwd, read under the session lock.
        let (gates, cwd) = {
            let inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            (inner.gates.clone(), inner.cwd.clone())
        };
        let Some(gates) = gates else {
            return Ok(true); // no gate configured: the turn stands
        };
        // A QUESTION: marker: the model is asking the user — the turn is
        // awaiting_input and the gate is skipped (spec §5.3).
        if crate::gate::is_question(text) {
            return Ok(true);
        }
        // A turn with no tool call is not qualifying: nothing to verify.
        if tool_calls == 0 {
            return Ok(true);
        }
        // A background job is live (#53): its completion will wake this
        // session with a new turn, so a check now would race the work —
        // the gate skips and re-qualifies when that wake arrives.
        if self.background_jobs().has_live() {
            return Ok(true);
        }
        // Run the check (honoring the deadline).
        let result = crate::gate::run(&gates, &cwd).await;
        if result.passed {
            // A pass clears any prior gate_failed: the live state is the
            // current gate status.
            self.set_gate_failed(false);
            return Ok(true); // the turn stands
        }
        // A failed check: count the attempt and record it (a system entry).
        *retries += 1;
        if *retries > gates.max_retries {
            // max_retries failed repairs: the session is gate_failed.
            self.append(
                KIND_SYSTEM,
                serde_json::json!({
                    "note": crate::gate::failed_note(&gates, &result, gates.max_retries)
                }),
            )?;
            self.set_gate_failed(true);
            return Ok(true); // end the turn
        }
        self.append(
            KIND_SYSTEM,
            serde_json::json!({
                "note": crate::gate::failure_note(&gates, &result, *retries)
            }),
        )?;
        // Bounce a repair turn: the steering message lands at the head of
        // the next LLM call in this same turn (spec §7).
        self.send(
            crate::gate::repair_message(&gates, &result, *retries, gates.max_retries),
            Lane::Steering,
        );
        Ok(false) // continue the loop for the repair
    }

    /// Set the live gate state (spec §5.3).
    pub(super) fn set_gate_failed(&self, failed: bool) {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        inner.gate_failed = failed;
    }
}
