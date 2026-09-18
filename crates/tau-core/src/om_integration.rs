//! OM integration (ADR-0004, spec §4): wires the pure `om` module into the
//! running session — the per-session record as appended session entries,
//! turn-end observe/reflect, Phase-2 buffered chunks, context assembly, the
//! `recall` tool, and the compacted-spawn frozen prefix.

use crate::om::{self, Cursor, OmConfig, OmRecord};
use crate::provider::TurnProviderRef;
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
struct NoopSink;

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
        let all = store
            .entries_range(0, usize::MAX)
            .map_err(OmError::Session)?;
        let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
        let branch = active_path(&all, leaf.as_deref());
        let Some(cursor) = &self.record.cursor else {
            return Ok(branch);
        };
        let unobserved = match branch.iter().position(|e| e.id == cursor.entry_id) {
            Some(i) => &branch[i + 1..],
            None => &branch[..],
        };
        Ok(unobserved.iter().filter(|e| is_raw(e)).cloned().collect())
    }

    fn pending_tokens(&self, entries: &[Entry]) -> u32 {
        entries
            .iter()
            .map(|e| om::token_count(&entry_text(e)))
            .sum()
    }

    /// The synchronous turn-end Observer (spec §4): observe the whole
    /// unobserved window, wrap it in its provenance group, append it after a
    /// boundary, advance the cursor, persist. Degenerate or empty output is
    /// discarded — the cursor stays where it was.
    pub async fn observe(
        &mut self,
        store: &mut SessionStore,
        provider: &TurnProviderRef,
        model: &str,
    ) -> Result<bool, OmError> {
        let entries = self.unobserved(store)?;
        let (Some(first), Some(last)) = (entries.first(), entries.last()) else {
            return Ok(false);
        };
        let text = transcript(&entries);
        if text.trim().is_empty() {
            return Ok(false);
        }
        let system = om::observer_system_prompt();
        let request = crate::provider::ResponseRequest::new(
            model,
            Some(system.as_str()),
            vec![crate::provider::InputEntry::Message(
                crate::provider::InputMessage {
                    role: "user".into(),
                    content: text,
                },
            )],
        );
        let mut sink = NoopSink;
        let result = provider
            .call(&request, &mut sink)
            .await
            .map_err(OmError::Provider)?;
        let parsed = om::parse_observer_output(&result.text);
        if parsed.degenerate || parsed.observations.trim().is_empty() {
            return Ok(false);
        }
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(&parsed.observations);
        let wrapped = om::wrap_in_observation_group(&parsed.observations, &range, &id, None);
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
        Ok(true)
    }

    /// The Phase-2 buffered chunk (spec §4): the next `buffer_increment` of
    /// the unobserved raw, observed and held until activation. A tail below
    /// one increment is not buffered — the threshold path covers it.
    pub async fn buffer_next(
        &mut self,
        store: &mut SessionStore,
        provider: &TurnProviderRef,
        model: &str,
    ) -> Result<bool, OmError> {
        let entries = self.unobserved(store)?;
        let start = match self.buffered.last() {
            Some(chunk) => entries
                .iter()
                .position(|e| e.id == chunk.range.1)
                .map(|i| i + 1)
                .unwrap_or(entries.len()),
            None => 0,
        };
        let slice = &entries[start..];
        let mut picked: Vec<Entry> = Vec::new();
        let mut tokens = 0u32;
        for entry in slice {
            picked.push((*entry).clone());
            tokens += om::token_count(&entry_text(entry));
            if tokens >= self.config.buffer_increment() {
                break;
            }
        }
        if picked.is_empty() || tokens < self.config.buffer_increment() {
            return Ok(false);
        }
        let system = om::observer_system_prompt();
        let request = crate::provider::ResponseRequest::new(
            model,
            Some(system.as_str()),
            vec![crate::provider::InputEntry::Message(
                crate::provider::InputMessage {
                    role: "user".into(),
                    content: transcript(&picked),
                },
            )],
        );
        let mut sink = NoopSink;
        let result = provider
            .call(&request, &mut sink)
            .await
            .map_err(OmError::Provider)?;
        let parsed = om::parse_observer_output(&result.text);
        if parsed.degenerate || parsed.observations.trim().is_empty() {
            return Ok(false);
        }
        let range = format!("{}:{}", picked[0].id, picked[picked.len() - 1].id);
        let id = om::generate_group_id(&parsed.observations);
        let text = om::wrap_in_observation_group(&parsed.observations, &range, &id, None);
        let tokens = om::token_count(&text);
        self.buffered.push(BufferedChunk {
            range: (picked[0].id.clone(), picked[picked.len() - 1].id.clone()),
            last_ts: picked[picked.len() - 1].timestamp,
            text,
            tokens,
        });
        Ok(true)
    }

    /// Activation (spec §4): the buffered chunks are promoted into the log
    /// and the cursor advances over them — no LLM call.
    pub fn promote(&mut self, store: &mut SessionStore) -> Result<bool, OmError> {
        if self.buffered.is_empty() {
            return Ok(false);
        }
        for chunk in &self.buffered {
            self.record.active_observations =
                om::append_observation(&self.record.active_observations, &now_iso(), &chunk.text);
            self.record.cursor = Some(Cursor {
                entry_id: chunk.range.1.clone(),
                timestamp: chunk.last_ts,
            });
        }
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        self.record.pending_tokens = 0;
        self.buffered.clear();
        self.changed = true;
        self.save(store)?;
        Ok(true)
    }

    fn activation_reached(&self, pending_tokens: u32) -> bool {
        om::should_observe(pending_tokens, self.record.observation_tokens, &self.config)
    }

    /// The Reflector (spec §4): rewrite the managed suffix as a new
    /// generation. The frozen prefix never enters the prompt body and stays
    /// byte-verbatim (ADR-0004); a non-shrinking output escalates through
    /// the compression ladder (levels 0..=4).
    pub async fn reflect(
        &mut self,
        store: &mut SessionStore,
        provider: &TurnProviderRef,
        model: &str,
    ) -> Result<bool, OmError> {
        let source = self.record.reflect_source().to_owned();
        if source.trim().is_empty() {
            return Ok(false);
        }
        let system = om::reflector_system_prompt();
        for level in 0..=4u8 {
            let prompt = if self.record.frozen_prefix.is_empty() {
                om::build_reflector_prompt(&source, level)
            } else {
                om::build_reflector_prompt_frozen(&self.record.frozen_prefix, &source, level)
            };
            let request = crate::provider::ResponseRequest::new(
                model,
                Some(system.as_str()),
                vec![crate::provider::InputEntry::Message(
                    crate::provider::InputMessage {
                        role: "user".into(),
                        content: prompt,
                    },
                )],
            );
            let mut sink = NoopSink;
            let result = provider
                .call(&request, &mut sink)
                .await
                .map_err(OmError::Provider)?;
            let reflected = result.text.trim();
            if reflected.is_empty() || !om::validate_compression(&source, reflected) {
                continue;
            }
            let new_suffix = match om::reconcile_groups_from_reflection(reflected, &source) {
                Some(reconciled) => reconciled,
                None => om::wrap_in_observation_group(
                    reflected,
                    &om::combine_group_ranges(&om::parse_observation_groups(&source)),
                    &om::generate_group_id(reflected),
                    Some("reflection"),
                ),
            };
            self.record.active_observations = new_suffix;
            self.record.generation += 1;
            self.record.observation_tokens = om::token_count(&self.record.active_observations);
            self.changed = true;
            self.save(store)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The overflow ladder (spec §4): after reflection, a combined log still
    /// over budget demotes the frozen prefix to recall-only (it stays in the
    /// session file); it is re-admitted when the managed suffix falls below
    /// the threshold.
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

    /// The turn-end compaction pass (spec §4): an activation with buffered
    /// chunks promotes them first (no LLM call); the Reflector fires at the
    /// observation threshold; the Observer fires synchronously at the
    /// (dynamic) message threshold; below it, the next buffer increment is
    /// observed out-of-band.
    pub async fn turn_end(
        &mut self,
        store: &mut SessionStore,
        provider: &TurnProviderRef,
        model: &str,
    ) -> Result<(), OmError> {
        let pending = self.pending_tokens(&self.unobserved(store)?);
        self.record.pending_tokens = pending;

        if !self.buffered.is_empty() && self.activation_reached(pending) {
            self.promote(store)?;
        }

        if om::should_reflect(self.record.observation_tokens, &self.config)
            && self.reflect(store, provider, model).await?
        {
            self.maintain_prefix_budget();
            self.save(store)?;
        }

        let pending = self.pending_tokens(&self.unobserved(store)?);
        self.record.pending_tokens = pending;
        if self.activation_reached(pending) {
            self.observe(store, provider, model).await?;
        } else if pending >= self.config.buffer_increment() {
            self.buffer_next(store, provider, model).await?;
        }
        Ok(())
    }
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

    /// A canned OM provider: answers every call with the same body and
    /// counts the calls (the no-LLM-call activation proof uses the count).
    struct Canned {
        text: String,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Canned {
        fn new(text: &str) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                text: text.into(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl crate::provider::TurnProvider for Canned {
        fn call<'a>(
            &self,
            _req: &crate::provider::ResponseRequest,
            _sink: &'a mut dyn crate::provider::TurnSink,
        ) -> crate::provider::ProviderTurn<'a> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let text = self.text.clone();
            Box::pin(async move {
                Ok(crate::provider::TurnResult {
                    text,
                    reasoning: String::new(),
                    usage: Some(crate::provider::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        total_tokens: 2,
                        output_tokens_details: None,
                    }),
                    completed: true,
                    calls: Vec::new(),
                    mid_stream_errors: Vec::new(),
                })
            })
        }
    }

    #[tokio::test]
    async fn observe_wraps_the_window_and_advances_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 3, 100);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        let provider: TurnProviderRef =
            Canned::new("<observations>user is setting up a workbench</observations>");

        assert!(state.observe(&mut store, &provider, "m").await.unwrap());
        let cursor = state.record.cursor.clone().unwrap();
        assert_eq!(cursor.entry_id, "00000003");
        assert_eq!(cursor.timestamp, store.entry("00000003").unwrap().timestamp);
        let log = state.record.active_observations.clone();
        let groups = crate::om::parse_observation_groups(&log);
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

    #[tokio::test]
    async fn observe_discards_degenerate_output_and_keeps_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 3, 100);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        // 100 identical 30-char lines (over 2k chars, >50% duplicates) —
        // the ported degenerate detector rejects this shape.
        let body = (0..100)
            .map(|_| "the same observation line")
            .collect::<Vec<_>>()
            .join("\n");
        let provider: TurnProviderRef =
            Canned::new(&format!("<observations>{body}</observations>"));

        assert!(!state.observe(&mut store, &provider, "m").await.unwrap());
        assert!(state.record.cursor.is_none());
        assert!(state.record.active_observations.is_empty());
    }

    #[test]
    fn unobserved_is_cut_at_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 5, 10);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        state.record.cursor = Some(Cursor {
            entry_id: "00000003".into(),
            timestamp: 1,
        });
        let unobserved = state.unobserved(&mut store).unwrap();
        assert_eq!(
            unobserved.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["00000004", "00000005"]
        );
    }

    #[test]
    fn promote_moves_buffered_chunks_into_the_log_without_an_llm_call() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 2, 100);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        state.buffered.push(BufferedChunk {
            range: ("00000001".into(), "00000002".into()),
            last_ts: 9,
            text: crate::om::wrap_in_observation_group(
                "buffered obs",
                "00000001:00000002",
                "a",
                None,
            ),
            tokens: 10,
        });
        let canned = Canned::new("<observations>never</observations>");

        assert!(state.promote(&mut store).unwrap());
        assert_eq!(canned.calls(), 0);
        assert!(state.record.active_observations.contains("buffered obs"));
        let cursor = state.record.cursor.clone().unwrap();
        assert_eq!(cursor.entry_id, "00000002");
        assert_eq!(cursor.timestamp, 9);
        assert!(state.buffered.is_empty());
    }

    #[tokio::test]
    async fn turn_end_observes_at_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        // 100 × 1200 chars = 30k tokens: exactly at the default threshold.
        let mut store = store_with_text_entries(dir.path(), 100, 1200);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        let canned = Canned::new("<observations>done</observations>");
        let provider: TurnProviderRef = canned.clone();

        state.turn_end(&mut store, &provider, "m").await.unwrap();
        assert_eq!(state.record.cursor.unwrap().entry_id, "00000100");
        assert!(state.record.active_observations.contains("done"));
        assert_eq!(canned.calls(), 1);
    }

    #[tokio::test]
    async fn turn_end_buffers_below_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        // 25 × 1000 chars = 25k tokens: below the 30k threshold, past the
        // 6k increment — one buffered chunk, no log change, no record saved.
        let mut store = store_with_text_entries(dir.path(), 25, 1000);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        let canned = Canned::new("<observations>chunk</observations>");
        let provider: TurnProviderRef = canned.clone();

        state.turn_end(&mut store, &provider, "m").await.unwrap();
        assert_eq!(state.buffered.len(), 1);
        assert_eq!(state.buffered[0].range.1, "00000024"); // 24 × 250 = 6k
        assert!(state.record.active_observations.is_empty());
        assert!(state.record.cursor.is_none());
        assert_eq!(canned.calls(), 1);
    }

    #[tokio::test]
    async fn activation_promotes_before_observing_and_makes_no_call_itself() {
        let dir = tempfile::tempdir().unwrap();
        // 90 × 400 tokens = 36k: buffer one 6k chunk (15 entries), then
        // the remaining 30k is observed synchronously at the threshold.
        let mut store = store_with_text_entries(dir.path(), 90, 1600);
        let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
        let canned = Canned::new("<observations>chunk</observations>");
        let provider: TurnProviderRef = canned.clone();

        state.buffer_next(&mut store, &provider, "m").await.unwrap();
        assert_eq!(canned.calls(), 1);

        state.turn_end(&mut store, &provider, "m").await.unwrap();
        assert_eq!(canned.calls(), 2);
        // The promoted chunk and the observed remainder share one log.
        let groups = crate::om::parse_observation_groups(&state.record.active_observations);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].range, "00000000:00000015");
        assert_eq!(groups[1].range, "00000016:00000090");
        assert_eq!(state.record.cursor.unwrap().entry_id, "00000090");
    }

    /// A canned provider that serves a different body per call (the
    /// compression-ladder tests escalate through levels).
    struct CannedSeq {
        bodies: Vec<String>,
        index: std::sync::atomic::AtomicUsize,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl CannedSeq {
        fn new(bodies: Vec<String>) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                bodies,
                index: std::sync::atomic::AtomicUsize::new(0),
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl crate::provider::TurnProvider for CannedSeq {
        fn call<'a>(
            &self,
            _req: &crate::provider::ResponseRequest,
            _sink: &'a mut dyn crate::provider::TurnSink,
        ) -> crate::provider::ProviderTurn<'a> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let i = self.index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let text = self.bodies[i.min(self.bodies.len() - 1)].clone();
            Box::pin(async move {
                Ok(crate::provider::TurnResult {
                    text,
                    reasoning: String::new(),
                    usage: Some(crate::provider::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        total_tokens: 2,
                        output_tokens_details: None,
                    }),
                    completed: true,
                    calls: Vec::new(),
                    mid_stream_errors: Vec::new(),
                })
            })
        }
    }

    #[tokio::test]
    async fn reflect_rewrites_the_suffix_and_keeps_the_prefix_verbatim() {
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
                cursor: None,
                generation: 0,
                observation_tokens: 0,
                pending_tokens: 0,
                prefix_demoted: false,
            },
        );
        let canned = CannedSeq::new(vec!["condensed suffix".to_owned()]);
        let provider: TurnProviderRef = canned.clone();

        assert!(state.reflect(&mut store, &provider, "m").await.unwrap());
        assert_eq!(state.record.frozen_prefix, prefix);
        assert_eq!(state.record.generation, 1);
        // The reflected output is re-wrapped as a reflection group; the
        // prefix + wrapped suffix form one continuous log.
        assert!(state.record.active_observations.contains("condensed suffix"));
        assert_eq!(
            state.record.live_observations(),
            format!("{}{}", prefix, state.record.active_observations)
        );
    }

    #[tokio::test]
    async fn reflect_escalates_the_compression_ladder_until_the_output_shrinks() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_with_text_entries(dir.path(), 1, 100);
        let source = "y".repeat(400); // 100 tokens
        let mut state = OmState::from_config(
            &crate::config::Om::default(),
            OmRecord {
                active_observations: source,
                ..Default::default()
            },
        );
        // Level 0 answers larger than the source (rejected), level 1 smaller.
        let canned = CannedSeq::new(vec!["z".repeat(800), "z".repeat(80)]);
        let provider: TurnProviderRef = canned.clone();

        assert!(state.reflect(&mut store, &provider, "m").await.unwrap());
        assert_eq!(canned.calls(), 2);
        assert_eq!(state.record.generation, 1);
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
