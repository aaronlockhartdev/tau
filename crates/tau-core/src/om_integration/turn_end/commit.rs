//! The turn-end action-commit half (spec §4): the `TurnEndAction` a plan calls
//! for and the sync commit that applies a completed action to the record. Split
//! from `turn_end.rs` at the 500-LOC soft limit (#88) — the sequence
//! (`settle_turn`/`plan`) stays there; this is the action execution.

use super::super::{BufferedChunk, Cursor, Entry, OmError, OmState, SessionStore, boundary_ts, om};
use crate::provider::{StreamOutcome, TurnResult};

/// The raw span one Observer call covered. The observation group's `range`
/// records it and the observation cursor lands on its end, so the commit needs
/// it verbatim: a bounded span (#394) stops short of the window, and recovering
/// the span by re-reading the window would advance the cursor over entries the
/// Observer was never shown.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedSpan {
    pub first_id: String,
    pub last_id: String,
    pub last_ts: u64,
}

impl ObservedSpan {
    /// The span a slice of raw entries covers, first to last. The slice is
    /// never empty: the observe bound always keeps its first entry, and the
    /// observe gate never fires over an empty window.
    #[must_use]
    pub fn of(entries: &[Entry]) -> Self {
        let first = entries
            .first()
            .expect("the observe bound keeps the first entry of a non-empty window");
        let last = entries
            .last()
            .expect("the observe bound keeps the first entry of a non-empty window");
        Self {
            first_id: first.id.clone(),
            last_id: last.id.clone(),
            last_ts: last.timestamp,
        }
    }
}

/// The action a turn-end plan calls for. The provider call runs outside the
/// session's write lock (the OM call is an LLM round-trip, not a user-facing
/// stream); only the sync store ops run under it.
#[derive(Debug, PartialEq)]
pub enum TurnEndAction {
    Observe {
        transcript: String,
        span: ObservedSpan,
    },
    Reflect {
        level: u8,
    },
    Buffer {
        transcript: String,
        span: ObservedSpan,
        /// The raw tokens the span covers — what the chunk counts, so the
        /// promote target (a raw-window quantity) reads coverage the
        /// observation's compression cannot understate (#399).
        coverage: u32,
    },
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
        match action {
            TurnEndAction::Observe { span, .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_observation(store, &parsed.observations, span)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Buffer { span, coverage, .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_buffer(&parsed.observations, span, *coverage);
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

    /// Apply one observation over `span`: wrap the span in its provenance
    /// group, append it after the boundary, advance the cursor to the span's
    /// end, persist (spec §4). The cursor follows the span, not the window: a
    /// bounded span leaves the rest of the window unobserved for the next
    /// settle (#394).
    fn apply_observation(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
        span: &ObservedSpan,
    ) -> Result<(), OmError> {
        let range = format!("{}:{}", span.first_id, span.last_id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        self.record.active_observations =
            om::append_observation(&self.record.active_observations, &boundary_ts(), &wrapped);
        self.record.cursor = Some(Cursor {
            entry_id: span.last_id.clone(),
            timestamp: span.last_ts,
        });
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        // The quantity the next settle's gate reads is the window the cursor
        // left behind, not 0: a span that stopped short leaves material to
        // observe (#392's gate quantity).
        self.record.pending_tokens = self.unobserved_tokens(store)?;
        self.save(store)?;
        Ok(())
    }

    /// Apply one buffered observation over the plan's bounded slice (the
    /// buffer cursor keeps runs disjoint, mastra's `lastBufferedAtTokens`):
    /// no observation-cursor advance and no save — the buffer is in memory
    /// until activation (the shared tail of the observe path). The chunk
    /// counts the raw tokens the slice covers, not the observation's
    /// compressed tokens: the promote target is a raw-window quantity, and a
    /// coverage understated by compression computes the next target from a
    /// window no gate can see (#399).
    fn apply_buffer(&mut self, observations: &str, span: &ObservedSpan, coverage: u32) {
        let range = format!("{}:{}", span.first_id, span.last_id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        let chunk = BufferedChunk {
            range: (span.first_id.clone(), span.last_id.clone()),
            last_ts: span.last_ts,
            text: wrapped,
            tokens: coverage,
        };
        // The record is the durable chunk store (ticket #86 P2): the sync
        // path writes there too, so the turn-end write-back merge (which
        // re-syncs the in-memory mirror from the record) cannot drop the
        // chunk.
        self.record.buffered_chunks.push(chunk.clone());
        self.buffered.push(chunk);
        self.buffer_cursor = Some(span.last_id.clone());
    }
}
