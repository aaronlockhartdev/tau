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

/// Provenance pointer from a compressed observation to the exact raw entries
/// it was derived from — the recall bookkeeping (spec §4; mastra
/// `observation-groups.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationGroup {
    /// 16-hex id.
    pub id: String,
    /// "startEntryId:endEntryId", or comma-joined segments for a merged span.
    pub range: String,
    /// "reflection" for reflected (re-wrapped) sections; absent for observer
    /// appends.
    pub kind: Option<String>,
    pub content: String,
}

/// Deterministic 16-hex group id (mastra uses `randomBytes(8)`; this module
/// is pure, so the id is a content hash — stable across reruns).
pub fn generate_group_id(source: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(source.as_bytes()))
}

/// Wrap an observation in its group tag (mastra `wrapInObservationGroup`).
pub fn wrap_in_observation_group(
    observations: &str,
    range: &str,
    id: &str,
    kind: Option<&str>,
) -> String {
    let kind_attr = kind.map(|k| format!(" kind=\"{k}\"")).unwrap_or_default();
    format!(
        "<observation-group id=\"{id}\" range=\"{range}\"{kind_attr}>\n{}\n</observation-group>",
        observations.trim()
    )
}

/// Extract all observation groups from a log (mastra `parseObservationGroups`).
pub fn parse_observation_groups(observations: &str) -> Vec<ObservationGroup> {
    let mut groups = Vec::new();
    let open_tag = "<observation-group ";
    let mut rest = observations;
    while let Some(start) = rest.find(open_tag) {
        let after_tag = &rest[start + open_tag.len()..];
        let Some(attr_end) = after_tag.find('>') else {
            break;
        };
        let attrs = &after_tag[..attr_end];
        let Some(close) = rest[start..].find("</observation-group>") else {
            break;
        };
        let content = &rest[start + open_tag.len() + attr_end + 1..start + close];
        let mut id: Option<&str> = None;
        let mut range: Option<&str> = None;
        let mut kind: Option<&str> = None;
        for attr in attrs.split_whitespace() {
            let Some((key, value)) = attr.split_once('=') else {
                continue;
            };
            let value = value.trim_matches('"');
            match key {
                "id" => id = Some(value),
                "range" => range = Some(value),
                "kind" => kind = Some(value),
                _ => {}
            }
        }
        if let (Some(id), Some(range)) = (id, range) {
            groups.push(ObservationGroup {
                id: id.to_owned(),
                range: range.to_owned(),
                kind: kind.map(str::to_owned),
                content: content.trim().to_owned(),
            });
        }
        rest = &rest[start + close + "</observation-group>".len()..];
    }
    groups
}

/// Remove the group tags, keeping the content (mastra `stripObservationGroups`).
pub fn strip_observation_groups(observations: &str) -> String {
    let mut out = observations.to_owned();
    let open_tag = "<observation-group ";
    while let Some(start) = out.find(open_tag) {
        let after_tag = &out[start + open_tag.len()..];
        let Some(attr_end) = after_tag.find('>') else {
            break;
        };
        let Some(close) = out[start..].find("</observation-group>") else {
            break;
        };
        let content = out[start + open_tag.len() + attr_end + 1..start + close]
            .trim()
            .to_owned();
        out = format!(
            "{}{}{}",
            &out[..start],
            content,
            &out[start + close + "</observation-group>".len()..]
        );
    }
    out.replace("\n\n\n", "\n\n").trim().to_owned()
}

/// Merge the ranges of several groups into one span (mastra
/// `combineObservationGroupRanges`): the span from the first segment's start
/// to the last segment's end (`first.start:last.end`).
pub fn combine_group_ranges(groups: &[ObservationGroup]) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for group in groups {
        for segment in group.range.split(',') {
            let segment = segment.trim();
            if !segment.is_empty() {
                segments.push(segment);
            }
        }
    }
    match (segments.first(), segments.last()) {
        (Some(first), Some(last)) => format!(
            "{}:{}",
            first.split(':').next().map(str::trim).unwrap_or_default(),
            last.split(':')
                .next_back()
                .map(str::trim)
                .unwrap_or_default()
        ),
        _ => String::new(),
    }
}

