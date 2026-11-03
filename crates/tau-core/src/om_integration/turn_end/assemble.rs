//! The main-turn assembly (spec §4): the stable system prompt and the OM-derived
//! input items. Split from `turn_end/mod.rs` at the 500-LOC soft limit (#395) —
//! the settle sequence stays there; this is what a turn sends.

use super::super::{OmState, om};
use crate::provider::{InputEntry, InputMessage};

/// The assembled main-turn context: the stable `instructions` (system prompt)
/// plus the OM-derived `input` items — the continuation hint and, per
/// assembly, the observation groups (#96). `om_input_items` is a `Vec` so the
/// groups append without reshaping this seam.
#[derive(Debug, Clone, Default)]
pub struct AssembledContext {
    pub instructions: String,
    pub om_input_items: Vec<InputEntry>,
}

impl OmState {
    /// The assembled main-turn context (spec §4). `instructions` is the stable
    /// system prompt: the base prompt and the observation framing. The
    /// volatile observation content is NOT in `instructions` — each
    /// observation group rides as its own `role: system` item in
    /// `om_input_items` (#96), so the prompt stays byte-stable across appends and
    /// reflector rewrites. The continuation hint is the trailing `role: user`
    /// item, carried on every assembly once observations exist (#97, #334:
    /// mastra's gate — the stable user item is the request's query anchor).
    /// Pure over the record (no store access), so the `om_suggested_response`
    /// consumption sticks when the loop runs it on the persistent state under
    /// the lock.
    pub fn assemble_context(&mut self, base: &str) -> AssembledContext {
        let mut instructions = base.to_owned();
        let observations = self.record.agent_observations();
        if !observations.is_empty() {
            instructions.push_str("\n\n");
            instructions.push_str(om::OBSERVATION_CONTEXT_PROMPT);
            instructions.push('\n');
            instructions.push_str(om::OBSERVATION_CONTEXT_INSTRUCTIONS);
        }
        // One `role: system` message per observation group (mastra parity,
        // #96), pushed before the hint so its "messages following are newer"
        // framing stays true.
        let mut om_input_items = Vec::new();
        for group in om::parse_observation_groups(&observations) {
            om_input_items.push(InputEntry::Message(InputMessage {
                role: "system".to_owned(),
                content: group.content,
            }));
        }
        // The continuation hint rides as a `role: user` item on every assembly
        // that has observations — mastra's form (their `om-continuation`
        // message, gated on `activeObservations`). Two jobs, one item: the model
        // must not react to the raw history vanishing (#97), and the request
        // must always answer a query — a chat template that finds no user
        // message once the observations (system) and tool calls are pruned
        // rejects the request (#334). Per-assembly is what makes the second job
        // structural; the text is constant, so the vLLM prefix cache survives,
        // and only the suggested response varies — consumed once below.
        if !observations.is_empty() {
            // One-shot steering: hand the observer's suggested-response to the
            // model alongside the hint (which says to follow it), then clear it
            // so a later reflect/promote can't re-inject a stale signal.
            let suggested = self.record.om_suggested_response.clone();
            self.record.om_suggested_response = String::new();
            let mut content = String::from("<system-reminder>");
            content.push_str(om::OBSERVATION_CONTINUATION_HINT);
            if !suggested.trim().is_empty() {
                content.push_str("\n\nSuggested response: ");
                content.push_str(&suggested);
            }
            content.push_str("</system-reminder>");
            om_input_items.push(InputEntry::Message(InputMessage {
                role: "user".to_owned(),
                content,
            }));
        }
        AssembledContext {
            instructions,
            om_input_items,
        }
    }
}
