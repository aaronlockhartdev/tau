//! Observational Memory (ADR-0004, spec §4): OM is tau's compaction — an
//! Observer LLM converts raw entries into a dense, append-only, date-grouped,
//! emoji-prioritized observation log; a Reflector LLM periodically rewrites
//! the log to keep it bounded; live context is always observations + recent
//! raw. No lossy one-shot summarization.
//!
//! Prompts and threshold math are ported verbatim from Mastra (Apache-2.0;
//! `third_party/mastra-om/` holds the pinned upstream files the fidelity
//! tests diff against). This module is pure: no I/O, no model calls — the
//! integration ticket wires it to the session store and provider.

use serde::{Deserialize, Serialize};
use std::fmt;

/// OM configuration (spec §4: all thresholds configurable; ADR-0004).
/// Defaults match Mastra's `OBSERVATIONAL_MEMORY_DEFAULTS`
/// (`third_party/mastra-om/constants.ts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OmConfig {
    /// Unobserved tokens at which the Observer fires (message threshold).
    pub observe_threshold: u32,
    /// Observation tokens at which the Reflector fires.
    pub reflect_threshold: u32,
    /// Raw tokens retained after activation: a ratio in (0, 1) of the message
    /// threshold, or an absolute count when ≥ 1000
    /// (`third_party/mastra-om/thresholds.ts` `resolveRetentionFloor`).
    pub buffer_activation: f64,
    /// Dynamic threshold as overflow guard (spec §4): when true, the message
    /// threshold expands into unused observation space up to the shared total
    /// budget (`third_party/mastra-om/thresholds.ts` `calculateDynamicThreshold`).
    pub share_token_budget: bool,
}

impl Default for OmConfig {
    fn default() -> Self {
        Self {
            observe_threshold: 30_000,
            reflect_threshold: 40_000,
            buffer_activation: 0.8,
            share_token_budget: false,
        }
    }
}

impl OmConfig {
    /// The effective Observer threshold for the current observation size
    /// (`calculateDynamicThreshold`): with a shared budget, raw history may
    /// grow into unused observation space — `max(total - observed, base)`.
    pub fn observe_threshold_for(&self, observation_tokens: u32) -> u32 {
        if !self.share_token_budget {
            return self.observe_threshold;
        }
        let total = self
            .observe_threshold
            .saturating_add(self.reflect_threshold);
        let effective = total.saturating_sub(observation_tokens);
        effective.max(self.observe_threshold)
    }

    /// The ~6k-token buffering increment at the default thresholds
    /// (`resolveBufferTokens`: ratios of the message threshold).
    pub fn buffer_increment(&self) -> u32 {
        let ratio = self.buffer_activation.clamp(0.0, 1.0);
        (self.observe_threshold as f64 * (1.0 - ratio)).round() as u32
    }

    /// Raw tokens kept after activation (`resolveRetentionFloor`).
    pub fn retention_floor(&self) -> u32 {
        let threshold = self.observe_threshold as u64;
        if self.buffer_activation >= 1000.0 {
            return self.buffer_activation as u32;
        }
        let ratio = self.buffer_activation.clamp(0.0, 1.0);
        (threshold as f64 * (1.0 - ratio)).round() as u32
    }
}

/// Observer trigger: unobserved tokens reached the (dynamic) threshold
/// (`getStatus` `shouldObserve`).
pub fn should_observe(pending_tokens: u32, config: &OmConfig) -> bool {
    pending_tokens >= config.observe_threshold_for(0)
}

/// Reflector trigger: observation tokens reached the threshold
/// (`getStatus` `shouldReflect`).
pub fn should_reflect(observation_tokens: u32, config: &OmConfig) -> bool {
    observation_tokens >= config.reflect_threshold
}

/// A buffered Observer chunk's footprint in the raw window
/// (`BufferedObservationChunk.messageTokens`).
#[derive(Debug, Clone, Copy)]
pub struct ChunkTokens(pub u32);

/// Projected raw tokens removed if activation happened now
/// (`calculateProjectedMessageRemoval`): the chunk boundary closest to
/// `pending - retention_floor`, biased over (remaining context lands at or
/// below the floor), with a 95%-of-floor overshoot guard and a
/// `min(1000, floor)` minimum remainder.
pub fn projected_message_removal(
    chunks: &[ChunkTokens],
    config: &OmConfig,
    current_pending_tokens: u32,
) -> u32 {
    if chunks.is_empty() {
        return 0;
    }
    let retention_floor = config.retention_floor() as u64;
    let target = (current_pending_tokens as u64).saturating_sub(retention_floor);
    if target == 0 {
        return 0;
    }

    let mut cumulative: u64 = 0;
    let mut best_over_boundary: u32 = 0;
    let mut best_over_tokens: u64 = 0;
    let mut best_under_tokens: u64 = 0;
    for (i, chunk) in chunks.iter().enumerate() {
        cumulative += chunk.0 as u64;
        let boundary = (i + 1) as u32;
        if cumulative >= target {
            if best_over_boundary == 0 || cumulative < best_over_tokens {
                best_over_boundary = boundary;
                best_over_tokens = cumulative;
            }
        } else if cumulative > best_under_tokens {
            best_under_tokens = cumulative;
        }
    }

    let max_overshoot = retention_floor * 95 / 100;
    let overshoot = best_over_tokens.saturating_sub(target);
    let remaining_after_over = (current_pending_tokens as u64).saturating_sub(best_over_tokens);
    let remaining_after_under = (current_pending_tokens as u64).saturating_sub(best_under_tokens);
    let min_remaining = 1000u64.min(retention_floor);

    if best_over_boundary > 0 && overshoot <= max_overshoot && remaining_after_over >= min_remaining
    {
        best_over_tokens as u32
    } else if best_under_tokens > 0 && remaining_after_under >= min_remaining {
        best_under_tokens as u32
    } else if best_over_boundary > 0 {
        best_over_tokens as u32
    } else {
        chunks[0].0
    }
}

