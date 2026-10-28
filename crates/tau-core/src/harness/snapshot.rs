//! Snapshot + paged-read projections: the metadata skeleton, the page read, and the closed-session (file) path.

use super::{
    Arc, Core, CoreUsage, Entry, EntryMeta, EntryStatus, LiveSession, LiveState, OmSnapshot,
    Ordering, Path, ProtocolError, SessionMeta, SessionStore, Snapshot, TurnState, Usage, Value,
    ViewEntry,
};
use tau_protocol::EntryKind;

impl Core {
    pub(crate) fn snapshot(&self, live: &LiveSession) -> Result<Snapshot, ProtocolError> {
        let workspace = self.workspace(&live.meta.lock().workspace)?;
        // The live session's own store serves the snapshot from its
        // in-memory log (verified at open; only it appends to the file):
        // no file re-open per snapshot — one open serves it all.
        // The header's `parent` flag decides whether the badge's tail read
        // runs: a 10k-entry root session must not pay for a projection it
        // never uses (ticket #221). Taken here so the store closure never
        // holds the session-meta lock.
        let child = live.meta.lock().parent.is_some();
        let (projection, leaf) = live
            .agent
            .with_store(|store: &mut SessionStore| {
                // An external writer (the 10k fixture) grows the file out
                // from under the log: one re-open keeps the view current.
                store.refresh_if_grown()?;
                let projection = project(store, child, &mut live.skeleton.lock())?;
                let leaf = store.leaf_cached()?;
                Ok::<_, crate::session::Error>((projection, leaf))
            })
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        let meta = live.meta.lock().clone();
        // The plan pane's data (spec §5.3): the watcher-parsed state;
        // a session with no observed plan renders empty.
        let plan = self
            .plans
            .lock()
            .get(&meta.id)
            .cloned()
            .map(Into::into)
            .unwrap_or_default();
        Ok(Snapshot {
            workspace,
            session: SessionMeta {
                usage: projection.usage,
                ..meta
            },
            entries: projection.entries,
            om: live
                .agent
                .om_state()
                .map(|s| OmSnapshot {
                    observation_tokens: s.record.observation_tokens,
                    pending_tokens: s.record.pending_tokens,
                    reflector_threshold: s.config.reflect_threshold,
                })
                .unwrap_or_default(),
            live: LiveState {
                // The queue's projection (the agent's lane queue is the
                // single source of truth), not a second ledger.
                queue: live.agent.queued_items(),
                turn: if live.turn.load(Ordering::SeqCst) {
                    TurnState::Running
                } else {
                    TurnState::Idle
                },
                gate_failed: live.agent.gate_failed(),
                plan,
                subagents: live_subagents(live),
                own_state: projection.own_state,
            },
            cursor: leaf.map(|e| e.id).unwrap_or_default(),
        })
    }
}

/// The live session's sub-agent list for the snapshot (ticket #221): the
/// supervisor's in-memory registry, projected. When the registry is empty
/// (a fresh restart drops it) the on-disk restore stands in — the child
/// records are the ground truth then, and the restore is a pure
/// projection (it mutates no registry, so a later spawn never conflicts).
fn live_subagents(live: &LiveSession) -> Vec<tau_protocol::SubagentInfo> {
    let Some(sup) = live.agent.subagents() else {
        return vec![];
    };
    // The union (design #250 §6.6): the registry's live handles plus the
    // on-disk children no longer in the registry (reclaimed). A pure
    // registry projection would make a partially-reclaimed fleet's early
    // children vanish from the snapshot until a restart.
    sup.fleet_view()
}
/// What a snapshot takes from a session's log: the metadata skeleton the
/// transcript map needs, the last assistant record's usage, and — for a
/// child — its own badge state.
struct Projection {
    entries: Arc<[EntryMeta]>,
    usage: Option<Usage>,
    own_state: Option<tau_protocol::snapshot::OwnState>,
}

/// The snapshot skeleton between polls (#316). The metadata rows are a
/// function of the log's prefix, and the log only grows: a poll builds rows
/// for the records newer than the last poll and reuses the rest. An idle
/// poll pays nothing; a poll that saw growth pays the one copy that hands
/// the wire an immutable share.
///
/// `count` is the log slots the rows cover, in `entries_range_cached`'s index
/// space, and `load_seq` is the store's load generation: rows from one
/// generation never describe the next file under that session's id.
#[derive(Default)]
pub(crate) struct Skeleton {
    shared: Arc<[EntryMeta]>,
    count: usize,
    load_seq: u64,
    last_assistant: Option<String>,
}

