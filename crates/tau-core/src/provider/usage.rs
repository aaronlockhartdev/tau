//! The providers' spellings of usage, kept out of the record (§6.5). The
//! record type (`TurnUsage`) names the concepts; this module names what each
//! shape calls them, so adding a provider adds a decoder here instead of a
//! field on the durable record.

use serde::Deserialize;
use tau_protocol::payload::TurnUsage;

/// A `usage` object as a provider sends it: the Responses spelling tau asks
/// for, plus the `prompt_tokens` / `completion_tokens` names gateways answer
/// with (spec §6). A stored `total_tokens` is read past — the record derives
/// the sum — as are per-token details nothing reads.
#[derive(Debug, Deserialize)]
pub(crate) struct UsageWire {
    #[serde(default, alias = "prompt_tokens")]
    input_tokens: u64,
    #[serde(default, alias = "completion_tokens")]
    output_tokens: u64,
    #[serde(default, alias = "input_tokens_details")]
    prompt_tokens_details: Option<PromptDetails>,
}

#[derive(Debug, Deserialize)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: u64,
}

impl UsageWire {
    pub(crate) fn into_usage(self) -> TurnUsage {
        TurnUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_input_tokens: self.prompt_tokens_details.map(|d| d.cached_tokens),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_spellings_decode_to_the_same_concepts() {
        let responses: UsageWire =
            serde_json::from_str(r#"{"input_tokens":3,"output_tokens":4}"#).unwrap();
        let chat: UsageWire =
            serde_json::from_str(r#"{"prompt_tokens":3,"completion_tokens":4}"#).unwrap();
        assert_eq!(responses.into_usage(), chat.into_usage());
    }

    #[test]
    fn a_nested_cache_count_becomes_the_concept() {
        for key in ["prompt_tokens_details", "input_tokens_details"] {
            let wire: UsageWire = serde_json::from_str(&format!(
                r#"{{"input_tokens":10,"output_tokens":2,"total_tokens":12,"{key}":{{"cached_tokens":6}}}}"#
            ))
            .unwrap();
            let usage = wire.into_usage();
            assert_eq!(usage.cached_input_tokens, Some(6), "the {key} nesting");
            assert_eq!(usage.total_tokens(), 12, "the sum, not the stored total");
        }
    }

    #[test]
    fn a_silent_provider_leaves_the_cache_unknown() {
        let wire: UsageWire =
            serde_json::from_str(r#"{"input_tokens":1,"output_tokens":1}"#).unwrap();
        assert_eq!(wire.into_usage().cached_input_tokens, None);
    }
}
