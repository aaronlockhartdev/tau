//! OM integration (ADR-0004, spec §4): wires the pure `om` module into the
//! running session — the per-session record as appended session entries,
//! turn-end observe/reflect, Phase-2 buffered chunks, context assembly, the
//! `recall` tool, and the compacted-spawn frozen prefix.

use crate::om::{OmConfig, OmRecord};
use crate::session::{Entry, SessionStore};

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
pub struct OmState {
    pub config: OmConfig,
    pub record: OmRecord,
    pub buffered: Vec<BufferedChunk>,
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
        active_path(&entries, leaf.as_deref())
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
fn active_path(entries: &[Entry], leaf_id: Option<&str>) -> Vec<Entry> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::om::Cursor;

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
}