/// Per-session OM state (ADR-0004: stored with the session as appended
/// entries). The cursor is path-scoped: an entry id plus its timestamp,
/// evaluated on the active branch (Mastra's cursor is a linear timestamp;
/// the entry id is tau's branch disambiguator).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OmRecord {
    /// The current observation log text (date-grouped, emoji-prioritized).
    pub active_observations: String,
    /// Newest entry already observed; `None` = nothing observed yet.
    pub cursor: Option<Cursor>,
    /// Reflection generation counter (each reflection is a new generation).
    pub generation: u32,
    pub observation_tokens: u32,
    pub pending_tokens: u32,
}

/// The observation cursor (ADR-0004 "path-scoped cursor (entry-id +
/// timestamp)").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub entry_id: String,
    pub timestamp: u64,
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.entry_id, self.timestamp)
    }
}

/// Soft token estimate for thresholding (spec §4: "a single estimator is
/// sufficient since thresholds are soft"). ~4 chars/token, no dependencies.
pub fn token_count(text: &str) -> u32 {
    (text.chars().count() / 4) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_mastra() {
        let c = OmConfig::default();
        assert_eq!(c.observe_threshold, 30_000);
        assert_eq!(c.reflect_threshold, 40_000);
        assert!(!c.share_token_budget);
    }

    #[test]
    fn observer_fires_at_threshold_and_only_above() {
        let c = OmConfig::default();
        assert!(!should_observe(29_999, &c));
        assert!(should_observe(30_000, &c));
        assert!(should_observe(31_000, &c));
    }

    #[test]
    fn observer_threshold_is_configurable() {
        let c = OmConfig {
            observe_threshold: 10_000,
            ..Default::default()
        };
        assert!(!should_observe(9_999, &c));
        assert!(should_observe(10_000, &c));
    }

    #[test]
    fn reflector_fires_at_threshold_and_only_above() {
        let c = OmConfig::default();
        assert!(!should_reflect(39_999, &c));
        assert!(should_reflect(40_000, &c));
    }

    #[test]
    fn reflector_threshold_is_configurable() {
        let c = OmConfig {
            reflect_threshold: 5_000,
            ..Default::default()
        };
        assert!(!should_reflect(4_999, &c));
        assert!(should_reflect(5_000, &c));
    }

    #[test]
    fn dynamic_threshold_uses_the_worked_example() {
        // The example in third_party/mastra-om/thresholds.ts (30k:40k, 70k total).
        let c = OmConfig {
            share_token_budget: true,
            ..Default::default()
        };
        assert_eq!(c.observe_threshold_for(0), 70_000);
        assert_eq!(c.observe_threshold_for(10_000), 60_000);
        assert_eq!(c.observe_threshold_for(40_000), 30_000);
        // Never below the base threshold.
        assert_eq!(c.observe_threshold_for(100_000), 30_000);
    }

    #[test]
    fn fixed_threshold_ignores_observation_size() {
        let c = OmConfig::default();
        assert_eq!(c.observe_threshold_for(0), 30_000);
        assert_eq!(c.observe_threshold_for(50_000), 30_000);
    }

    #[test]
    fn buffer_increment_is_20_percent_of_the_message_threshold() {
        assert_eq!(OmConfig::default().buffer_increment(), 6_000);
        let c = OmConfig {
            observe_threshold: 20_000,
            buffer_activation: 0.75,
            ..Default::default()
        };
        assert_eq!(c.buffer_increment(), 5_000);
    }

    #[test]
    fn retention_floor_is_ratio_and_absolute() {
        assert_eq!(OmConfig::default().retention_floor(), 6_000);
        let absolute = OmConfig {
            buffer_activation: 2_000.0,
            ..Default::default()
        };
        assert_eq!(absolute.retention_floor(), 2_000);
    }

    #[test]
    fn projected_removal_prefers_the_nearest_over_boundary() {
        // 6k chunks (the ~6k increment); pending 30k, floor 6k → target 24k.
        let chunks = vec![ChunkTokens(6_000); 5];
        let c = OmConfig::default();
        assert_eq!(projected_message_removal(&chunks, &c, 30_000), 24_000);
    }

    #[test]
    fn projected_removal_falls_back_under_when_overshoot_exceeds_95_percent_of_floor() {
        // Chunks 10k then 20k; pending 30k, floor 6k → target 24k. The over
        // boundary (30k) overshoots by 6k > 95% of the 6k floor (5.7k) → the
        // nearest under boundary (10k) wins.
        let chunks = vec![ChunkTokens(10_000), ChunkTokens(20_000)];
        let c = OmConfig::default();
        assert_eq!(projected_message_removal(&chunks, &c, 30_000), 10_000);
    }

    #[test]
    fn projected_removal_noop_within_the_floor() {
        let chunks = vec![ChunkTokens(6_000); 5];
        let c = OmConfig::default();
        assert_eq!(projected_message_removal(&chunks, &c, 6_000), 0);
        assert_eq!(projected_message_removal(&[], &c, 30_000), 0);
    }

    #[test]
    fn record_defaults_are_empty() {
        let r = OmRecord::default();
        assert!(r.active_observations.is_empty());
        assert!(r.cursor.is_none());
        assert_eq!(r.generation, 0);
    }

    #[test]
    fn token_count_is_a_soft_quarter_estimate() {
        assert_eq!(token_count(""), 0);
        assert_eq!(token_count("abcd"), 1);
        assert_eq!(token_count(&"x".repeat(40_000)), 10_000);
    }
}
