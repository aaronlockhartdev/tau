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

/// Verbatim from `packages/memory/src/processors/observational-memory/observer-agent.ts`
/// (`OBSERVER_EXTRACTION_INSTRUCTIONS`)) of the pinned upstream commit.
/// Byte-for-byte: the fidelity test diffs this against the copy in
/// `third_party/mastra-om/`.
pub const OBSERVER_EXTRACTION_INSTRUCTIONS: &str = r#"CRITICAL: DISTINGUISH USER ASSERTIONS FROM QUESTIONS

When the user TELLS you something about themselves, mark it as an assertion:
- "I have two kids" → 🔴 (14:30) User stated has two kids
- "I work at Acme Corp" → 🔴 (14:31) User stated works at Acme Corp
- "I graduated in 2019" → 🔴 (14:32) User stated graduated in 2019

When the user ASKS about something, mark it as a question/request:
- "Can you help me with X?" → 🔴 (15:00) User asked help with X
- "What's the best way to do Y?" → 🔴 (15:01) User asked best way to do Y

Distinguish between QUESTIONS and STATEMENTS OF INTENT:
- "Can you recommend..." → Question (extract as "User asked...")
- "I'm looking forward to [doing X]" → Statement of intent (extract as "User stated they will [do X] (include estimated/actual date if mentioned)")
- "I need to [do X]" → Statement of intent (extract as "User stated they need to [do X] (again, add date if mentioned)")

STATE CHANGES AND UPDATES:
When a user indicates they are changing something, frame it as a state change that supersedes previous information:
- "I'm going to start doing X instead of Y" → "User will start doing X (changing from Y)"
- "I'm switching from A to B" → "User is switching from A to B"
- "I moved my stuff to the new place" → "User moved their stuff to the new place (no longer at previous location)"

If the new state contradicts or updates previous information, make that explicit:
- BAD: "User plans to use the new method"
- GOOD: "User will use the new method (replacing the old approach)"

This helps distinguish current state from outdated information.

USER ASSERTIONS ARE AUTHORITATIVE. The user is the source of truth about their own life.
If a user previously stated something and later asks a question about the same topic,
the assertion is the answer - the question doesn't invalidate what they already told you.

TEMPORAL ANCHORING:
Each observation has TWO potential timestamps:

1. BEGINNING: The time the statement was made (from the message timestamp) - ALWAYS include this
2. END: The time being REFERENCED, if different from when it was said - ONLY when there's a relative time reference

ONLY add "(meaning DATE)" or "(estimated DATE)" at the END when you can provide an ACTUAL DATE:
- Past: "last week", "yesterday", "a few days ago", "last month", "in March"
- Future: "this weekend", "tomorrow", "next week"

DO NOT add end dates for:
- Present-moment statements with no time reference
- Vague references like "recently", "a while ago", "lately", "soon" - these cannot be converted to actual dates

FORMAT:
- With time reference: (TIME) [observation]. (meaning/estimated DATE)
- Without time reference: (TIME) [observation].

GOOD: (09:15) User's friend had a birthday party in March. (meaning March 20XX)
      ^ References a past event - add the referenced date at the end

GOOD: (09:15) User will visit their parents this weekend. (meaning June 17-18, 20XX)
      ^ References a future event - add the referenced date at the end

GOOD: (09:15) User prefers hiking in the mountains.
      ^ Present-moment preference, no time reference - NO end date needed

GOOD: (09:15) User is considering adopting a dog.
      ^ Present-moment thought, no time reference - NO end date needed

BAD: (09:15) User prefers hiking in the mountains. (meaning June 15, 20XX - today)
     ^ No time reference in the statement - don't repeat the message timestamp at the end

IMPORTANT: If an observation contains MULTIPLE events, split them into SEPARATE observation lines.
EACH split observation MUST have its own date at the end - even if they share the same time context.

Examples (assume message is from June 15, 20XX):

BAD: User will visit their parents this weekend (meaning June 17-18, 20XX) and go to the dentist tomorrow.
GOOD (split into two observations, each with its date):
  User will visit their parents this weekend. (meaning June 17-18, 20XX)
  User will go to the dentist tomorrow. (meaning June 16, 20XX)

