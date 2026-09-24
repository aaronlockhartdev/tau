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

/// Observer trigger: unobserved tokens reached the (dynamic) threshold,
/// which shrinks as the observation log fills the shared budget
/// (`getStatus` `shouldObserve`; `calculateDynamicThreshold` takes the
/// current observation token count).
pub fn should_observe(pending_tokens: u32, observed_tokens: u32, config: &OmConfig) -> bool {
    pending_tokens >= config.observe_threshold_for(observed_tokens)
}

/// Reflector trigger: observation tokens reached the threshold
/// (`getStatus` `shouldReflect`).
pub fn should_reflect(observation_tokens: u32, config: &OmConfig) -> bool {
    observation_tokens >= config.reflect_threshold
}

/// A reflection pass must shrink the log (mastra `validateCompression`):
/// a non-smaller output is discarded and the ladder escalates.
pub fn validate_compression(source: &str, reflected: &str) -> bool {
    token_count(reflected) < token_count(source)
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
    /// Frozen prefix: the parent's observation log verbatim at a compacted
    /// spawn (ADR-0004) — never re-observed and never re-reflected, and
    /// excluded from the observation-token accounting. Empty in fresh mode.
    pub frozen_prefix: String,
    /// The managed suffix: this session's own observations (date-grouped,
    /// emoji-prioritized). The Reflector rewrites only this portion, keeping
    /// the frozen prefix byte-verbatim (ADR-0004).
    pub active_observations: String,
    /// Newest entry already observed; `None` = nothing observed yet.
    pub cursor: Option<Cursor>,
    /// Reflection generation counter (each reflection is a new generation).
    pub generation: u32,
    pub observation_tokens: u32,
    pub pending_tokens: u32,
    /// The frozen prefix demoted to recall-only (spec §4 overflow ladder:
    /// the suffix was compressed and the combined log still exceeds budget —
    /// the prefix leaves the live context, stays in the session file, and is
    /// re-admitted when space frees).
    #[serde(default)]
    pub prefix_demoted: bool,
}

impl OmRecord {
    /// The full live observation text: the frozen prefix (byte-verbatim) and
    /// the managed suffix — one continuous log (ADR-0004); a demoted prefix
    /// drops out of the live context (it stays in the session file, reachable
    /// via `recall`; spec §4 overflow ladder).
    pub fn live_observations(&self) -> String {
        if self.prefix_demoted {
            return self.active_observations.clone();
        }
        format!("{}{}", self.frozen_prefix, self.active_observations)
    }

    /// What the Reflector sees and rewrites: the managed suffix only.
    pub fn reflect_source(&self) -> &str {
        &self.active_observations
    }
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

/// The cache-stability delimiter the Observer appends observations after
/// (spec §4; mastra `strategy-base.ts` `createMessageBoundary`).
pub fn message_boundary(iso_timestamp: &str) -> String {
    format!("--- message boundary ({iso_timestamp}) ---")
}

fn is_boundary_line(line: &str) -> bool {
    line.starts_with("--- message boundary (") && line.ends_with(" ---")
}

/// Split the observation log into cache-stable chunks at boundary delimiters
/// (mastra `splitObservationContextChunks`: one system message per chunk).
pub fn split_observation_chunks(observations: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in observations.lines() {
        if is_boundary_line(line) {
            if !current.trim().is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks.into_iter().map(|c| c.trim().to_owned()).collect()
}

/// Append a new observation after a fresh boundary (spec §4: appends keep
/// the stable prefix intact for prompt caching).
pub fn append_observation(existing: &str, iso_timestamp: &str, new_observation: &str) -> String {
    let mut out = existing.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&message_boundary(iso_timestamp));
    out.push_str("\n\n");
    out.push_str(new_observation.trim());
    out.push('\n');
    out
}

mod groups;
mod observer;
mod reflector;
pub use groups::*;
pub use observer::*;
pub use reflector::*;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_core;
#[cfg(test)]
mod tests_groups;
#[cfg(test)]
mod tests_observer;
#[cfg(test)]
mod tests_reflector;
