//! The reasoning-effort → output-cap adjustment (spec §12, #36). A thinking
//! turn reserves output room for its thinking: the cap rises to base + the
//! level's token budget, capped at the model's max. The wire carries only
//! `reasoning.effort` — the Responses API has no `budget_tokens` field, so
//! raising the cap is the consumer (pi's `adjustMaxTokensForThinking`, the
//! provider-agnostic part).
use std::collections::BTreeMap;

use crate::config::ThinkingLevel;

/// A `None` base with a declared model max uses the model max; `off` leaves
/// the base untouched.
pub(super) fn thinking_adjusted_max_output(
    base: Option<u32>,
    model_max: Option<u32>,
    level: ThinkingLevel,
    budgets: &BTreeMap<String, u32>,
) -> Option<u64> {
    if level == ThinkingLevel::Off {
        return base.map(u64::from);
    }
    let budget = thinking_budget(level, budgets) as u64;
    match (base, model_max) {
        (Some(b), Some(m)) => Some((b as u64 + budget).min(m as u64)),
        (Some(b), None) => Some(b as u64 + budget),
        (None, Some(m)) => Some(m as u64),
        (None, None) => None,
    }
}

/// The token budget for a thinking level: the user's `thinking.budgets` entry
/// wins, else the pi defaults; xhigh and max fold to high.
pub(super) fn thinking_budget(level: ThinkingLevel, budgets: &BTreeMap<String, u32>) -> u32 {
    let key: &'static str = match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High | ThinkingLevel::XHigh | ThinkingLevel::Max => "high",
        ThinkingLevel::Off => "off",
    };
    budgets.get(key).copied().unwrap_or(match key {
        "minimal" => 1024,
        "low" => 2048,
        "medium" => 8192,
        "high" => 16384,
        _ => 0,
    })
}