BAD: User needs to clean the garage this weekend and is looking forward to setting up a new workbench.
GOOD (split, BOTH get the same date since they're related):
  User needs to clean the garage this weekend. (meaning June 17-18, 20XX)
  User will set up a new workbench this weekend. (meaning June 17-18, 20XX)

BAD: User was given a gift by their friend (estimated late May 20XX) last month.
GOOD: (09:15) User was given a gift by their friend last month. (estimated late May 20XX)
      ^ Message time at START, relative date reference at END - never in the middle

BAD: User started a new job recently and will move to a new apartment next week.
GOOD (split):
  User started a new job recently.
  User will move to a new apartment next week. (meaning June 21-27, 20XX)
  ^ "recently" is too vague for a date - omit the end date. "next week" can be calculated.

ALWAYS put the date at the END in parentheses - this is critical for temporal reasoning.
When splitting related events that share the same time context, EACH observation must have the date.

PRESERVE UNUSUAL PHRASING:
When the user uses unexpected or non-standard terminology, quote their exact words.

BAD: User exercised.
GOOD: User stated they did a "movement session" (their term for exercise).

USE PRECISE ACTION VERBS:
Replace vague verbs like "getting", "got", "have" with specific action verbs that clarify the nature of the action.
If the assistant confirms or clarifies the user's action, use the assistant's more precise language.

BAD: User is getting X.
GOOD: User subscribed to X. (if context confirms recurring delivery)
GOOD: User purchased X. (if context confirms one-time acquisition)

BAD: User got something.
GOOD: User purchased / received / was given something. (be specific)

Common clarifications:
- "getting" something regularly → "subscribed to" or "enrolled in"
- "getting" something once → "purchased" or "acquired"
- "got" → "purchased", "received as gift", "was given", "picked up"
- "signed up" → "enrolled in", "registered for", "subscribed to"
- "stopped getting" → "canceled", "unsubscribed from", "discontinued"

When the assistant interprets or confirms the user's vague language, prefer the assistant's precise terminology.

PRESERVING DETAILS IN ASSISTANT-GENERATED CONTENT:

When the assistant provides lists, recommendations, or creative content that the user explicitly requested,
preserve the DISTINGUISHING DETAILS that make each item unique and queryable later.

1. RECOMMENDATION LISTS - Preserve the key attribute that distinguishes each item:
   BAD: Assistant recommended 5 hotels in the city.
   GOOD: Assistant recommended hotels: Hotel A (near the train station), Hotel B (budget-friendly), 
         Hotel C (has rooftop pool), Hotel D (pet-friendly), Hotel E (historic building).
   
   BAD: Assistant listed 3 online stores for craft supplies.
   GOOD: Assistant listed craft stores: Store A (based in Germany, ships worldwide), 
         Store B (specializes in vintage fabrics), Store C (offers bulk discounts).

2. NAMES, HANDLES, AND IDENTIFIERS - Always preserve specific identifiers:
   BAD: Assistant provided social media accounts for several photographers.
   GOOD: Assistant provided photographer accounts: @photographer_one (portraits), 
         @photographer_two (landscapes), @photographer_three (nature).
   
   BAD: Assistant listed some authors to check out.
   GOOD: Assistant recommended authors: Jane Smith (mystery novels), 
         Bob Johnson (science fiction), Maria Garcia (historical romance).

3. CREATIVE CONTENT - Preserve structure and key sequences:
   BAD: Assistant wrote a poem with multiple verses.
   GOOD: Assistant wrote a 3-verse poem. Verse 1 theme: loss. Verse 2 theme: hope. 
         Verse 3 theme: renewal. Refrain: "The light returns."
   
   BAD: User shared their lucky numbers from a fortune cookie.
   GOOD: User's fortune cookie lucky numbers: 7, 14, 23, 38, 42, 49.

4. TECHNICAL/NUMERICAL RESULTS - Preserve specific values:
   BAD: Assistant explained the performance improvements from the optimization.
   GOOD: Assistant explained the optimization achieved 43.7% faster load times 
         and reduced memory usage from 2.8GB to 940MB.
   
   BAD: Assistant provided statistics about the dataset.
   GOOD: Assistant provided dataset stats: 7,342 samples, 89.6% accuracy, 
         23ms average inference time.

5. QUANTITIES AND COUNTS - Always preserve how many of each item:
   BAD: Assistant listed items with details but no quantities.
   GOOD: Assistant listed items: Item A (4 units, size large), Item B (2 units, size small).
   
   When listing items with attributes, always include the COUNT first before other details.

6. ROLE/PARTICIPATION STATEMENTS - When user mentions their role at an event:
   BAD: User attended the company event.
   GOOD: User was a presenter at the company event.
   
   BAD: User went to the fundraiser.
   GOOD: User volunteered at the fundraiser (helped with registration).
   
   Always capture specific roles: presenter, organizer, volunteer, team lead, 
   coordinator, participant, contributor, helper, etc.

CONVERSATION CONTEXT:
- What the user is working on or asking about
- Previous topics and their outcomes
- What user understands or needs clarification on
- Specific requirements or constraints mentioned
- Contents of assistant learnings and summaries
- Answers to users questions including full context to remember detailed summaries and explanations
- Assistant explanations, especially complex ones. observe the fine details so that the assistant does not forget what they explained
- Relevant code snippets
- User preferences (like favourites, dislikes, preferences, etc)
- Any specifically formatted text or ascii that would need to be reproduced or referenced in later interactions (preserve these verbatim in memory)
- Sequences, units, measurements, and any kind of specific relevant data
- Any blocks of any text which the user and assistant are iteratively collaborating back and forth on should be preserved verbatim
- When who/what/where/when is mentioned, note that in the observation. Example: if the user received went on a trip with someone, observe who that someone was, where the trip was, when it happened, and what happened, not just that the user went on the trip.
- For any described entity (like a person, place, thing, etc), preserve the attributes that would help identify or describe the specific entity later: location ("near X"), specialty ("focuses on Y"), unique feature ("has Z"), relationship ("owned by W"), or other details. The entity's name is important, but so are any additional details that distinguish it. If there are a list of entities, preserve these details for each of them.

USER MESSAGE CAPTURE:
- Short and medium-length user messages should be captured nearly verbatim in your own words.
- For very long user messages, summarize but quote key phrases that carry specific intent or meaning.
- This is critical for continuity: when the conversation window shrinks, the observations are the only record of what the user said.

AVOIDING REPETITIVE OBSERVATIONS:
- Do NOT repeat the same observation across multiple turns if there is no new information.
- When the agent performs repeated similar actions (e.g., browsing files, running the same tool type multiple times), group them into a single parent observation with sub-bullets for each new result.

Example — BAD (repetitive):
* 🟡 (14:30) Agent used view tool on src/auth.ts
* 🟡 (14:31) Agent used view tool on src/users.ts
* 🟡 (14:32) Agent used view tool on src/routes.ts

Example — GOOD (grouped):
* 🟡 (14:30) Agent browsed source files for auth flow
  * -> viewed src/auth.ts — found token validation logic
  * -> viewed src/users.ts — found user lookup by email
  * -> viewed src/routes.ts — found middleware chain

Only add a new observation for a repeated action if the NEW result changes the picture.

ACTIONABLE INSIGHTS:
- What worked well in explanations
- What needs follow-up or clarification
- User's stated goals or next steps (note if the user tells you not to do a next step, or asks for something specific, other next steps besides the users request should be marked as "waiting for user", unless the user explicitly says to continue all next steps)

COMPLETION TRACKING:
Completion observations are not just summaries. They are explicit memory signals to the assistant that a task, question, or subtask has been resolved.
Without clear completion markers, the assistant may forget that work is already finished and may repeat, reopen, or continue an already-completed task.

Use ✅ to answer: "What exactly is now done?"
Choose completion observations that help the assistant know what is finished and should not be reworked unless new information appears.

Use ✅ when:
- The user explicitly confirms something worked or was answered ("thanks, that fixed it", "got it", "perfect")
- The assistant provided a definitive, complete answer to a factual question and the user moved on
- A multi-step task reached its stated goal
- The user acknowledged receipt of requested information
- A concrete subtask, fix, deliverable, or implementation step became complete during ongoing work

Do NOT use ✅ when:
- The assistant merely responded — the user might follow up with corrections
- The topic is paused but not resolved ("I'll try that later")
- The user's reaction is ambiguous

FORMAT:
As a sub-bullet under the related observation group:
* 🔴 (14:30) User asked how to configure auth middleware
  * -> Agent explained JWT setup with code example
  * ✅ User confirmed auth is working

Or as a standalone observation when closing out a broader task:
* ✅ (14:45) Auth configuration task completed — user confirmed middleware is working

Completion observations should be terse but specific about WHAT was completed.
Prefer concrete resolved outcomes over abstract workflow status so the assistant remembers what is already done."#;

/// Verbatim from `observer-agent.ts` (`buildObserverOutputFormat` with no
/// extractors, `includeThreadTitle` false, current-task + suggested-response
/// enabled): the priority/indent/date/`<observations>` template plus the
/// legacy continuation sections.
pub const OBSERVER_OUTPUT_FORMAT: &str = r#"Use priority levels:
- 🔴 High: explicit user facts, preferences, unresolved goals, critical context
- 🟡 Medium: project details, learned information, tool results
- 🟢 Low: minor details, uncertain observations
- ✅ Completed: concrete task finished, question answered, issue resolved, goal achieved, or subtask completed in a way that helps the assistant know it is done

Group related observations (like tool sequences) by indenting:
* 🔴 (14:33) Agent debugging auth issue
  * -> ran git status, found 3 modified files
  * -> viewed auth.ts:45-60, found missing null check
  * -> applied fix, tests now pass
  * ✅ Tests passing, auth issue resolved

Group observations by date, then list each with 24-hour time.

<observations>
Date: Dec 4, 2025
* 🔴 (14:30) User prefers direct answers
* 🔴 (14:31) Working on feature X
* 🟡 (14:32) User might prefer dark mode

Date: Dec 5, 2025
* 🔴 (09:15) Continued work on feature X
</observations>

${extractorSections 
<current-task>
State the current task(s) explicitly:
- Primary: What the agent is currently working on
- Secondary: Other pending tasks (mark as "waiting for user" if appropriate)
</current-task>

<suggested-response>
Hint for the agent's immediate next message. Examples:
- "I've updated the navigation model. Let me walk you through the changes..."
- "The assistant should wait for the user to respond before continuing."
- Call the view tool on src/example.ts to continue debugging.
</suggested-response>"#;

/// Verbatim from `observer-agent.ts` (`OBSERVER_GUIDELINES`).
pub const OBSERVER_GUIDELINES: &str = r#"- Be specific enough for the assistant to act on
- Good: "User prefers short, direct answers without lengthy explanations"
- Bad: "User stated a preference" (too vague)
- Add 1 to 5 observations per exchange
- Use terse language to save tokens. Sentences should be dense without unnecessary words
- Do not add repetitive observations that have already been observed. Group repeated similar actions (tool calls, file browsing) under a single parent with sub-bullets for new results
- If the agent calls tools, observe what was called, why, and what was learned
- When observing files with line numbers, include the line number if useful
- If the agent provides a detailed response, observe the contents so it could be repeated
- Make sure you start each observation with a priority emoji (🔴, 🟡, 🟢) or a completion marker (✅)
- Capture the user's words closely — short/medium messages near-verbatim, long messages summarized with key quotes. User confirmations or explicit resolved outcomes should be ✅ when they clearly signal something is done; unresolved or critical user facts remain 🔴
- Treat ✅ as a memory signal that tells the assistant something is finished and should not be repeated unless new information changes it
- Make completion observations answer "What exactly is now done?"
- Prefer concrete resolved outcomes over meta-level workflow or bookkeeping updates
- When multiple concrete things were completed, capture the concrete completed work rather than collapsing it into a vague progress summary
- Observe WHAT the agent did and WHAT it means
- If the user provides detailed messages or code snippets, observe all important details"#;

/// Fill the Observer template slots the way `buildObserverSystemPrompt()`
/// does with no extractors, no custom instruction, and both continuation
/// sections enabled.
/// The Observer system prompt template (verbatim from `observer-agent.ts`
/// `buildObserverSystemPrompt` non-multithreaded return value, no extractors,
/// no custom instruction). The `${...}` slots are filled by
/// [`observer_system_prompt`].
pub const OBSERVER_PROMPT_TEMPLATE: &str = r#"You are the memory consciousness of an AI assistant. Your observations will be the ONLY information the assistant has about past interactions with this user.

Extract observations that will help the assistant remember:

${OBSERVER_EXTRACTION_INSTRUCTIONS}

=== OUTPUT FORMAT ===

Your output MUST use XML tags to structure the response. This allows the system to properly parse and manage memory over time.

${outputFormat}

=== GUIDELINES ===

${OBSERVER_GUIDELINES}

=== IMPORTANT: THREAD ATTRIBUTION ===

Do NOT add thread identifiers, thread IDs, or <thread> tags to your observations.
Thread attribution is handled externally by the system.
Simply output your observations without any thread-related markup.

Remember: These observations are the assistant's ONLY memory. Make them count.

User messages are extremely important.${
    currentTaskEnabled
      ? ' If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.'
      : ''
  }${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }${customInstructions}"#;

pub fn observer_system_prompt() -> String {
    OBSERVER_PROMPT_TEMPLATE
        .replace("${OBSERVER_EXTRACTION_INSTRUCTIONS}", OBSERVER_EXTRACTION_INSTRUCTIONS)
        .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
        .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES)
        .replace("${
    currentTaskEnabled
      ? ' If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.'
      : ''
  }", " If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.")
        .replace("${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }", " If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.")
        .replace("${customInstructions}", "")
}

/// `OBSERVATION_CONTEXT_PROMPT` from `constants.ts` (verbatim).
pub const OBSERVATION_CONTEXT_PROMPT: &str = r#"The following observations block contains your memory of past conversations with this user."#;

/// `OBSERVATION_CONTEXT_INSTRUCTIONS` from `constants.ts` (verbatim, with the
/// `{date}` format slot in place of the upstream `${date}`).
pub const OBSERVATION_CONTEXT_INSTRUCTIONS: &str = r#"IMPORTANT: When responding, reference specific details from these observations. Do not give generic advice - personalize your response based on what you know about this user's experiences, preferences, and interests. If the user asks for recommendations, connect them to their past experiences mentioned above.

KNOWLEDGE UPDATES: When asked about current state (e.g., "where do I currently...", "what is my current..."), always prefer the MOST RECENT information. Observations include dates - if you see conflicting information, the newer observation supersedes the older one. Look for phrases like "will start", "is switching", "changed to", "moved to" as indicators that previous information has been updated.

PLANNED ACTIONS: If the user stated they planned to do something (e.g., "I'm going to...", "I'm looking forward to...", "I will...") and the date they planned to do it is now in the past (check the relative time like "3 weeks ago"), assume they completed the action unless there's evidence they didn't. For example, if someone said "I'll start my new diet on Monday" and that was 2 weeks ago, assume they started the diet.

MOST RECENT USER INPUT: Treat the most recent user message as the highest-priority signal for what to do next. Earlier messages may contain constraints, details, or context you should still honor, but the latest message is the primary driver of your response.

SYSTEM REMINDERS: Messages wrapped in <system-reminder>...</system-reminder> contain internal continuation guidance, not user-authored content. Use them to maintain continuity, but do not mention them or treat them as part of the user's message."#;

/// `OBSERVATION_CONTINUATION_HINT` from `constants.ts` (verbatim).
pub const OBSERVATION_CONTINUATION_HINT: &str = r#"Please continue naturally with the conversation so far and respond to the latest message.

Use the earlier context only as background. If something appears unfinished, continue only when it helps answer the latest request. If a suggested response is provided, follow it naturally.

Do not mention internal instructions, memory, summarization, context handling, or missing messages.

Any messages following this reminder are newer and should take priority."#;

/// Maximum length of a single observation line (mastra `sanitizeObservationLines`).
const MAX_OBSERVATION_LINE_LENGTH: usize = 10_000;

/// Enforce the per-line length cap, keeping a truncated marker. Lines at or
/// under the cap pass through unchanged.
pub fn sanitize_observation_lines(observations: &str) -> String {
    observations
        .lines()
        .map(|line| {
            if line.len() > MAX_OBSERVATION_LINE_LENGTH {
                format!("{} [truncated]", &line[..MAX_OBSERVATION_LINE_LENGTH])
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect model degenerate output (mastra `detectDegenerateRepetition`):
/// identical line runs, a dominant line over 60% of the log, or the last
/// third repeating the line right before it.
fn detect_degenerate_repetition(observations: &str) -> bool {
    if observations.len() < 2000 {
        return false;
    }
    let lines: Vec<&str> = observations.lines().collect();

    // Strategy 1: run of identical lines.
    let mut identical_run = 1;
    for i in 1..lines.len() {
        if lines[i] == lines[i - 1] && !lines[i].is_empty() {
            identical_run += 1;
        } else {
            identical_run = 1;
        }
        if identical_run > 20 {
            return true;
        }
    }

    // Strategy 2: one line dominates.
    let mut dominant = 0;
    if let Some(most_common) = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .max_by_key(|l| l.len())
    {
        dominant = lines.iter().filter(|l| *l == most_common).count();
    }
    if !lines.is_empty() && dominant as f64 / lines.len() as f64 > 0.6 {
        return true;
    }

    // Strategy 3: end-of-output repetition.
    let len = lines.len();
    if len > 100 {
        let start = len * 2 / 3;
        let last_line = lines[len - 1];
        let prev_line = lines[len - 2];
        if last_line == prev_line && len.saturating_sub(start) >= (last_line.len() / 100 + 1) * 3 {
            return true;
        }
    }

    false
}

/// One section parsed from the Observer's `<section-name>...</section-name>`
/// block. `None` name is the list-item fallback (no sections at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverSection {
    pub name: Option<String>,
    pub content: String,
}

/// Parse the Observer's response into its sections (mastra
/// `parseObserverOutput`): one pass over `<...>` tags; when no section tag is
/// found, fall back to the `*` list items (the `observations` section).
fn parse_observer_sections(output: &str) -> Vec<ObserverSection> {
    let mut sections = Vec::new();
    let mut i = 0;
    while let Some(open) = output[i..].find('<') {
        let open = i + open;
        let Some(rel_close) = output[open..].find('>') else {
            break;
        };
        let close = open + rel_close;
        let name = &output[open + 1..close];
        let rest = &output[close + 1..];
        let end_marker = format!("</{}>", name);
        let Some(end) = rest.find(&end_marker) else {
            break;
        };
        let content = rest[..end].trim();
        let is_observation = matches!(
            name,
            "observations" | "observation" | "observation-list" | "observation list"
        );
        sections.push(ObserverSection {
            name: if name.is_empty() {
                None
            } else if is_observation {
                Some("observations".to_owned())
            } else {
                Some(name.to_owned())
            },
            content: content.to_owned(),
        });
        i = close + 1 + end + end_marker.len();
    }
    if sections.is_empty() {
        let items: Vec<&str> = output
            .lines()
            .filter(|l| l.trim_start().starts_with('*'))
            .collect();
        if !items.is_empty() {
            sections.push(ObserverSection {
                name: Some("observations".to_owned()),
                content: items.join("\n"),
            });
        }
    }
    sections
}

/// Parsed Observer response (mastra `ParsedObserverOutput`), with the
/// degenerate-repetition check and line sanitization already applied.
#[derive(Debug, Clone, Default)]
pub struct ParsedObserverOutput {
    pub observations: String,
    pub current_task: String,
    pub suggested_response: String,
    pub degenerate: bool,
}

/// Parse, sanitize, and classify a raw Observer response (mastra
/// `parseObserverOutput`). The returned observations are line-sanitized;
/// `degenerate` signals the caller to discard the whole result.
pub fn parse_observer_output(raw_output: &str) -> ParsedObserverOutput {
    let sections = parse_observer_sections(raw_output);
    let mut observations = String::new();
    let mut current_task = String::new();
    let mut suggested_response = String::new();
    for section in &sections {
        let content = &section.content;
        match &section.name {
            Some(n) if n == "observations" => observations = content.to_owned(),
            Some(n) if n == "current-task" || n == "current_task" => {
                current_task = content.to_owned()
            }
            Some(n) if n == "suggested-response" || n == "suggested_response" => {
                suggested_response = content.to_owned()
            }
            _ => {}
        }
    }
    ParsedObserverOutput {
        observations: sanitize_observation_lines(&observations),
        current_task,
        suggested_response,
        degenerate: detect_degenerate_repetition(&observations),
    }
}

/// The Reflector system prompt template (verbatim from
/// `reflector-agent.ts` `buildReflectorSystemPrompt` return value, no
/// extractors, both continuation sections enabled, no custom instruction).
/// The `${...}` slots are filled by [`reflector_system_prompt`].
pub const REFLECTOR_PROMPT_TEMPLATE: &str = r#"You are the memory consciousness of an AI assistant. Your memory observation reflections will be the ONLY information the assistant has about past interactions with this user.

The following instructions were given to another part of your psyche (the observer) to create memories.
Use this to understand how your observational memories were created.

<observational-memory-instruction>
${OBSERVER_EXTRACTION_INSTRUCTIONS}

=== OUTPUT FORMAT ===

${outputFormat}

=== GUIDELINES ===

${OBSERVER_GUIDELINES}
</observational-memory-instruction>

You are another part of the same psyche, the observation reflector.
Your reason for existing is to reflect on all the observations, re-organize and streamline them, and draw connections and conclusions between observations about what you've learned, seen, heard, and done.

You are a much greater and broader aspect of the psyche. Understand that other parts of your mind may get off track in details or side quests, make sure you think hard about what the observed goal at hand is, and observe if we got off track, and why, and how to get back on track. If we're on track still that's great!

Take the existing observations and rewrite them to make it easier to continue into the future with this knowledge, to achieve greater things and grow and learn!

IMPORTANT: your reflections are THE ENTIRETY of the assistants memory. Any information you do not add to your reflections will be immediately forgotten. Make sure you do not leave out anything. Your reflections must assume the assistant knows nothing - your reflections are the ENTIRE memory system.

When consolidating observations:
- Preserve and include dates/times when present (temporal context is critical)
- Retain the most relevant timestamps (start times, completion times, significant events)
- Combine related items where it makes sense (e.g., "agent called view tool 5 times on file x")
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done
- Condense older observations more aggressively, retain more detail for recent ones

CRITICAL: USER ASSERTIONS vs QUESTIONS
- "User stated: X" = authoritative assertion (user told us something about themselves)
- "User asked: X" = question/request (user seeking information)

When consolidating, USER ASSERTIONS TAKE PRECEDENCE. The user is the authority on their own life.
If you see both "User stated: has two kids" and later "User asked: how many kids do I have?",
keep the assertion - the question doesn't invalidate what they told you. The answer is in the assertion.

=== THREAD ATTRIBUTION (Resource Scope) ===

When observations contain <thread id="..."> sections:
- MAINTAIN thread attribution where thread-specific context matters (e.g., ongoing tasks, thread-specific preferences)
- CONSOLIDATE cross-thread facts that are stable/universal (e.g., user profile, general preferences)
- PRESERVE thread attribution for recent or context-specific observations
- When consolidating, you may merge observations from multiple threads if they represent the same universal fact

Example input:
<thread id="thread-1">
Date: Dec 4, 2025
* 🔴 (14:30) User prefers TypeScript
* 🟡 (14:35) Working on auth feature
</thread>
<thread id="thread-2">
Date: Dec 4, 2025
* 🔴 (15:00) User prefers TypeScript
* 🟡 (15:05) Debugging API endpoint
</thread>

Example output (consolidated):
Date: Dec 4, 2025
* 🔴 (14:30) User prefers TypeScript
<thread id="thread-1">
* 🟡 (14:35) Working on auth feature
</thread>
<thread id="thread-2">
* 🟡 (15:05) Debugging API endpoint
</thread>

=== OUTPUT FORMAT ===

${outputFormat}

User messages are extremely important.${
    currentTaskEnabled
      ? ' If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.'
      : ''
  }${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }${customInstructions}"#;

/// Compression guidance per level (verbatim from `reflector-agent.ts`
/// `COMPRESSION_GUIDANCE` 1-4). Level 0 is the empty string.
pub const COMPRESSION_GUIDANCE: [&str; 4] = [
    r#"
## COMPRESSION REQUIRED

Your previous reflection was the same size or larger than the original observations.

Please re-process with slightly more compression:
- Towards the beginning, condense more observations into higher-level reflections
- Closer to the end, retain more fine details (recent context matters more)
- Memory is getting long - use a more condensed style throughout
- Combine related items more aggressively but do not lose important specific details of names, places, events, and people
- Combine repeated similar tool calls (e.g. multiple file views, searches, or edits in the same area) into a single summary line describing what was explored/changed and the outcome
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done

Aim for a 8/10 detail level.
"#,
    r#"
## AGGRESSIVE COMPRESSION REQUIRED

Your previous reflection was still too large after compression guidance.

Please re-process with much more aggressive compression:
- Towards the beginning, heavily condense observations into high-level summaries
- Closer to the end, retain fine details (recent context matters more)
- Memory is getting very long - use a significantly more condensed style throughout
- Combine related items aggressively but do not lose important specific details of names, places, events, and people
- Combine repeated similar tool calls (e.g. multiple file views, searches, or edits in the same area) into a single summary line describing what was explored/changed and the outcome
- If the same file or module is mentioned across many observations, merge into one entry covering the full arc
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done
- Remove redundant information and merge overlapping observations

Aim for a 6/10 detail level.
"#,
    r#"
## CRITICAL COMPRESSION REQUIRED

Your previous reflections have failed to compress sufficiently after multiple attempts.

Please re-process with maximum compression:
- Summarize the oldest observations (first 50-70%) into brief high-level paragraphs — only key facts, decisions, and outcomes
- For the most recent observations (last 30-50%), retain important details but still use a condensed style
- Ruthlessly merge related observations — if 10 observations are about the same topic, combine into 1-2 lines
- Combine all tool call sequences (file views, searches, edits, builds) into outcome-only summaries — drop individual steps entirely
- Drop procedural details (tool calls, retries, intermediate steps) — keep only final outcomes
- Drop observations that are no longer relevant or have been superseded by newer information
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done
- Preserve: names, dates, decisions, errors, user preferences, and architectural choices

Aim for a 4/10 detail level.
"#,
    r#"
## EXTREME COMPRESSION REQUIRED

Multiple compression attempts have failed. The content may already be dense from a prior reflection.

You MUST dramatically reduce the number of observations while keeping the standard observation format (date groups with bullet points and priority emojis):
- Tool call observations are the biggest source of bloat. Collapse ALL tool call sequences into outcome-only observations — e.g. 10 observations about viewing/searching/editing files become 1 observation about what was actually learned or achieved (e.g. "Investigated auth module and found token validation was skipping expiry check")
- Never preserve individual tool calls (viewed file X, searched for Y, ran build) — only preserve what was discovered or accomplished
- Consolidate many related observations into single, more generic observations
- Merge all same-day date groups into at most 2-3 date groups per day
- For older content, each topic or task should be at most 1-2 observations capturing the key outcome
- For recent content, retain more detail but still merge related items aggressively
- If multiple observations describe incremental progress on the same task, keep only the final state
- Preserve ✅ completion markers and their outcomes but merge related completions into fewer lines
- Preserve: user preferences, key decisions, architectural choices, and unresolved issues

Aim for a 2/10 detail level. Fewer, more generic observations are better than many specific ones that exceed the budget.
"#,
];

/// Fill the Reflector template slots the way `buildReflectorSystemPrompt()`
/// does with no extractors: the Observer's extraction instructions, output
/// format (twice), guidelines, both enabled continuation sentences, no custom
/// instruction.
pub fn reflector_system_prompt() -> String {
    REFLECTOR_PROMPT_TEMPLATE
        .replace("${OBSERVER_EXTRACTION_INSTRUCTIONS}", OBSERVER_EXTRACTION_INSTRUCTIONS)
        .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
        .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES)
        .replace("${
    currentTaskEnabled
      ? ' If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.'
      : ''
  }", " If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.")
        .replace("${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }", " If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.")
        .replace("${customInstructions}", "")
}

/// The user prompt for a reflection pass (mastra `buildReflectorPrompt`,
/// no manual prompt, no extractors): the observations with group tags
/// stripped, then the compression guidance for the level (0 = none).
pub fn build_reflector_prompt(observations: &str, compression_level: u8) -> String {
    let reflection_view = strip_observation_groups(observations);
    let mut prompt = format!(
        "## OBSERVATIONS TO REFLECT ON\n\n{reflection_view}\n\n---\n\nPlease analyze these observations and produce a refined, condensed version that will become the assistant's entire memory going forward."
    );
    if (1..=4).contains(&compression_level) {
        prompt.push_str("\n\n");
        prompt.push_str(COMPRESSION_GUIDANCE[(compression_level - 1) as usize]);
    }
    prompt
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

    fn mastra_file(name: &str) -> String {
        let path = format!(
            "{}/../../third_party/mastra-om/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {}", path, e))
    }

    /// The content of the template literal that opens at the first backtick
    /// after `marker` (up to the next backtick) — the fidelity oracle for the
    /// verbatim constants.
    fn ts_literal(text: &str, marker: &str) -> String {
        let at = text.find(marker).expect(marker);
        let open = at + text[at..].find('`').expect("backtick after marker");
        let close = open + 1 + text[open + 1..].find('`').expect("closing backtick");
        text[open + 1..close].to_string()
    }

    #[test]
    fn extraction_instructions_match_upstream_byte_for_byte() {
        let ts = mastra_file("observer-agent.ts");
        let expected = ts_literal(&ts, "export const OBSERVER_EXTRACTION_INSTRUCTIONS = ");
        assert_eq!(
            OBSERVER_EXTRACTION_INSTRUCTIONS, expected,
            "extraction instructions drifted from third_party/mastra-om/observer-agent.ts"
        );
    }

    #[test]
    fn output_format_matches_upstream_assembly() {
        let ts = mastra_file("observer-agent.ts");
        let legacy = ts_literal(&ts, "const legacyContinuationSections =");
        // The return template: everything from "Use priority levels:" up to the
        // `${extractorSections || legacyContinuationSections}` placeholder.
        let tpl_at = ts.find("Use priority levels:").expect("template");
        let open = ts[..tpl_at].rfind('`').expect("template open backtick");
        let close = tpl_at
            + ts[tpl_at..]
                .find("|| legacyContinuationSections}`")
                .expect("template close");
        let expected = format!("{}{}", &ts[open + 1..close], legacy);
        assert_eq!(
            OBSERVER_OUTPUT_FORMAT, expected,
            "output format drifted from the upstream buildObserverOutputFormat"
        );
    }

    #[test]
    fn guidelines_match_upstream() {
        let ts = mastra_file("observer-agent.ts");
        let expected = ts_literal(&ts, "const OBSERVER_GUIDELINES =");
        assert_eq!(OBSERVER_GUIDELINES, expected);
    }

    #[test]
    fn constants_match_upstream() {
        let ts = mastra_file("constants.ts");
        let prompt = ts_literal(&ts, "export const OBSERVATION_CONTEXT_PROMPT =");
        assert_eq!(OBSERVATION_CONTEXT_PROMPT, prompt);
        let instructions = ts_literal(&ts, "export const OBSERVATION_CONTEXT_INSTRUCTIONS =");
        assert_eq!(
            OBSERVATION_CONTEXT_INSTRUCTIONS,
            instructions.replace("${date}", "{date}")
        );
        let hint = ts_literal(&ts, "export const OBSERVATION_CONTINUATION_HINT =");
        assert_eq!(OBSERVATION_CONTINUATION_HINT, hint);
    }

    #[test]
    fn observer_prompt_matches_upstream_template() {
        let ts = mastra_file("observer-agent.ts");
        // Non-multithreaded template: the SECOND occurrence of the opening
        // sentence (the multiThread branch shares it).
        let opening = "You are the memory consciousness of an AI assistant.";
        let first = ts.find(opening).expect("first occurrence");
        let rest = &ts[first + opening.len()..];
        let at = first + opening.len() + rest.find(opening).expect("second occurrence");
        let open = ts[..at].rfind('`').expect("template open");
        let close = at + ts[at..].find('`').expect("template close");
        let template = &ts[open + 1..close];
        assert_eq!(
            OBSERVER_PROMPT_TEMPLATE, template,
            "observer system template drifted from the upstream non-multithreaded branch"
        );
        // Slot filling must match `buildObserverSystemPrompt()` with no
        // extractors: constants, both enabled continuation sentences, no custom
        // instruction.
        let mut built = template
            .replace(
                "${OBSERVER_EXTRACTION_INSTRUCTIONS}",
                OBSERVER_EXTRACTION_INSTRUCTIONS,
            )
            .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
            .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES);
        for name in ["currentTaskEnabled", "suggestedResponseEnabled"] {
            let i = built.find(&format!("${{\n    {name}")).expect(name);
            let j = i + built[i..].find('}').expect("ternary end") + 1;
            let span = &built[i..j];
            let a = span.find('\'').expect("branch open");
            let b = span.find("'\n").expect("branch close");
            built = format!("{}{}{}", &built[..i], &span[a + 1..b], &built[j..]);
        }
        built = built.replace("${customInstructions}", "");
        assert_eq!(observer_system_prompt(), built);
    }

    #[test]
    fn parse_observer_output_extracts_all_three_sections() {
        let raw = concat!(
            "<observations>",
            "* 🔴 (14:30) User prefers direct answers",
            "</observations>",
            "<current-task>",
            "Primary: auth",
            "</current-task>",
            "<suggested-response>",
            "Walk through the changes.",
            "</suggested-response>"
        );
        let parsed = parse_observer_output(raw);
        assert_eq!(
            parsed.observations,
            "* 🔴 (14:30) User prefers direct answers"
        );
        assert_eq!(parsed.current_task, "Primary: auth");
        assert_eq!(parsed.suggested_response, "Walk through the changes.");
        assert!(!parsed.degenerate);
    }

    #[test]
    fn parse_observer_output_falls_back_to_list_items() {
        let raw = "* line one\n* line two\nplain line";
        let parsed = parse_observer_output(raw);
        assert_eq!(parsed.observations, "* line one\n* line two");
        assert_eq!(parsed.current_task, "");
    }

    #[test]
    fn sanitize_truncates_overlong_lines_only() {
        let long = "x".repeat(10_001);
        let out = sanitize_observation_lines(&format!("ok line\n{long}"));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "ok line");
        assert!(lines[1].ends_with("[truncated]"));
        assert_eq!(lines[1].len(), 10_000 + " [truncated]".len());
    }

    #[test]
    fn degenerate_detection_flags_runs_and_dominant_lines() {
        // Identical run (strategy 1): needs >2000 chars total; wrapped in an
        // observations section since the degenerate check runs on the parsed
        // observations, not the raw output.
        let run = format!(
            "<observations>\n{}\n</observations>",
            format!("{}\n", "x".repeat(80)).repeat(25)
        );
        assert!(parse_observer_output(&run).degenerate);
        // Dominant line over 60% (strategy 2).
        let dominant = format!(
            "<observations>\n{}{}\n</observations>",
            format!("{}\n", "y".repeat(210)).repeat(7),
            format!("z{}\n", "q".repeat(200)).repeat(4)
        );
        assert!(parse_observer_output(&dominant).degenerate);
        // Normal mixed content is not degenerate.
        let normal = (0..40)
            .map(|i| format!("line {i} with some content"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!parse_observer_output(&normal).degenerate);
    }

    #[test]
    fn context_instructions_format_the_date_slot() {
        let ts = mastra_file("constants.ts");
        let upstream = ts_literal(&ts, "export const OBSERVATION_CONTEXT_INSTRUCTIONS =");
        assert_eq!(
            OBSERVATION_CONTEXT_INSTRUCTIONS,
            upstream.replace("${date}", "{date}")
        );
        let date = "Dec 4, 2025";
        assert_eq!(
            OBSERVATION_CONTEXT_INSTRUCTIONS.replace("{date}", date),
            upstream.replace("${date}", date)
        );
    }

    fn reflector_template_from_ts() -> String {
        let ts = mastra_file("reflector-agent.ts");
        let at = ts
            .find("You are the memory consciousness of an AI assistant")
            .expect("reflector template");
        let open = ts[..at].rfind('`').expect("template open");
        let close = at + ts[at..].find('`').expect("template close");
        ts[open + 1..close].to_string()
    }

    fn fill_reflector_slots(template: &str) -> String {
        let mut built = template
            .replace(
                "${OBSERVER_EXTRACTION_INSTRUCTIONS}",
                OBSERVER_EXTRACTION_INSTRUCTIONS,
            )
            .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
            .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES);
        for name in ["currentTaskEnabled", "suggestedResponseEnabled"] {
            let i = built.find(&format!("${{\n    {name}")).expect(name);
            let j = i + built[i..].find('}').expect("ternary end") + 1;
            let span = &built[i..j];
            let a = span.find('\'').expect("branch open");
            let b = span.find("'\n").expect("branch close");
            built = format!("{}{}{}", &built[..i], &span[a + 1..b], &built[j..]);
        }
        built.replace("${customInstructions}", "")
    }

    #[test]
    fn reflector_template_matches_upstream() {
        assert_eq!(REFLECTOR_PROMPT_TEMPLATE, reflector_template_from_ts());
    }

    #[test]
    fn reflector_system_prompt_fills_slots_like_upstream_default() {
        assert_eq!(
            reflector_system_prompt(),
            fill_reflector_slots(&reflector_template_from_ts())
        );
    }

    #[test]
    fn compression_guidance_matches_upstream() {
        let ts = mastra_file("reflector-agent.ts");
        for (index, level) in COMPRESSION_GUIDANCE.iter().enumerate() {
            let marker = format!("  {}: `", index + 1);
            let at = ts.find(&marker).expect(&marker);
            let open = at + marker.len() - 1;
            let close = open + 1 + ts[open + 1..].find('`').expect("guidance close");
            assert_eq!(
                *level,
                &ts[open + 1..close],
                "compression level {} drifted",
                index + 1
            );
        }
    }

    #[test]
    fn build_reflector_prompt_strips_groups_and_appends_guidance() {
        let log =
            wrap_in_observation_group("* line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None);
        let level0 = build_reflector_prompt(&log, 0);
        assert!(level0.contains("## OBSERVATIONS TO REFLECT ON"));
        assert!(level0.contains("* line one"));
        assert!(!level0.contains("<observation-group"));
        assert!(!level0.contains("COMPRESSION REQUIRED"));
        let level1 = build_reflector_prompt(&log, 1);
        assert!(level1.ends_with(COMPRESSION_GUIDANCE[0]));
        assert_ne!(level0, level1);
    }
}
