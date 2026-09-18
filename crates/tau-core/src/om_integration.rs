//! OM integration (ADR-0004, spec §4): wires the pure `om` module into the
//! running session — the per-session record as appended session entries,
//! turn-end observe/reflect, Phase-2 buffered chunks, context assembly, the
//! `recall` tool, and the compacted-spawn frozen prefix.

use crate::om::{self, Cursor, OmConfig, OmRecord};
use crate::session::{Entry, SessionStore};
use serde_json::Value;

/// The session entry kind that carries the OM record (spec §4: the session
/// file carries the record; the newest entry is the current state).
pub const KIND_OM: &str = "om";

/// The child session's first entry at a compacted spawn (ADR-0004): points
/// at the parent session and the source range the frozen prefix covers.
pub const KIND_SPAWN_SNAPSHOT: &str = "spawn-snapshot";

/// Fixed idle timeout for buffered-chunk activation (spec §4: v0 uses a
/// fixed idle timeout, not provider-cache-TTL auto-mapping).
pub const IDLE_ACTIVATION_SECS: u64 = 60;

/// One buffered Observer chunk (spec §4 Phase 2): the observed text for a
/// raw range, held in memory until activation promotes it to the log —
/// promotion itself makes no LLM call. A restart loses the buffer; the
/// threshold path then re-observes the range (rework, not data loss).
#[derive(Debug, Clone)]
pub struct BufferedChunk {
    /// (first entry id, last entry id) of the observed raw range.
    pub range: (String, String),
    /// The entry's timestamp the cursor advances to on promotion.
    pub last_ts: u64,
    /// The observed text, already wrapped in its provenance group.
    pub text: String,
    pub tokens: u32,
}

/// Per-session OM state, owned by the agent loop (the loop performs no OM
/// discovery of its own — the caller seeds this, like the system prompt).
#[derive(Clone)]
pub struct OmState {
    pub config: OmConfig,
    pub record: OmRecord,
    pub buffered: Vec<BufferedChunk>,
    /// The last entry a buffered run covered (mastra's buffer cursor, the
    /// port's `lastBufferedAtTokens`): runs are disjoint — a new buffer run
    /// covers only entries after this one. In memory like the chunks
    /// themselves (a restart re-observes via the threshold path).
    buffer_cursor: Option<String>,
    /// The log changed since the last assembled context: the next assembly
    /// carries the continuation hint so the model doesn't react to the raw
    /// history vanishing (spec §4).
    pub changed: bool,
}

impl OmState {
    /// Fold the config's absolute `buffer_increment` into the module's
    /// `buffer_activation` ratio (1 − increment/threshold; the two forms
    /// agree at the defaults: 6k over 30k = 0.8).
    pub fn from_config(om: &crate::config::Om, record: OmRecord) -> Self {
        let observe = om.observe_threshold.max(1) as f64;
        let activation = (1.0 - om.buffer_increment as f64 / observe).clamp(0.0, 1.0);
        Self {
            config: OmConfig {
                observe_threshold: om.observe_threshold as u32,
                reflect_threshold: om.reflect_threshold as u32,
                buffer_activation: activation,
                share_token_budget: false,
            },
            record,
            buffered: Vec::new(),
            buffer_cursor: None,
            changed: false,
        }
    }

    /// The current record: the last `om` entry on the active branch,
    /// reconstructed on open (spec §4: the record is appended, not
    /// maintained in place).
    pub fn load_record(store: &mut SessionStore) -> OmRecord {
        let entries = match store.entries_range(0, usize::MAX) {
            Ok(e) => e,
            Err(_) => return OmRecord::default(),
        };
        let leaf = store.leaf().ok().flatten().map(|e| e.id);
        branch_entries(&entries, leaf.as_deref())
            .into_iter()
            .rev()
            .find(|e| e.kind == KIND_OM)
            .and_then(|e| serde_json::from_value(e.payload).ok())
            .unwrap_or_default()
    }