/// Render a grouped log for the Reflector: each group becomes a
/// `## Group \`id\`` section with its `_range:` line (mastra
/// `renderObservationGroupsForReflection`).
pub fn render_groups_for_reflection(observations: &str) -> Option<String> {
    let groups = parse_observation_groups(observations);
    if groups.is_empty() {
        return None;
    }
    let mut result = String::new();
    let mut rest = observations;
    let open_tag = "<observation-group ";
    while let Some(start) = rest.find(open_tag) {
        let after_tag = &rest[start + open_tag.len()..];
        let Some(attr_end) = after_tag.find('>') else {
            break;
        };
        let Some(close) = rest[start..].find("</observation-group>") else {
            break;
        };
        let content = rest[start + open_tag.len() + attr_end + 1..start + close].trim();
        let group = groups.iter().find(|g| g.content == content);
        let rendered = match group {
            Some(g) => format!(
                "## Group `{}`\n_range: `{}`_\n\n{}",
                g.id, g.range, g.content
            ),
            None => content.to_owned(),
        };
        result.push_str(&rest[..start]);
        result.push_str(&rendered);
        rest = &rest[start + close + "</observation-group>".len()..];
    }
    result.push_str(rest);
    Some(result.replace("\n\n\n", "\n\n").trim().to_owned())
}

/// Re-derive group provenance from a Reflector's rewritten sections: each
/// `## Group` section keeps the canonical id from its heading, merges the
/// ranges of the source groups whose lines it shares (index fallback), and
/// is marked `kind="reflection"` (mastra `deriveObservationGroupProvenance`).
pub fn derive_group_provenance(
    content: &str,
    groups: &[ObservationGroup],
) -> Vec<ObservationGroup> {
    let sections = reflection_sections(content);
    if sections.is_empty() || groups.is_empty() {
        return Vec::new();
    }
    sections
        .into_iter()
        .enumerate()
        .map(|(index, (heading, body))| {
            let body_lines: std::collections::BTreeSet<&str> = body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            let matching: Vec<ObservationGroup> = groups
                .iter()
                .filter(|g| {
                    g.content
                        .lines()
                        .any(|l| !l.trim().is_empty() && body_lines.contains(l.trim()))
                })
                .cloned()
                .collect();
            let resolved = if !matching.is_empty() {
                matching
            } else {
                vec![groups[index.min(groups.len() - 1)].clone()]
            };
            let id = match heading.split('`').nth(1).map(str::trim) {
                Some(s) if !s.is_empty() => s.to_owned(),
                _ => format!("derived-group-{}", index + 1),
            };
            ObservationGroup {
                id,
                range: combine_group_ranges(&resolved),
                kind: Some("reflection".to_owned()),
                content: body,
            }
        })
        .collect()
}

