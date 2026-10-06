//! OM integration (ADR-0004, spec §4): wires the pure `om` module into the
//! running session — the per-session record as appended session entries,
//! turn-end observe/reflect, Phase-2 buffered chunks, context assembly, the
//! `recall` tool, and the compacted-spawn frozen prefix.

use std::fmt::Write as _;

use crate::om::{self, Cursor, OmConfig, OmRecord};
use crate::session::{Entry, SessionStore};
use serde::{Deserialize, Serialize};
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
/// raw range, held until activation promotes it to the log — promotion
/// itself makes no LLM call. Durable in the record (ticket #86 P2): a
/// restart keeps the buffer; an un-promoted range is re-observed only if
/// its chunk was never committed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug)]
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
    #[must_use]
    pub fn from_config(om: &crate::config::Om, record: OmRecord) -> Self {
        let observe = f64::from(
            u32::try_from(om.observe_threshold.max(1))
                .expect("om thresholds are validated to fit a u32 at config load"),
        );
        let activation = (1.0
            - f64::from(
                u32::try_from(om.buffer_increment)
                    .expect("om thresholds are validated to fit a u32 at config load"),
            ) / observe)
            .clamp(0.0, 1.0);
        // The record is the single source of truth for the chunks; the
        // in-memory vec is its mirror (ticket #86 P2).
        let buffered = record.buffered_chunks.clone();
        Self {
            config: OmConfig {
                observe_threshold: u32::try_from(om.observe_threshold)
                    .expect("om thresholds are validated to fit a u32 at config load"),
                reflect_threshold: u32::try_from(om.reflect_threshold)
                    .expect("om thresholds are validated to fit a u32 at config load"),
                buffer_activation: activation,
                share_token_budget: false,
                buffer_tokens: u32::try_from(om.buffer_tokens)
                    .expect("om thresholds are validated to fit a u32 at config load"),
                retries: om.retries.clone(),
            },
            record,
            buffered,
            buffer_cursor: None,
            changed: false,
        }
    }

    /// The current record: the last `om` entry on the active branch,
    /// reconstructed on open (spec §4: the record is appended, not
    /// maintained in place). A newest om entry that cannot be decoded
    /// (corrupt payload, missing sidecar) degrades to an empty record —
    pub fn load_record(store: &mut SessionStore) -> Result<OmRecord, OmError> {
        // Snapshot consistency: resolve the leaf BEFORE the entry read. The
        // walk runs over the entry list from that read, and the leaf's
        // ancestors are all older entries — an append-only file guarantees
        // they are in the list. Reading the leaf after the list lets a
        // concurrent writer append a new leaf between the two reads, which
        // strands the walk on an id the list does not contain.
        let leaf = store.leaf_cached()?.map(|e| e.id);
        let entries: Vec<Entry> = store
            .entries_range_cached(0, usize::MAX)?
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        Ok(branch_entries(&entries, leaf.as_deref())
            .into_iter()
            .rev()
            .find(|e| e.kind == KIND_OM)
            .and_then(|e| record_from_entry(store, &e).ok())
            .unwrap_or_default())
    }
}

/// The record an `om` entry carries: the inline payload, or — when the
/// record crossed the blob threshold (a long observation log) — the decoded
/// sidecar. A blob-backed record must not read back empty: a null payload
/// is a storage shape, not an absence, and the record is the session's OM
/// state (spec §4).
fn record_from_entry(store: &SessionStore, entry: &Entry) -> Result<OmRecord, OmError> {
    if !entry.payload.is_null() {
        return serde_json::from_value(entry.payload.clone())
            .map_err(|e| OmError::Session(crate::session::Error::Other(e.to_string())));
    }
    let Some(blob) = &entry.blob else {
        return Ok(OmRecord::default());
    };
    let bytes = store.resolve_blob(blob)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| OmError::Session(crate::session::Error::Other(e.to_string())))
}

impl OmState {
    /// Persist the record as a new `om` entry (the newest entry wins).
    pub fn save(&self, store: &mut SessionStore) -> Result<(), crate::session::Error> {
        let parent = store.leaf()?.map(|e| e.id);
        let payload = serde_json::to_value(&self.record)
            .map_err(|e| crate::session::Error::Other(e.to_string()))?;
        store
            .append(KIND_OM, payload, parent.as_deref())
            .map(|_| ())
    }

    /// The turn-end write-back merge (ticket #86 P2, D13.2): the pass's
    /// state is the base and its observation fields win; the chunk fields
    /// are unioned in from the live state and the freshest file record
    /// (deduped by range), so a background cycle that committed during the
    /// pass (the bounded join timed out) is not clobbered — and vice
    /// versa. The boundary is the max; the buffer cursor rides the largest
    /// boundary (a cycle sets its cursor and boundary together).
    pub(crate) fn merge_turn_end(&mut self, live: &OmState, fresh: &OmRecord) {
        let mut chunks = self.record.buffered_chunks.clone();
        for src in [&live.record.buffered_chunks, &fresh.buffered_chunks] {
            for c in src {
                if !chunks.iter().any(|k| k.range == c.range) {
                    chunks.push(c.clone());
                }
            }
        }
        let boundary = self
            .record
            .last_buffered_at_tokens
            .max(live.record.last_buffered_at_tokens)
            .max(fresh.last_buffered_at_tokens);
        let mut cursor = self.buffer_cursor.clone();
        if live.record.last_buffered_at_tokens > self.record.last_buffered_at_tokens {
            cursor.clone_from(&live.buffer_cursor);
        }
        if fresh.last_buffered_at_tokens > self.record.last_buffered_at_tokens
            && fresh.last_buffered_at_tokens >= live.record.last_buffered_at_tokens
        {
            cursor = fresh
                .buffered_chunks
                .last()
                .map(|c| c.range.1.clone())
                .or(cursor);
        }
        self.record.buffered_chunks = chunks;
        self.record.last_buffered_at_tokens = boundary;
        self.buffer_cursor = cursor;
        self.buffered = self.record.buffered_chunks.clone();
    }
}