    /// Persist the record as a new `om` entry (the newest entry wins).
    pub fn save(&self, store: &mut SessionStore) -> Result<(), crate::session::Error> {
        let parent = store.leaf()?.map(|e| e.id);
        let payload = serde_json::to_value(&self.record)
            .map_err(|e| crate::session::Error::Other(e.to_string()))?;
        store
            .append(KIND_OM, payload, parent.as_deref())
            .map(|_| ())
    }
}

/// The active branch, root to leaf: the leaf's parent chain walked over the
/// file-order entries (the cursor is path-scoped, spec §4).
pub fn branch_entries(entries: &[Entry], leaf_id: Option<&str>) -> Vec<Entry> {
    let by_id: std::collections::HashMap<&str, &Entry> =
        entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut path = Vec::new();
    let mut cur = leaf_id;
    while let Some(id) = cur {
        let Some(entry) = by_id.get(id) else {
            break;
        };
        path.push((*entry).clone());
        cur = entry.parent.as_deref();
    }
    path.reverse();
    path
}

/// A fork copies the parent's record at the fork point (observations +
/// cursor); a fork owns its copy outright — no frozen prefix (ADR-0004: a
/// fork is the same session's history).
pub fn fork_record(parent: &OmRecord) -> OmRecord {
    OmRecord {
        frozen_prefix: String::new(),
        active_observations: parent.live_observations(),
        cursor: parent.cursor.clone(),
        generation: parent.generation,
        observation_tokens: parent.observation_tokens,
        pending_tokens: 0,
        prefix_demoted: false,
    }
}
/// An OM failure: session storage or the model call.
#[derive(Debug)]
pub enum OmError {
    Session(crate::session::Error),
    Provider(crate::provider::ProviderError),
}

impl std::fmt::Display for OmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(e) => write!(f, "session: {e}"),
            Self::Provider(e) => write!(f, "provider: {e}"),
        }
    }
}

impl std::error::Error for OmError {}

impl From<crate::session::Error> for OmError {
    fn from(e: crate::session::Error) -> Self {
        Self::Session(e)
    }
}

impl From<crate::provider::ProviderError> for OmError {
    fn from(e: crate::provider::ProviderError) -> Self {
        Self::Provider(e)
    }
}

/// A raw conversation entry (spec §4: raw = what the Observer observes);
/// record entries (om, spawn-snapshot, system) are not raw.
fn is_raw(entry: &Entry) -> bool {
    matches!(entry.kind.as_str(), "user" | "assistant" | "tool")
}

/// The raw text of an entry for token accounting and transcripts: user and
/// assistant text, tool output. Record entries (om, spawn-snapshot, system)
/// are not raw.
fn entry_text(entry: &Entry) -> String {
    let text = entry
        .payload
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !text.is_empty() {
        return text;
    }
    let mut out = String::new();
    let reasoning = entry
        .payload
        .get("reasoning")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty());
    if let Some(reasoning) = reasoning {
        out.push_str(reasoning);
        out.push('\n');
    }
    if let Some(output) = entry.payload.get("output").and_then(Value::as_str) {
        out.push_str(output);
    }
    out
}

fn transcript(entries: &[Entry]) -> String {
    let mut out = String::new();
    for entry in entries {
        let text = entry_text(entry);
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("[{}] {}\n", entry.kind, text));
    }
    out
}

/// The boundary delimiter's timestamp: epoch millis (no chrono in the
/// dependency surface; the delimiter needs monotonicity, not a calendar).
fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

/// The OM call's sink: OM calls are not user-facing — nothing kills them.
pub struct NoopSink;

impl crate::provider::TurnSink for NoopSink {
    fn event(&mut self, _: crate::provider::TurnEvent) -> bool {
        true
    }
}

