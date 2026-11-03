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
    /// reflector rewrites. The continuation hint LEADS the items: mastra
    /// stamps its `om-continuation` at epoch 0 — sorted before every real
    /// message — and the positional order here is tau's equivalent (#406):
    /// the hint is a preface, never the thing being answered. It is carried
    /// on every assembly once observations exist (#97, #334: mastra's gate —
    /// the stable user item is the request's query anchor). Pure over the
    /// record: no store access, no mutation.
    #[must_use]
    pub fn assemble_context(&self, base: &str) -> AssembledContext {
        let mut instructions = base.to_owned();
        let observations = self.record.agent_observations();
        if !observations.is_empty() {
            instructions.push_str("\n\n");
            instructions.push_str(om::OBSERVATION_CONTEXT_PROMPT);
            instructions.push('\n');
            instructions.push_str(om::OBSERVATION_CONTEXT_INSTRUCTIONS);
        }
        // The continuation hint leads the OM items (#406). Two jobs, one
        // item: the model must not react to the raw history vanishing (#97),
        // and the request must always answer a query — a chat template that
        // finds no user message once the observations (system) and tool
        // calls are pruned rejects the request (#334). Per-assembly is what
        // makes the second job structural; the text is constant, so the vLLM
        // prefix cache survives.
        let mut om_input_items = Vec::new();
        if !observations.is_empty() {
            om_input_items.push(InputEntry::Message(InputMessage {
                role: "user".to_owned(),
                content: format!(
                    "<system-reminder>{}</system-reminder>",
                    om::OBSERVATION_CONTINUATION_HINT
                ),
            }));
        }
        // One `role: system` message per observation group (mastra parity,
        // #96), following the hint: the context instructions label the
        // groups as background, and the live tail comes last, so the hint's
        // "messages following this reminder are newer" framing stays true.
        for group in om::parse_observation_groups(&observations) {
            om_input_items.push(InputEntry::Message(InputMessage {
                role: "system".to_owned(),
                content: group.content,
            }));
        }
        AssembledContext {
            instructions,
            om_input_items,
        }
    }
}
