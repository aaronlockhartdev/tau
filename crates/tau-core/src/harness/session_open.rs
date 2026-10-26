//! What a session's record already says, and what opening it adds (ADR-0034).
//!
//! The audit channel has one writer once an agent exists
//! ([`crate::agent::AgentSession::diagnostic`]); a session being opened has no
//! agent yet, so this module is the one place that appends the kind without it.

use super::ProtocolError;
use crate::agent::TurnConfig;
use crate::diagnostics::{self, Diagnostics};
use crate::session::SessionStore;
use tau_protocol::Diagnostic;

/// What the session file already says, in one walk: `None` when the session
/// has no entries yet, so there is nothing to have changed.
pub(crate) fn facts(store: &mut SessionStore) -> Result<Option<Diagnostics>, ProtocolError> {
    diagnostics::project_store(store).map_err(|e| ProtocolError::Other {
        message: e.to_string(),
    })
}

/// Record the model a session opens with, and whether a context gate exists
/// for it (ADR-0034). The effective model is the config's whenever the record
/// names none, so this is where a config-level model change — invisible in the
/// file before this ticket — leaves a record. A model whose declared facts
/// leave no gate is recorded once per model, never once per request.
/// `facts` is what the record already said, from [`facts`].
pub(crate) fn record_at_open(
    store: &mut SessionStore,
    facts: Option<&Diagnostics>,
    model: &str,
    turn: &TurnConfig,
) -> Result<(), ProtocolError> {
    let Some(facts) = facts else {
        // A fresh session has nothing to have changed, so it gains no
        // `model_changed`. The gate gap is not recorded here either: a session
        // created on an ungated model is recorded at its first reopen, which is
        // the point a config edit or a stale model becomes visible.
        return Ok(());
    };
    let mut pending: Vec<Diagnostic> = Vec::new();
    if facts.model.as_deref() != Some(model) {
        pending.push(Diagnostic::ModelChanged {
            from: facts.model.clone(),
            to: model.to_owned(),
        });
    }
    let missing = turn.gate_unavailable_reasons();
    if !missing.is_empty() && facts.last_gate_unavailable.as_deref() != Some(model) {
        pending.push(Diagnostic::GateUnavailable {
            model: model.to_owned(),
            missing,
        });
    }
    diagnostics::append_facts(store, &pending).map_err(|e| ProtocolError::Other {
        message: e.to_string(),
    })
}
