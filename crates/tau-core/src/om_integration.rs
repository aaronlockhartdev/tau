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
    /// maintained in place). A storage failure is a storage failure — a
    /// corrupted file must not masquerade as fresh OM state.
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
            .and_then(|e| serde_json::from_value(e.payload).ok())
            .unwrap_or_default())
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
/// cursor); a fork owns its copy outright (ADR-0004: a fork is the same
/// session's history) — so it copies the FULL log, including a demoted
/// prefix (which the fork undemotes: in the child it is owned context,
/// reflectable like any of the child's own material), carried as the
/// fork's suffix.
pub fn fork_record(parent: &OmRecord) -> OmRecord {
    OmRecord {
        frozen_prefix: String::new(),
        active_observations: format!("{}{}", parent.frozen_prefix, parent.active_observations),
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
}

impl std::fmt::Display for OmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(e) => write!(f, "session: {e}"),
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
        out.push_str(&format!("[{}] {}\n", entry.kind, text));
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
mod turn_end;
pub use turn_end::*;

#[cfg(test)]
pub(crate) mod tests;