impl OmState {
    /// The unobserved raw on the active branch: the entries after the
    /// cursor. A missing cursor means the whole branch; a cursor not on
    /// this branch (a branch switch) means the branch is unobserved —
    /// the cursor is path-scoped (spec §4).
    pub fn unobserved(&self, store: &mut SessionStore) -> Result<Vec<Entry>, OmError> {
        let all = store.entries_range(0, usize::MAX)?;
        let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
        let branch = branch_entries(&all, leaf.as_deref());
        let Some(cursor) = &self.record.cursor else {
            return Ok(branch);
        };
        let unobserved = match branch.iter().position(|e| e.id == cursor.entry_id) {
            Some(i) => &branch[i + 1..],
            None => &branch[..],
        };
        Ok(unobserved.iter().filter(|e| is_raw(e)).cloned().collect())
    }

    pub fn pending_tokens(&self, entries: &[Entry]) -> u32 {
        entries
            .iter()
            .map(|e| om::token_count(&entry_text(e)))
            .sum()
    }

    /// Activation (no LLM call, mastra `swapBufferedToActive`): promote
    /// buffered chunks to the log up to the boundary that lands the
    /// remaining raw at or below the retention floor
    /// (`projected_message_removal`), leaving later chunks buffered. The
    /// observation cursor advances to the last promoted entry; `false` when
    /// there is nothing to promote.
    pub fn promote(&mut self, store: &mut SessionStore) -> Result<bool, OmError> {
        if self.buffered.is_empty() {
            return Ok(false);
        }
        let chunks: Vec<om::ChunkTokens> =
            self.buffered.iter().map(|c| om::ChunkTokens(c.tokens)).collect();
        let remove =
            om::projected_message_removal(&chunks, &self.config, self.record.pending_tokens);
        if remove == 0 {
            // Pending is already at or below the retention floor: the
            // chunks stay buffered for the next activation.
            return Ok(false);
        }
        // The returned count is the cumulative at the selected boundary;
        // the cumulative sums are monotone, so the walk lands exactly on it.
        let mut count = 0usize;
        let mut cumulative = 0u32;
        for chunk in &self.buffered {
            cumulative = cumulative.saturating_add(chunk.tokens);
            count += 1;
            if cumulative >= remove {
                break;
            }
        }
        for chunk in &self.buffered[..count] {
            self.record.active_observations =
                om::append_observation(&self.record.active_observations, &now_iso(), &chunk.text);
            self.record.cursor = Some(Cursor {
                entry_id: chunk.range.1.clone(),
                timestamp: chunk.last_ts,
            });
        }
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        self.record.pending_tokens = self.buffered.iter().skip(count).map(|c| c.tokens).sum();
        self.buffered.drain(0..count);
        self.changed = true;
        self.save(store)?;
        Ok(true)
    }

    pub fn activation_reached(&self, pending_tokens: u32) -> bool {
        om::should_observe(pending_tokens, self.record.observation_tokens, &self.config)
    }

    fn maintain_prefix_budget(&mut self) {
        let combined = om::token_count(&self.record.live_observations());
        if !self.record.prefix_demoted && combined > self.config.reflect_threshold {
            self.record.prefix_demoted = true;
        } else if self.record.prefix_demoted
            && om::token_count(&self.record.active_observations) < self.config.reflect_threshold
        {
            self.record.prefix_demoted = false;
        }
    }
}
/// The action a turn-end plan calls for. The provider call runs outside the
/// session's write lock (the OM call is an LLM round-trip, not a user-facing
/// stream); only the sync store ops run under it.
#[derive(Debug, PartialEq)]
pub enum TurnEndAction {
    Observe { transcript: String },
    Reflect { level: u8 },
    Buffer { transcript: String },
    Done,
}