/// The projection, read from the store's loaded log through `cache`.
/// Payloads are read only where a fact lives in one: the last assistant
/// record's usage, and a child's state record from the log's tail.
///
/// `child` is the header's `parent` flag, taken by the caller so this read
/// needs no session-meta lock.
fn project(
    store: &mut SessionStore,
    child: bool,
    cache: &mut Skeleton,
) -> Result<Projection, crate::session::Error> {
    let count = store.entry_count();
    let load_seq = store.load_seq();
    if cache.load_seq != load_seq || cache.count > count {
        // A new load generation (the file was re-opened) or fewer slots than
        // the rows cover: either way the rows describe a different file.
        cache.shared = Arc::from(Vec::new());
        cache.count = 0;
        cache.load_seq = load_seq;
        cache.last_assistant = None;
    }
    if cache.count < count {
        let mut rows: Vec<EntryMeta> = cache.shared.to_vec();
        for (entry, size) in store.entries_range_cached(cache.count, usize::MAX)? {
            if entry.kind == EntryKind::Assistant {
                cache.last_assistant = Some(entry.id.clone());
            }
            rows.push(entry_meta(&entry, size));
        }
        cache.shared = rows.into();
        cache.count = count;
    }
    // The usage of the session's last assistant record; the skeleton records
    // its id as it builds, so no payload is scanned for it.
    let usage = cache
        .last_assistant
        .as_deref()
        .and_then(|id| store.entry_cached(id).ok())
        .and_then(|e| e.payload.get("usage").and_then(usage_of));
    // A child's badge is its last lifecycle record, and ticket #302's tail
    // window is the bound: a state record is appended when the child parks,
    // stops, fails or completes, so at most one interrupted turn's records
    // follow it. Deeper than the window reads as no state record — the
    // pre-#221 fallback, which fails safe.
    let own_state = if child {
        let tail = store.entries_tail_cached(crate::subagent::restore::STATE_TAIL)?;
        crate::subagent::snapshot::own_state_for(&tail)
    } else {
        None
    };
    Ok(Projection {
        entries: cache.shared.clone(),
        usage,
        own_state,
    })
}

impl Core {
    pub(crate) fn entries(
        live: &LiveSession,
        since: Option<String>,
        range: Option<tau_protocol::snapshot::EntryRange>,
    ) -> Result<Vec<ViewEntry>, ProtocolError> {
        // The live session's own store serves the page from its in-memory
        // log: the viewport's recompute storm is a cache hit, not a
        // whole-file re-open per read.
        let entries: Vec<Entry> = live
            .agent
            .with_store(|store| {
                // An external writer grows the file out from under the log:
                // one re-open keeps the view current.
                store.refresh_if_grown()?;
                match (since, range) {
                    (Some(cursor), None) => store.entries_since_cached(&cursor),
                    (None, Some(r)) => store
                        .entries_range_cached(r.start, r.start + r.count)
                        .map(|v| v.into_iter().map(|(e, _)| e).collect()),
                    // (None, None) would be a full dump with payloads — spec §8
                    // has no full-dump command (ADR-0006); one of the two is
                    // required.
                    _ => Err(crate::session::Error::Other(
                        "exactly one of since/range is required".into(),
                    )),
                }
            })
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        Ok(entries.iter().map(super::sessions::entry_to_view).collect())
    }

    /// A closed session is a file: open it read-only and project it the way
    /// a live session is (idle live state, no OM, no queue).
    pub(crate) fn snapshot_from_disk(&self, id: &str) -> Result<Snapshot, ProtocolError> {
        let (workspace, meta) = self
            .owning_session(id)
            .ok_or_else(|| ProtocolError::NotFound {
                what: format!("session {id} is not open"),
            })?;
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // One open serves the projection: the in-memory log the open just
        // filled, not a second whole-file read.
        // A closed session keeps no cache: one open serves one projection.
        let projection = project(&mut store, meta.parent.is_some(), &mut Skeleton::default())
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        let leaf = store.leaf_cached().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // The plan cache drops on close (the file is the record): a closed
        // session's snapshot parses the file fresh.
        let plan = std::fs::read_to_string(crate::plan::path(Path::new(&workspace.cwd), id))
            .ok()
            .map(|raw| crate::plan::parse(&raw))
            .map(tau_protocol::snapshot::Plan::from)
            .unwrap_or_default();
        // The restart restore (ticket #221): a closed session's children
        // and a closed child's own state are projected from the file (the
        // registry is empty and there is no live log to read).
        let cwd = Path::new(&workspace.cwd);
        let subagents = crate::subagent::snapshot::restore_children_from_disk(cwd, id);
        Ok(Snapshot {
            workspace,
            session: SessionMeta {
                usage: projection.usage,
                ..meta
            },
            entries: projection.entries,
            om: OmSnapshot::default(),
            live: LiveState {
                queue: vec![],
                turn: TurnState::Idle,
                gate_failed: false,
                plan,
                subagents,
                own_state: projection.own_state,
            },
            cursor: leaf.map(|e| e.id).unwrap_or_default(),
        })
    }

