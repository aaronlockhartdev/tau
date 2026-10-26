//! The diagnostic projection (ADR-0034): session state derived by folding
//! the record, never by re-reading prose the harness itself wrote.
//!
//! The entry log is the store and this is its read model, so the fold is
//! pure and branch scoping belongs to the caller
//! ([`crate::om_integration::branch_entries`]). Diagnostics are append-only:
//! there are no upserts here, and a new code costs no migration.

use crate::session::Entry;
use serde_json::Value;
use tau_protocol::{Diagnostic, EntryKind};

/// What the record says about a session's gates and effective model.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    /// A completion-gate failure exhausted its repairs on this branch. The
    /// record is sticky — a later gate pass leaves no record to replace it, so
    /// the flag stands across restarts; the in-memory `LiveState` flag can still
    /// clear for the run (ADR-0034 append-only).
    pub gate_failed: bool,
    /// The model the record last named, when it names one. A session whose
    /// model was never changed has no such record — its model lives in the
    /// workspace config.
    pub model: Option<String>,
    /// The model the record last said had no computable context gate. The
    /// bracket is per model, so re-opening a session never re-records one it
    /// already named (ADR-0034: transitions, not per-request spam).
    pub gate_unavailable_for: Option<String>,
}

/// Fold a branch's entries into session state (ADR-0034's one reader, in
/// place of the three prose scans this replaces). `branch` is the caller's
/// active-branch slice; the fold itself is order-only.
#[must_use]
pub fn project(branch: &[Entry]) -> Diagnostics {
    let mut out = Diagnostics::default();
    for entry in branch {
        match entry.kind {
            EntryKind::Diagnostic => {
                if let Some(fact) = Diagnostic::from_value(&entry.payload) {
                    out.apply(fact);
                }
            }
            EntryKind::System => {
                if let Some(note) = entry.payload.get("note").and_then(Value::as_str) {
                    out.apply_legacy(note);
                }
            }
            EntryKind::User
            | EntryKind::Assistant
            | EntryKind::Tool
            | EntryKind::Om
            | EntryKind::Subagent
            | EntryKind::SpawnSnapshot
            | EntryKind::Other(_) => {}
        }
    }
    out
}

/// Fold a store's active branch into session state — the one read every
/// consumer shares, so none walks the log itself. `None` when the session has
/// no entries yet.
pub(crate) fn project_store(
    store: &mut crate::session::SessionStore,
) -> Result<Option<Diagnostics>, crate::session::Error> {
    let entries: Vec<crate::session::Entry> = store
        .entries_range_cached(0, usize::MAX)?
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    let leaf = store.leaf_cached()?.map(|e| e.id);
    if entries.is_empty() {
        return Ok(None);
    }
    let branch = crate::om_integration::branch_entries(&entries, leaf.as_deref());
    Ok(Some(project(&branch)))
}

impl Diagnostics {
    fn apply(&mut self, fact: Diagnostic) {
        match fact {
            Diagnostic::ModelChanged { to, .. } => self.set_model(to),
            Diagnostic::GateUnavailable { model, .. } => self.gate_unavailable_for = Some(model),
            Diagnostic::GateFailed { .. } => self.gate_failed = true,
            // The rest are facts about a moment, not state to carry.
            Diagnostic::GateAttempt { .. }
            | Diagnostic::ContextOverflow { .. }
            | Diagnostic::OmStalled { .. }
            | Diagnostic::ProviderRejected { .. }
            | Diagnostic::TurnCutoff { .. }
            | Diagnostic::Unknown { .. } => {}
        }
    }

    /// The frozen reader for `system` notes a pre-#306 build wrote
    /// (ADR-0034). Exactly three formats were ever written — a completion-gate
    /// attempt, the terminal gate failure, and a model change — and only the
    /// last two carry state. This list never grows: nothing in this build can
    /// write a `system` note.
    fn apply_legacy(&mut self, note: &str) {
        if let Some(model) = note.strip_prefix("model: ") {
            // The old note spelled a change as `model: old → new`; the name
            // is whatever follows the last arrow.
            self.set_model(model.rsplit(" → ").next().unwrap_or(model).to_owned());
        } else if note.contains("marked gate_failed") {
            self.gate_failed = true;
        }
    }

