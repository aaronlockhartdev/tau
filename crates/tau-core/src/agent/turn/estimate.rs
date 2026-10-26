use super::assemble::{Assembled, input_items};
use crate::provider::ToolSpec;

/// What the provider charged for one request the turn sent, held beside the
/// parts of the assembly it priced: the estimate for the next call anchors on
/// that count, so only material appended after the anchored request is ever a
/// guess (spec §4, plan §6.4). The guard fields are what makes the anchor
/// trustworthy — the count describes the request only while they are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PromptAnchor {
    /// The entry the anchored request ended at.
    leaf: String,
    /// The instructions it carried: the observation log and the plan state live
    /// here, so a compaction or a re-injection shows up as a change.
    instructions: String,
    /// The tool specs it carried, as serialized.
    tools: String,
    /// How many items the observation state contributed.
    om_items: usize,
    /// The provider's own `usage.input_tokens` — never an estimate (plan §6.4:
    /// the provider's number is the recorded number).
    input_tokens: u64,
}

impl PromptAnchor {
    /// The anchor for the request just sent, charged `input_tokens` by the
    /// provider. None for an assembly with no entries to end at (a session
    /// whose first call has no history yet).
    pub(super) fn after_call(
        assembled: &Assembled,
        tools_json: &str,
        input_tokens: u64,
    ) -> Option<Self> {
        Some(Self {
            leaf: assembled.leaf()?.to_owned(),
            instructions: assembled.instructions.clone(),
            tools: tools_json.to_owned(),
            om_items: assembled.om_items,
            input_tokens,
        })
    }

    /// This assembly priced from the anchor: the provider's count plus the
    /// estimate for the entries appended since. None when this is not that
    /// request with a tail — a compaction, a re-injection, a changed tool
    /// surface, or a branch that no longer carries the entry it ended at.
    fn price(&self, assembled: &Assembled, tools_json: &str) -> Option<u32> {
        if self.instructions != assembled.instructions
            || self.tools != tools_json
            || self.om_items != assembled.om_items
        {
            return None;
        }
        let tail = assembled.tail_after(&self.leaf)?;
        let items = input_items(tail, assembled.parent_session.as_deref());
        let delta = u64::from(crate::provider::items_token_estimate(&items));
        // A prompt beyond u32::MAX is a prompt over any declared window: the
        // gate reads the ceiling as "refuse", which is the honest answer.
        u32::try_from(self.input_tokens.saturating_add(delta)).ok()
    }
}

/// The tool specs as the request serializes them — computed once per call so
/// the anchor's comparison and the estimate read the same string.
pub(super) fn tool_fingerprint(tools: &[ToolSpec]) -> String {
    serde_json::to_string(tools).unwrap_or_default()
}

/// The prompt's size for the gate and the output clamp: anchored when an anchor
/// covers this assembly, otherwise the whole-prompt estimate — a turn's first
/// call, or any call after the prompt changed under the anchor (plan §6.4).
pub(super) fn prompt_size(
    anchor: Option<&PromptAnchor>,
    assembled: &Assembled,
    tools: &[ToolSpec],
    tools_json: &str,
) -> u32 {
    anchor
        .and_then(|a| a.price(assembled, tools_json))
        .unwrap_or_else(|| {
            crate::provider::prompt_token_estimate(&assembled.instructions, &assembled.items, tools)
        })
}
