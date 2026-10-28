//! The turn-end action-commit half (spec §4): the `TurnEndAction` a plan calls
//! for and the sync commit that applies a completed action to the record. Split
//! from `turn_end.rs` at the 500-LOC soft limit (#88) — the sequence
//! (`settle_turn`/`plan`) stays there; this is the action execution.

use super::super::{BufferedChunk, Cursor, OmError, OmState, SessionStore, boundary_ts, om};

use crate::provider::{StreamOutcome, TurnResult};

/// The action a turn-end plan calls for. The provider call runs outside the
/// session's write lock (the OM call is an LLM round-trip, not a user-facing
/// stream); only the sync store ops run under it.
#[derive(Debug, PartialEq)]
pub enum TurnEndAction {
    Observe { transcript: String },
    Reflect { level: u8 },
    Buffer { transcript: String },
    Done,
}

impl OmState {
    /// Commit a completed turn-end action (sync — the agent loop runs it
    /// under the session lock after the LLM round-trip). The action
    /// advances: a rejected reflection level escalates, anything applied
    /// becomes `Done`.
    pub fn commit(
        &mut self,
        store: &mut SessionStore,
        action: &mut TurnEndAction,
        result: &TurnResult,
    ) -> Result<(), OmError> {
        let parsed = om::parse_observer_output(&result.text);
        // Capture the suggested-response steering for the observation-producing
        // actions (Observe/Buffer). A Reflect commit reuses save() too, but its
        // suggested-response is the reflector's, not an observation's.
        if let TurnEndAction::Observe { .. } | TurnEndAction::Buffer { .. } = action {
            parsed
                .suggested_response
                .clone_into(&mut self.record.om_suggested_response);
        }
        match action {
            TurnEndAction::Observe { .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_observation(store, &parsed.observations)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Buffer { .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_buffer(store, &parsed.observations)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Reflect { level } => {
                let source = self.record.reflect_source().to_owned();
                // The committed suffix is the parsed <observations> content,
                // never the raw response (mastra `parseReflectorOutput`):
                // the <suggested-response> section is steering, not
                // observation material, and must not pollute the log.
                let parsed = om::parse_reflector_output(&result.text, Some(&source));
                if !parsed.degenerate
                    && !parsed.observations.trim().is_empty()
                    && om::validate_compression(&source, &parsed.observations)
                {
                    let new_suffix =
                        if om::parse_observation_groups(&parsed.observations).is_empty() {
                            om::wrap_in_observation_group(
                                &parsed.observations,
                                &om::combine_group_ranges(&om::parse_observation_groups(&source)),
                                &om::generate_group_id(&parsed.observations),
                                Some("reflection"),
                            )
                        } else {
                            parsed.observations
                        };
                    self.record.active_observations = new_suffix;
                    self.record.generation += 1;
                    self.record.observation_tokens =
                        om::token_count(&self.record.active_observations);
                    self.maintain_prefix_budget();
                    self.changed = true;
                    self.save(store)?;
                    *action = TurnEndAction::Done;
                } else if result.outcome == StreamOutcome::Completed
                    && result.mid_stream_errors.is_empty()
                {
                    // A well-formed completion that fails validation at
                    // this level: escalate (the cap is enforced here).
                    if *level < 4 {
                        *level += 1;
                    } else {
                        *action = TurnEndAction::Done;
                    }
                }
            }
            TurnEndAction::Done => {}
        }
        Ok(())
    }

    /// Apply one observation: wrap the unobserved entries in their
    /// provenance group, append it after the boundary, advance the cursor
    /// past them, persist (spec §4).
    fn apply_observation(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
    ) -> Result<(), OmError> {
        let entries = self.unobserved(store)?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let first = entries.first().unwrap_or(last);
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        self.record.active_observations =
            om::append_observation(&self.record.active_observations, &boundary_ts(), &wrapped);
        self.record.cursor = Some(Cursor {
            entry_id: last.id.clone(),
            timestamp: last.timestamp,
        });
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        self.record.pending_tokens = 0;
        self.changed = true;
        self.save(store)?;
        Ok(())
    }

    /// Apply one buffered observation over the entries NOT already covered
    /// by a previous run (the buffer cursor keeps runs disjoint, mastra's
    /// `lastBufferedAtTokens`): no observation-cursor advance and no save —
    /// the buffer is in memory until activation (the shared tail of the
    /// observe path).
    fn apply_buffer(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
    ) -> Result<(), OmError> {
        let entries = self.unbuffered(store)?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let first = entries.first().unwrap_or(last);
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        let chunk = BufferedChunk {
            range: (first.id.clone(), last.id.clone()),
            last_ts: last.timestamp,
            text: wrapped,
            tokens: om::token_count(observations),
        };
        // The record is the durable chunk store (ticket #86 P2): the sync
        // path writes there too, so the turn-end write-back merge (which
        // re-syncs the in-memory mirror from the record) cannot drop the
        // chunk.
        self.record.buffered_chunks.push(chunk.clone());
        self.buffered.push(chunk);
        self.buffer_cursor = Some(last.id.clone());
        Ok(())
    }
}