    /// A blank name is no name. The hand-written `Diagnostic` deserializer
    /// defaults a missing field to `""`, so a truncated `model_changed` must
    /// not answer the session's model question with nothing.
    fn set_model(&mut self, to: String) {
        if !to.is_empty() {
            self.model = Some(to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(kind: EntryKind, payload: Value) -> Entry {
        Entry {
            id: format!("e{}", payload.to_string().len()),
            parent: None,
            timestamp: 1,
            kind,
            payload,
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        }
    }

    fn diagnostic(fact: &Diagnostic) -> Entry {
        entry(EntryKind::Diagnostic, fact.to_value())
    }

    #[test]
    fn a_gate_failure_is_session_state() {
        let branch = vec![entry(
            EntryKind::System,
            json!({ "note": "completion gate: check `true` failed (attempt 1): boom" }),
        )];
        assert!(
            !project(&branch).gate_failed,
            "an attempt alone is not terminal"
        );
        let branch = vec![diagnostic(&Diagnostic::GateFailed {
            command: "cargo test".into(),
            retries: 2,
        })];
        assert!(project(&branch).gate_failed);
    }

    #[test]
    fn the_last_model_change_wins_and_the_predecessor_is_ignored() {
        let branch = vec![
            diagnostic(&Diagnostic::ModelChanged {
                from: None,
                to: "a".into(),
            }),
            diagnostic(&Diagnostic::ModelChanged {
                from: Some("a".into()),
                to: "b".into(),
            }),
        ];
        assert_eq!(project(&branch).model.as_deref(), Some("b"));
    }

    #[test]
    fn a_truncated_model_record_answers_nothing() {
        // The hand-written deserializer defaults a missing field to `""`; a
        // record that names no model must not become the session's model.
        let branch = vec![
            diagnostic(&Diagnostic::ModelChanged {
                from: None,
                to: "real".into(),
            }),
            entry(EntryKind::Diagnostic, json!({ "code": "model_changed" })),
        ];
        assert_eq!(project(&branch).model.as_deref(), Some("real"));
    }

    #[test]
    fn a_blank_note_names_no_model() {
        let branch = vec![entry(EntryKind::System, json!({ "note": "model: " }))];
        assert_eq!(project(&branch).model, None);
    }

    // The negative: codes that are not state must not move it.
    #[test]
    fn other_codes_leave_the_state_alone() {
        let branch = vec![
            diagnostic(&Diagnostic::TurnCutoff {
                model: "m".into(),
                ended_by: tau_protocol::StreamEnd::EofWithoutDone,
                blocks: 1,
            }),
            diagnostic(&Diagnostic::OmStalled {
                pending: 1,
                activation: 1,
            }),
            entry(EntryKind::Diagnostic, json!({ "note": "not a diagnostic" })),
            entry(EntryKind::User, json!({ "text": "hi" })),
        ];
        assert_eq!(project(&branch), Diagnostics::default());
    }

    #[test]
    fn an_unknown_code_is_read_without_failing_the_projection() {
        let branch = vec![entry(
            EntryKind::Diagnostic,
            json!({ "code": "future_thing", "detail": "kept" }),
        )];
        assert_eq!(project(&branch), Diagnostics::default());
    }

    // The legacy shim, tested against the exact strings the old writers
    // produced (ADR-0034: closed list, closed tests).
    #[test]
    fn a_legacy_gate_note_still_marks_the_session() {
        let branch = vec![entry(
            EntryKind::System,
            json!({ "note": "completion gate: check `cargo test` failed after 2 repair attempts — the session is marked gate_failed. Last output: boom" }),
        )];
        assert!(project(&branch).gate_failed);
    }

    #[test]
    fn a_legacy_model_note_still_names_the_model() {
        let branch = vec![
            entry(EntryKind::System, json!({ "note": "model: a" })),
            entry(EntryKind::System, json!({ "note": "model: a → b" })),
        ];
        assert_eq!(project(&branch).model.as_deref(), Some("b"));
    }

    #[test]
    fn a_legacy_note_that_is_neither_carries_no_state() {
        let branch = vec![entry(
            EntryKind::System,
            json!({ "note": "provider rejection" }),
        )];
        assert_eq!(project(&branch), Diagnostics::default());
    }
}