/// The reflection section shape the Reflector is steered toward: `## Group`
/// headings with an optional `_range: \`...\`_` metadata line (mastra
/// `parseReflectionObservationGroupSections`).
fn reflection_sections(content: &str) -> Vec<(String, String)> {
    let normalized = content.trim();
    if !normalized.lines().any(|l| l.starts_with("## Group ")) {
        return Vec::new();
    }
    let mut sections = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in normalized.lines() {
        if line.starts_with("## Group ") {
            if let Some((h, b)) = current.take() {
                sections.push((h, b.trim().to_owned()));
            }
            current = Some((line.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = current.as_mut() {
            if line.starts_with("_range:") {
                continue; // metadata line, stripped by the upstream parser
            }
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some((h, b)) = current {
        sections.push((h, b.trim().to_owned()));
    }
    sections
}

/// Commit a Reflector's output as the new log: re-wrap its sections as
/// `kind="reflection"` groups over the merged source ranges (mastra
/// `reconcileObservationGroupsFromReflection`).
pub fn reconcile_groups_from_reflection(
    content: &str,
    source_observations: &str,
) -> Option<String> {
    let source_groups = parse_observation_groups(source_observations);
    if source_groups.is_empty() {
        return None;
    }
    let normalized = content.trim();
    if normalized.is_empty() {
        return Some(String::new());
    }
    let derived = derive_group_provenance(normalized, &source_groups);
    if !derived.is_empty() {
        return Some(
            derived
                .iter()
                .map(|g| wrap_in_observation_group(&g.content, &g.range, &g.id, g.kind.as_deref()))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }
    Some(wrap_in_observation_group(
        normalized,
        &combine_group_ranges(&source_groups),
        &generate_group_id(normalized),
        Some("reflection"),
    ))
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

    #[test]
    fn append_observation_adds_a_boundary_delimiter() {
        let first = "Date: Dec 4, 2025\n* 🔴 (14:30) User prefers direct answers";
        let appended = append_observation(
            first,
            "2025-12-05T09:15:00Z",
            "Date: Dec 5, 2025\n* 🔴 (09:15) Continued work on feature X",
        );
        assert!(appended.contains("--- message boundary (2025-12-05T09:15:00Z) ---"));
        assert!(appended.trim_end().ends_with('X'));
        let chunks = split_observation_chunks(&appended);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("Date: Dec 4, 2025"));
        assert!(chunks[1].starts_with("Date: Dec 5, 2025"));
    }

    #[test]
    fn split_observation_chunks_drops_empty_segments() {
        let log = format!(
            "{}\n\n{}",
            "* 🟢 (10:00) info",
            message_boundary("2025-12-05T09:15:00Z")
        );
        assert_eq!(split_observation_chunks(&log).len(), 1);
        assert_eq!(split_observation_chunks("").len(), 0);
    }

    #[test]
    fn group_wrap_parse_round_trip() {
        let wrapped = wrap_in_observation_group(
            "* 🔴 (14:30) User prefers direct answers",
            "00000001:00000012",
            "0123456789abcdef",
            None,
        );
        let groups = parse_observation_groups(&wrapped);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "0123456789abcdef");
        assert_eq!(groups[0].range, "00000001:00000012");
        assert_eq!(groups[0].kind, None);
        assert_eq!(
            groups[0].content,
            "* 🔴 (14:30) User prefers direct answers"
        );
    }

    #[test]
    fn group_kind_is_preserved() {
        let wrapped = wrap_in_observation_group("x", "a:b", "id1234567890abcd", Some("reflection"));
        assert_eq!(
            parse_observation_groups(&wrapped)[0].kind.as_deref(),
            Some("reflection")
        );
    }

    #[test]
    fn multiple_groups_parse_in_order() {
        let log = format!(
            "{}\n\n{}",
            wrap_in_observation_group("one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None),
            wrap_in_observation_group("two", "00000006:00000010", "bbbbbbbbbbbbbbbb", None),
        );
        let groups = parse_observation_groups(&log);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].id, "aaaaaaaaaaaaaaaa");
        assert_eq!(groups[1].id, "bbbbbbbbbbbbbbbb");
        assert_eq!(strip_observation_groups(&log), "one\n\ntwo");
    }

    #[test]
    fn combine_group_ranges_merges_a_span() {
        let groups = vec![
            ObservationGroup {
                id: "a".into(),
                range: "00000001:00000005".into(),
                kind: None,
                content: String::new(),
            },
            ObservationGroup {
                id: "b".into(),
                range: "00000006:00000010".into(),
                kind: None,
                content: String::new(),
            },
        ];
        assert_eq!(combine_group_ranges(&groups), "00000001:00000010");
    }

    #[test]
    fn combine_group_ranges_collapses_to_first_start_last_end() {
        let groups = vec![ObservationGroup {
            id: "a".into(),
            range: "solo1,solo2".into(),
            kind: None,
            content: String::new(),
        }];
        assert_eq!(combine_group_ranges(&groups), "solo1:solo2");
        assert_eq!(combine_group_ranges(&[]), "");
    }

    #[test]
    fn render_groups_for_reflection_emits_group_sections() {
        let log = wrap_in_observation_group(
            "* 🔴 (14:30) User prefers direct answers",
            "00000001:00000012",
            "0123456789abcdef",
            None,
        );
        let rendered = render_groups_for_reflection(&log).unwrap();
        assert_eq!(
            rendered,
            "## Group `0123456789abcdef`\n_range: `00000001:00000012`_\n\n* 🔴 (14:30) User prefers direct answers"
        );
        assert_eq!(
            render_groups_for_reflection("plain log").unwrap_or_default(),
            ""
        );
        assert!(render_groups_for_reflection("").is_none());
    }

    #[test]
    fn derive_provenance_matches_by_shared_lines_and_merges_ranges() {
        let source = format!(
            "{}\n\n{}",
            wrap_in_observation_group(
                "* 🔴 (14:30) User prefers direct answers\n* 🟡 (14:31) Working on feature X",
                "00000001:00000005",
                "aaaaaaaaaaaaaaaa",
                None
            ),
            wrap_in_observation_group(
                "* 🔴 (09:15) Continued work on feature X",
                "00000006:00000010",
                "bbbbbbbbbbbbbbbb",
                None
            ),
        );
        let reflected = "## Group `aaaaaaaaaaaaaaaa`\n* 🔴 (14:30) User prefers direct answers\n* 🟡 (14:31) Working on feature X";
        let groups = parse_observation_groups(&source);
        let derived = derive_group_provenance(reflected, &groups);
        assert_eq!(derived.len(), 1);
        assert_eq!(derived[0].id, "aaaaaaaaaaaaaaaa");
        assert_eq!(derived[0].range, "00000001:00000005");
        assert_eq!(derived[0].kind.as_deref(), Some("reflection"));
    }

    #[test]
    fn derive_provenance_falls_back_to_index_when_lines_do_not_match() {
        let source = wrap_in_observation_group(
            "original line",
            "00000001:00000005",
            "aaaaaaaaaaaaaaaa",
            None,
        );
        let reflected = "## Group `cccccccccccccccc`\nrewritten line";
        let derived = derive_group_provenance(reflected, &parse_observation_groups(&source));
        assert_eq!(derived.len(), 1);
        assert_eq!(derived[0].id, "cccccccccccccccc");
        assert_eq!(derived[0].range, "00000001:00000005");
    }

    #[test]
    fn reconcile_rewraps_reflection_sections_as_groups() {
        let source = format!(
            "{}\n\n{}",
            wrap_in_observation_group("line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None),
            wrap_in_observation_group("line two", "00000006:00000010", "bbbbbbbbbbbbbbbb", None),
        );
        let reflected =
            "## Group `aaaaaaaaaaaaaaaa`\nline one\n\n## Group `bbbbbbbbbbbbbbbb`\nline two";
        let reconciled = reconcile_groups_from_reflection(reflected, &source).unwrap();
        let groups = parse_observation_groups(&reconciled);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].id, "aaaaaaaaaaaaaaaa");
        assert_eq!(groups[0].range, "00000001:00000005");
        assert_eq!(groups[0].kind.as_deref(), Some("reflection"));
        assert_eq!(groups[1].id, "bbbbbbbbbbbbbbbb");
        assert_eq!(groups[1].range, "00000006:00000010");
    }

    #[test]
    fn reconcile_wraps_unsectioned_output_as_one_reflection_group() {
        let source =
            wrap_in_observation_group("line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None);
        let reconciled = reconcile_groups_from_reflection("plain rewritten log", &source).unwrap();
        let groups = parse_observation_groups(&reconciled);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].range, "00000001:00000005");
        assert_eq!(groups[0].kind.as_deref(), Some("reflection"));
    }

    #[test]
    fn reconcile_without_source_groups_is_a_noop_and_empty_content_stays_empty() {
        assert_eq!(reconcile_groups_from_reflection("text", "plain log"), None);
        let source =
            wrap_in_observation_group("line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None);
        assert_eq!(
            reconcile_groups_from_reflection("", &source),
            Some(String::new())
        );
    }

    #[test]
    fn generate_group_id_is_16_hex_and_deterministic() {
        let id = generate_group_id("00000001:00000005|some content");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, generate_group_id("00000001:00000005|some content"));
    }

    #[test]
    fn date_grouped_emoji_log_survives_the_group_round_trip() {
        // The log shape from the spec/ADR (date groups, emoji priorities, ✅
        // completions) must survive strip/wrap/parse without content change.
        let log = "Date: Dec 4, 2025\n* 🔴 (14:30) User prefers direct answers\n* 🟡 (14:32) User might prefer dark mode\n\nDate: Dec 5, 2025\n* 🔴 (09:15) Continued work on feature X\n* ✅ (09:40) Feature X shipped — user confirmed";
        let wrapped = wrap_in_observation_group(log, "00000001:00000020", "0123456789abcdef", None);
        assert_eq!(parse_observation_groups(&wrapped)[0].content, log);
        assert_eq!(strip_observation_groups(&wrapped), log);
    }
}