    /// Paged reads of a closed session's file: same semantics as the live
    /// path (exactly one of since/range).
    pub(crate) fn entries_from_disk(
        &self,
        id: &str,
        since: Option<String>,
        range: Option<tau_protocol::snapshot::EntryRange>,
    ) -> Result<Vec<ViewEntry>, ProtocolError> {
        let cwd = self
            .owning_session(id)
            .map(|(w, _)| w.cwd.clone())
            .ok_or_else(|| ProtocolError::NotFound {
                what: format!("session {id} is not open"),
            })?;
        let mut store = SessionStore::for_workspace(Path::new(&cwd), id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // One open serves the page: the in-memory log, not a second
        // whole-file read per paged read.
        let entries = match (since, range) {
            (Some(cursor), None) => {
                store
                    .entries_since_cached(&cursor)
                    .map_err(|e| ProtocolError::Other {
                        message: e.to_string(),
                    })?
            }
            (None, Some(r)) => store
                .entries_range_cached(r.start, r.start + r.count)
                .map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?
                .into_iter()
                .map(|(e, _)| e)
                .collect(),
            _ => {
                return Err(ProtocolError::Other {
                    message: "exactly one of since/range is required".into(),
                });
            }
        };
        Ok(entries.iter().map(super::sessions::entry_to_view).collect())
    }
}
/// The entry tree's one-line preview: the entry's first text line, cut at
/// 80 chars (the snapshot carries previews, never payloads, spec §8). The
/// kind decides where that text lives; a diagnostic's sentence is composed in
/// the GUI from typed fields, so its preview is the code name (ADR-0034).
pub(crate) fn preview(entry: &Entry) -> String {
    let text = match &entry.kind {
        EntryKind::Diagnostic => entry.payload.get("code").and_then(Value::as_str),
        // A tool's call-phase entry (recorded before dispatch, no output yet)
        // falls back to the tool's name.
        _ => entry
            .payload
            .get("text")
            .or_else(|| entry.payload.get("note"))
            .or_else(|| entry.payload.get("state"))
            .or_else(|| entry.payload.get("output"))
            .or_else(|| entry.payload.get("name"))
            .and_then(Value::as_str),
    }
    .unwrap_or_default();
    let first = text.lines().next().unwrap_or_default();
    let mut out: String = first.chars().take(80).collect();
    if first.len() > 80 {
        out.push('…');
    }
    out
}

/// One file entry's snapshot projection (the metadata the GUI renders
/// before a paged read supplies payloads).
pub(crate) fn entry_meta(e: &Entry, size: u64) -> EntryMeta {
    EntryMeta {
        id: e.id.clone(),
        parent: e.parent.clone(),
        kind: e.kind.clone(),
        timestamp: e.timestamp,
        size,
        preview: preview(e),
        first_kept: e.first_kept_entry_id.clone(),
        status: if e.kind == EntryKind::Assistant
            && e.payload.get("interrupted") == Some(&Value::Bool(true))
        {
            EntryStatus::Interrupted
        } else {
            EntryStatus::Ok
        },
    }
}

pub(crate) fn usage_of(value: &Value) -> Option<Usage> {
    let u = CoreUsage::from_record(value)?;
    Some(Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: u.total_tokens(),
        cached_prompt_tokens: u.cached_input_tokens.unwrap_or(0),
    })
}

/// The model the record names as current: the last `model_changed` on the
/// active branch (a closed session's header carries no model, so the record
/// is the only trace — ADR-0034). Pre-#306 sessions are read through the
/// projection's frozen shim.
pub(crate) fn recorded_model(store: &mut SessionStore) -> Option<String> {
    crate::diagnostics::project_store(store)
        .ok()
        .flatten()
        .and_then(|record| record.model)
}

pub(crate) fn delete_session_files(cwd: &Path, session: &str) {
    // The session file plus its sidecar blobs (blobs are keyed by entry id,
    // which is session-scoped — ADR-0005). The store must be opened before
    // the file goes: it is the only way to enumerate the session's blobs.
    let root = cwd.join(".tau");
    let mut store = SessionStore::for_workspace(cwd, session);
    let blobs = if store.open().is_ok() {
        // The whole log by design (#302): every entry's blob id is a delete
        // target, so no window bounds this walk. The visit borrows each
        // entry, so the walk pays no payload clone (#316).
        let mut blobs: Vec<String> = Vec::new();
        store
            .visit_entries_cached(|e, _| {
                if let Some(blob) = &e.blob {
                    blobs.push(blob.id.clone());
                }
            })
            .ok();
        blobs
    } else {
        Vec::new()
    };
    let _ = std::fs::remove_file(root.join("sessions").join(format!("{session}.jsonl")));
    // Archived sessions live in the archive dir as .zst (ADR-0005); a delete
    // must clear that path too, or an archived session's file survives.
    let _ = std::fs::remove_file(root.join("archive").join(format!("{session}.jsonl.zst")));
    for id in blobs {
        let _ = std::fs::remove_file(root.join("blobs").join(id));
    }
}