impl OmState {
    /// The turn-end decision (pure — the agent loop reads the unobserved
    /// entries and updates the pending count under the session lock, then
    /// calls this outside it): reflect at the observation threshold,
    /// observe at activation, buffer at the increment (spec §4).
    pub fn plan(&mut self, unobserved: &[Entry]) -> TurnEndAction {
        if om::should_reflect(self.record.observation_tokens, &self.config) {
            return TurnEndAction::Reflect { level: 0 };
        }
        if self.activation_reached(self.record.pending_tokens) {
            return TurnEndAction::Observe {
                transcript: transcript(unobserved),
            };
        }
        if self.record.pending_tokens >= self.config.buffer_increment() {
            // The transcript covers only the not-yet-buffered tail of the
            // unobserved range (the buffer cursor keeps runs disjoint).
            let start = match &self.buffer_cursor {
                Some(id) => match unobserved.iter().position(|e| e.id == *id) {
                    Some(i) => i + 1,
                    None => 0,
                },
                None => 0,
            };
            return TurnEndAction::Buffer {
                transcript: transcript(&unobserved[start..]),
            };
        }
        TurnEndAction::Done
    }

    /// Commit a completed turn-end action (sync — the agent loop runs it
    /// under the session lock after the LLM round-trip). The action
    /// advances: a rejected reflection level escalates, anything applied
    /// becomes `Done`.
    pub fn commit(
        &mut self,
        store: &mut SessionStore,
        action: &mut TurnEndAction,
        result: &crate::provider::TurnResult,
    ) -> Result<(), OmError> {
        let parsed = om::parse_observer_output(&result.text);
        match action {
            TurnEndAction::Observe { .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_observation(store, &parsed.observations)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Buffer { .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_buffer(store, &parsed.observations)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Reflect { level } => {
                let source = self.record.reflect_source().to_owned();
                // The committed suffix is the parsed <observations> content,
                // never the raw response (mastra `parseReflectorOutput`):
                // the <current-task>/<suggested-response> sections are not
                // observation material and must not pollute the log.
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
                    self.changed = true;
                    self.save(store)?;
                    *action = TurnEndAction::Done;
                } else if result.completed && result.mid_stream_errors.is_empty() {
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
    /// Apply one observation: wrap the unobserved entries in their
    /// provenance group, append it after the boundary, advance the cursor
    /// past them, persist (spec §4).
    fn apply_observation(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
    ) -> Result<(), OmError> {
        let entries = self.unobserved(store)?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let first = entries.first().unwrap_or(last);
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        self.record.active_observations =
            om::append_observation(&self.record.active_observations, &now_iso(), &wrapped);
        self.record.cursor = Some(Cursor {
            entry_id: last.id.clone(),
            timestamp: last.timestamp,
        });
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        self.record.pending_tokens = 0;
        self.changed = true;
        self.save(store)?;
        Ok(())
    }

    /// Apply one buffered observation over the entries NOT already covered
    /// by a previous run (the buffer cursor keeps runs disjoint, mastra's
    /// `lastBufferedAtTokens`): no observation-cursor advance and no save —
    /// the buffer is in memory until activation (the shared tail of the
    /// observe path).
    fn apply_buffer(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
    ) -> Result<(), OmError> {
        let entries = self.unbuffered(store)?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let first = entries.first().unwrap_or(last);
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        self.buffered.push(BufferedChunk {
            range: (first.id.clone(), last.id.clone()),
            last_ts: last.timestamp,
            text: wrapped,
            tokens: om::token_count(observations),
        });
        self.buffer_cursor = Some(last.id.clone());
        Ok(())
    }

    /// The raw entries not yet covered by a buffered run: after the buffer
    /// cursor, or the whole branch when no run has happened (the buffer
    /// cursor never lags the observation cursor — promotion advances the
    /// latter to the buffered material's end).
    fn unbuffered(&self, store: &mut SessionStore) -> Result<Vec<Entry>, OmError> {
        let all = store.entries_range(0, usize::MAX)?;
        let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
        let branch = branch_entries(&all, leaf.as_deref());
        let after = match &self.buffer_cursor {
            Some(id) => match branch.iter().position(|e| e.id == *id) {
                Some(i) => &branch[i + 1..],
                None => &branch[..],
            },
            None => &branch[..],
        };
        Ok(after.iter().filter(|e| is_raw(e)).cloned().collect())
    }

    /// The Reflector prompt for a level (spec §4): the frozen variant
    /// (ADR-0004) when a frozen prefix is present — the prefix never
    /// enters the prompt body and stays byte-verbatim.
    pub fn reflector_prompt(&self, level: u8) -> String {
        let source = self.record.reflect_source();
        if self.record.frozen_prefix.is_empty() {
            om::build_reflector_prompt(source, level)
        } else {
            om::build_reflector_prompt_frozen(&self.record.frozen_prefix, source, level)
        }
    }

    /// The context's system-prompt part (spec §4): the base prompt, the
    /// observation log (a demoted prefix drops out — it stays in the
    /// session file, reachable via `recall`), the active task's resume
    /// contract (ticket #24 fills the slot; the loop passes `None`
    /// today), and the continuation hint after a log change. Pure over the
    /// record (no store access), so the one-shot `changed` flip sticks
    /// when the loop runs it on the persistent state under the lock.
    pub fn assemble_context(&mut self, base: &str, task_contract: Option<&str>) -> String {
        let mut instructions = base.to_owned();
        let observations = self.record.live_observations();
        if !observations.is_empty() {
            instructions.push_str("\n\n");
            instructions.push_str(om::OBSERVATION_CONTEXT_PROMPT);
            instructions.push('\n');
            instructions.push_str(om::OBSERVATION_CONTEXT_INSTRUCTIONS);
            instructions.push_str("\n\n");
            instructions.push_str(&observations);
        }
        if let Some(contract) = task_contract {
            instructions.push_str("\n\n# Task (resume contract)\n");
            instructions.push_str(contract);
        }
        if self.changed && !observations.is_empty() {
            self.changed = false;
            instructions.push_str("\n\n<system-reminder>");
            instructions.push_str(om::OBSERVATION_CONTINUATION_HINT);
            instructions.push_str("</system-reminder>");
        }
        instructions
    }

    /// The raw window over already-read entries (pure — no store
    /// access): the active-branch entries after the cursor, pruned to
    /// the retention floor from the head, never cutting a tool result
    /// from its call (spec §4 cut rule).
    pub fn raw_window_from(&self, entries: &[Entry], leaf_id: Option<&str>) -> Vec<Entry> {
        let branch = branch_entries(entries, leaf_id);
        let unobserved = match self.record.cursor.as_ref() {
            Some(cursor) => match branch.iter().position(|e| e.id == cursor.entry_id) {
                Some(i) => &branch[i + 1..],
                None => &branch[..],
            },
            None => &branch[..],
        };
        let mut raw: Vec<Entry> = unobserved.iter().filter(|e| is_raw(e)).cloned().collect();
        let floor = self.config.retention_floor() as u64;
        let total: u64 = raw
            .iter()
            .map(|e| om::token_count(&entry_text(e)) as u64)
            .sum();
        if total > floor {
            let mut keep_from = 0;
            let mut running = total;
            for (i, entry) in raw.iter().enumerate() {
                running = running.saturating_sub(om::token_count(&entry_text(entry)) as u64);
                keep_from = i + 1;
                if running <= floor {
                    break;
                }
            }
            raw = raw.split_off(keep_from);
            // A window starting at a tool result would orphan it from its
            // call: advance past the leading result run.
            while let Some(entry) = raw.first() {
                if entry.kind == "tool" {
                    raw.remove(0);
                } else {
                    break;
                }
            }
        }
        raw
    }
}

/// The `recall` tool (spec §4): browse the raw entries an observation group
/// covers. Groups carry `range="startEntryId:endEntryId"` (comma-joined
/// segments for a merged span); browsing only, no vector search.
pub fn recall(store: &mut SessionStore, record: &OmRecord, args: &Value) -> String {
    let Some(group_id) = args.get("group").and_then(Value::as_str) else {
        return "recall: missing \"group\"".into();
    };
    // Search the whole log: the frozen prefix is included even while
    // demoted (it stays in the record, recall reaches it — spec §4).
    let log = format!("{}{}", record.frozen_prefix, record.active_observations);
    let Some(group) = om::parse_observation_groups(&log)
        .into_iter()
        .find(|g| g.id == group_id)
    else {
        return format!("no observation group {group_id}");
    };
    let segments: Vec<&str> = group
        .range
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let (Some(start), Some(end)) = (
        segments.first().and_then(|s| s.split(':').next()),
        segments.last().and_then(|s| s.split(':').next_back()),
    ) else {
        return "the group has no range".into();
    };
    let Ok(entries) = store.entries_range(0, usize::MAX) else {
        return "session read failed".into();
    };
    let Some(start_idx) = entries.iter().position(|e| e.id == start) else {
        return "range start not in this session".into();
    };
    let end_idx = entries
        .iter()
        .position(|e| e.id == end)
        .map(|i| i + 1)
        .unwrap_or(entries.len());
    entries[start_idx..end_idx]
        .iter()
        .map(|e| {
            let text = entry_text(e);
            let preview: String = text.chars().take(200).collect();
            format!("{} {} {}\n", e.id, e.kind, preview)
        })
        .collect()
}

/// The idle gap before the latest turn, in seconds: the timestamp distance
/// between that turn's first user entry and the entry before it (spec §4:
/// v0 activates pending buffered chunks on a fixed idle timeout, not on
/// provider-cache TTLs). Zero when the branch starts with the turn's user
/// entry (a fresh session).
pub fn idle_gap_secs(all: &[Entry], leaf_id: Option<&str>) -> u64 {
    let branch = branch_entries(all, leaf_id);
    // Back over the current turn's model output (assistant + tool entries)
    // to its user run, then to the run's first entry.
    let mut i = branch.len();
    while i > 0 && matches!(branch[i - 1].kind.as_str(), "assistant" | "tool") {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    while i > 0 && branch[i - 1].kind == "user" {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    branch[i].timestamp.saturating_sub(branch[i - 1].timestamp) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_fold_maps_increment_to_activation() {
        let om = crate::config::Om::default(); // 30k / 40k / 6k
        let state = OmState::from_config(&om, OmRecord::default());
        assert_eq!(state.config.observe_threshold, 30_000);
        assert_eq!(state.config.reflect_threshold, 40_000);
        assert!((state.config.buffer_activation - 0.8).abs() < 1e-9);
        assert_eq!(state.config.buffer_increment(), 6_000);
    }

    #[test]
    fn record_round_trips_through_the_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), "s1");
        store.create().unwrap();

        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        state.record.active_observations = "first".into();
        state.save(&mut store).unwrap();
        assert_eq!(
            OmState::load_record(&mut store).active_observations,
            "first"
        );

        state.record.active_observations = "second".into();
        state.save(&mut store).unwrap();
        // The newest entry is the current state.
        assert_eq!(
            OmState::load_record(&mut store).active_observations,
            "second"
        );
    }

    #[test]
    fn load_without_any_record_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), "s1");
        store.create().unwrap();
        let record = OmState::load_record(&mut store);
        assert!(record.frozen_prefix.is_empty());
        assert!(record.active_observations.is_empty());
        assert!(record.cursor.is_none());
        assert_eq!(record.generation, 0);
    }

    #[test]
    fn fork_copies_observations_and_cursor_without_a_prefix() {
        let parent = OmRecord {
            frozen_prefix: "frozen".into(),
            active_observations: "live".into(),
            cursor: Some(Cursor {
                entry_id: "00000005".into(),
                timestamp: 7,
            }),
            generation: 3,
            observation_tokens: 42,
            pending_tokens: 9,
            prefix_demoted: false,
        };
        let fork = fork_record(&parent);
        assert!(fork.frozen_prefix.is_empty());
        assert_eq!(fork.active_observations, "frozenlive");
        assert_eq!(fork.cursor, parent.cursor);
        assert_eq!(fork.generation, 3);
    }
    fn store_with_text_entries(dir: &std::path::Path, n: usize, chars_each: usize) -> SessionStore {
        let mut store = SessionStore::for_workspace(dir, "s1");
        store.create().unwrap();
        let mut parent: Option<String> = None;
        for _ in 0..n {
            let text = "w".repeat(chars_each);
            let entry = store
                .append(
                    "user",
                    serde_json::json!({ "text": text, "lane": "follow-up" }),
                    parent.as_deref(),
                )
                .unwrap();
            parent = Some(entry.id);
        }
        store
    }

    fn turn_result(text: &str) -> crate::provider::TurnResult {
        crate::provider::TurnResult {
            text: text.into(),
            reasoning: String::new(),
            usage: None,
            completed: true,
            calls: Vec::new(),
            mid_stream_errors: Vec::new(),
        }
    }

    #[test]
    fn plan_picks_observe_at_activation_and_commit_advances_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 3, 100);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        state.record.pending_tokens = state.config.observe_threshold;
        let unobserved = state.unobserved(&mut store).unwrap();
        match state.plan(&unobserved) {
            TurnEndAction::Observe { .. } => {}
            other => panic!("expected Observe, got {other:?}"),
        }
        let result = turn_result("<observations>user is setting up a workbench</observations>");
        let mut action = TurnEndAction::Observe {
            transcript: String::new(),
        };
        state.commit(&mut store, &mut action, &result).unwrap();
        assert_eq!(action, TurnEndAction::Done);
        let cursor = state.record.cursor.clone().unwrap();
        assert_eq!(cursor.entry_id, "00000003");
        let groups = crate::om::parse_observation_groups(&state.record.active_observations);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].range, "00000000:00000003");
        assert!(groups[0].content.contains("workbench"));
        assert_eq!(state.record.pending_tokens, 0);
        // The observation persists across a re-load from the session file.
        assert!(
            OmState::load_record(&mut store)
                .active_observations
                .contains("workbench")
        );
    }

    #[test]
    fn plan_buffers_below_activation_and_commit_holds_the_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 25, 1000);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        // 25k tokens: below the 30k threshold, past the 6k increment.
        state.record.pending_tokens = 25_000;
        let unobserved = state.unobserved(&mut store).unwrap();
        match state.plan(&unobserved) {
            TurnEndAction::Buffer { .. } => {}
            other => panic!("expected Buffer, got {other:?}"),
        }
        let result = turn_result("<observations>chunk</observations>");
        let mut action = TurnEndAction::Buffer {
            transcript: String::new(),
        };
        state.commit(&mut store, &mut action, &result).unwrap();
        assert_eq!(state.buffered.len(), 1);
        assert!(state.record.active_observations.is_empty());
        assert!(state.record.cursor.is_none());
    }

    #[test]
    fn plan_reflects_at_the_observation_threshold_and_commit_rewrites_the_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 1, 100);
        let prefix = "FROZEN PARENT LOG".to_owned();
        let suffix =
            crate::om::wrap_in_observation_group(&"x".repeat(4000), "00000001:00000002", "b", None);
        let mut state = OmState::from_config(
            &crate::config::Om::default(),
            OmRecord {
                frozen_prefix: prefix.clone(),
                active_observations: suffix,
                observation_tokens: crate::config::Om::default().reflect_threshold as u32,
                ..Default::default()
            },
        );
        match state.plan(&[]) {
            TurnEndAction::Reflect { level } => assert_eq!(level, 0),
            other => panic!("expected Reflect, got {other:?}"),
        }
        // A realistic TAGGED reflection: only the <observations> content
        // may land in the log — the other sections are not observation
        // material (B1).
        let tagged = "<observations>condensed suffix</observations>"
            .to_owned()
            + "\n<current-task>finish the refactor</current-task>"
            + "\n<suggested-response>report the summary</suggested-response>";
        let result = turn_result(&tagged);
        let mut action = TurnEndAction::Reflect { level: 0 };
        state.commit(&mut store, &mut action, &result).unwrap();
        assert_eq!(action, TurnEndAction::Done);
        assert_eq!(state.record.frozen_prefix, prefix);
        assert_eq!(state.record.generation, 1);
        let suffix = &state.record.active_observations;
        assert!(suffix.contains("condensed suffix"), "{suffix:?}");
        assert!(!suffix.contains("<observations>"), "{suffix:?}");
        assert!(!suffix.contains("<current-task>"), "{suffix:?}");
        assert!(!suffix.contains("<suggested-response>"), "{suffix:?}");
        assert!(!suffix.contains("finish the refactor"), "{suffix:?}");
        assert!(!suffix.contains("report the summary"), "{suffix:?}");
        assert_eq!(
            state.record.live_observations(),
            format!("{}{}", prefix, state.record.active_observations)
        );
    }

    #[test]
    fn commit_escalates_a_rejected_reflection_level() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 1, 100);
        let mut state = OmState::from_config(
            &crate::config::Om::default(),
            OmRecord {
                active_observations: "y".repeat(400),
                ..Default::default()
            },
        );
        let result = turn_result(&"z".repeat(800)); // bigger than the source
        let mut action = TurnEndAction::Reflect { level: 0 };
        state.commit(&mut store, &mut action, &result).unwrap();
        match action {
            TurnEndAction::Reflect { level } => assert_eq!(level, 1),
            other => panic!("expected escalated Reflect, got {other:?}"),
        }
        assert_eq!(state.record.generation, 0);
    }

    #[test]
    fn commit_keeps_the_cursor_on_degenerate_observation_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 3, 100);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        // 100 identical 30-char lines (over 2k chars, >50% duplicates) —
        // the ported degenerate detector rejects this shape.
        let body = (0..100)
            .map(|_| "the same observation line")
            .collect::<Vec<_>>()
            .join("\n");
        let result = turn_result(&format!("<observations>{body}</observations>"));
        let mut action = TurnEndAction::Observe {
            transcript: String::new(),
        };
        state.commit(&mut store, &mut action, &result).unwrap();
        assert_eq!(action, TurnEndAction::Done);
        assert!(state.record.cursor.is_none());
        assert!(state.record.active_observations.is_empty());
    }

    #[test]
    fn the_overflow_ladder_demotes_the_prefix_and_readmits_it() {
        let mut state = OmState {
            config: OmConfig {
                reflect_threshold: 100,
                ..Default::default()
            },
            record: OmRecord {
                frozen_prefix: "a".repeat(400),       // 100 tokens
                active_observations: "b".repeat(400), // 100 tokens
                ..Default::default()
            },
            buffered: Vec::new(),
            buffer_cursor: None,
            changed: false,
        };
        // 200 combined tokens over the 100 budget: the prefix demotes out of
        // the live context (it stays in the record, recall reaches it).
        state.maintain_prefix_budget();
        assert!(state.record.prefix_demoted);
        assert_eq!(state.record.live_observations(), "b".repeat(400));
        // The suffix falls under the budget: the prefix comes back.
        state.record.active_observations = "b".repeat(40);
        state.record.observation_tokens = 10;
        state.maintain_prefix_budget();
        assert!(!state.record.prefix_demoted);
        assert_eq!(
            state.record.live_observations(),
            format!("{}{}", "a".repeat(400), "b".repeat(40))
        );
    }

    #[test]
    fn the_frozen_prompt_carries_the_marker_and_the_suffix_only() {
        let prompt = crate::om::build_reflector_prompt_frozen("FROZEN", "LIVE SUFFIX", 2);
        assert!(prompt.contains("<frozen-prefix>\nFROZEN\n</frozen-prefix>"));
        assert!(prompt.contains("byte-verbatim"));
        assert!(prompt.contains("LIVE SUFFIX"));
        // The compression guidance for level 2 is appended.
        assert!(prompt.contains(crate::om::COMPRESSION_GUIDANCE[1]));
    }
}