/// The active branch, root to leaf: the leaf's parent chain walked over the
/// file-order entries (the cursor is path-scoped, spec §4).
#[must_use]
pub fn branch_entries(entries: &[Entry], leaf_id: Option<&str>) -> Vec<Entry> {
    let by_id: std::collections::HashMap<&str, &Entry> =
        entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut path = Vec::new();
    let mut cur = leaf_id;
    while let Some(id) = cur {
        // Cycle guard: a legitimate chain visits each entry at most once, so
        // a walk longer than the entry count means a corrupted file (duplicate
        // ids) — stop instead of looping forever (the live-om 80GB regression).
        if path.len() >= entries.len() {
            break;
        }
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
/// cursor); a fork owns its copy outright (ADR-0004: a fork is the same
/// session's history) — so it copies the FULL log, including a demoted
/// prefix (which the fork undemotes: in the child it is owned context,
/// reflectable like any of the child's own material), carried as the
/// fork's suffix.
#[must_use]
pub fn fork_record(parent: &OmRecord) -> OmRecord {
    OmRecord {
        frozen_prefix: String::new(),
        active_observations: format!("{}{}", parent.frozen_prefix, parent.active_observations),
        cursor: parent.cursor.clone(),
        generation: parent.generation,
        observation_tokens: parent.observation_tokens,
        pending_tokens: 0,
        prefix_demoted: false,
        // The fork inherits the parent's un-promoted buffer, but starts its
        // boundary fresh: the parent's absolute boundary would suppress the
        // child's own interval crossings (ticket #86 P2).
        buffered_chunks: parent.buffered_chunks.clone(),
        ..Default::default()
    }
}
/// An OM failure: session storage or the model call (the turn-end pass
/// owns the round-trip, R4).
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
        let _ = writeln!(out, "[{}] {}", entry.kind, text);
    }
    out
}

/// The boundary delimiter's timestamp: epoch millis (no chrono in the
/// dependency surface; the delimiter needs monotonicity, not a calendar).
fn now_ms() -> String {
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

/// Compacted-spawn seeding (ADR-0004, spec §5.1): the parent's observation
/// log verbatim becomes the child's frozen prefix — appended to the child's
/// session file as a `spawn-snapshot` entry carrying the parent's session
/// pointer and the source range the prefix covers, and returned as the
/// child's record (an empty managed suffix). The prefix is never
/// re-observed and never re-reflected: the child's Reflector rewrites only
/// the suffix.
pub fn seed_compacted_child(
    parent: &OmRecord,
    parent_session_id: &str,
    child: &mut SessionStore,
) -> Result<OmRecord, OmError> {
    let log = format!("{}{}", parent.frozen_prefix, parent.active_observations);
    if !log.is_empty() {
        let range = om::combine_group_ranges(&om::parse_observation_groups(&log));
        let payload = tau_protocol::payload::SpawnSnapshotPayload {
            parent_session: parent_session_id.to_owned(),
            range,
            log: log.clone(),
        }
        .to_value();
        let leaf = child.leaf()?.map(|e| e.id);
        child.append(KIND_SPAWN_SNAPSHOT, payload, leaf.as_deref())?;
    }
    Ok(OmRecord {
        frozen_prefix: log,
        ..Default::default()
    })
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
        .map_or(entries.len(), |i| i + 1);
    let mut out = String::new();
    for e in &entries[start_idx..end_idx] {
        let text = entry_text(e);
        let preview: String = text.chars().take(200).collect();
        let _ = writeln!(out, "{} {} {}", e.id, e.kind, preview);
    }
    out
}

/// The idle gap before the latest turn, in seconds: the timestamp distance
/// between that turn's first user entry and the entry before it (spec §4:
/// v0 activates pending buffered chunks on a fixed idle timeout, not on
/// provider-cache TTLs). Zero when the branch starts with the turn's user
/// entry (a fresh session).
#[must_use]
pub fn idle_gap_secs(all: &[Entry], leaf_id: Option<&str>) -> u64 {
    let branch = branch_entries(all, leaf_id);
    // Back over the current turn's model output (assistant + tool entries)
    // to its user run, then to the run's first entry. `om` entries are
    // skipped too: a background buffer cycle can commit one mid-episode,
    // after the turn's last model output (ticket #86 P2).
    let mut i = branch.len();
    while i > 0 && matches!(branch[i - 1].kind.as_str(), "assistant" | "tool" | "om") {
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
mod retry;
mod turn_end;
pub use turn_end::*;
pub(crate) mod background;

#[cfg(test)]
pub(crate) mod tests;
