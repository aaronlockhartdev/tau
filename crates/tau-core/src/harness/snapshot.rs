//! Snapshot + paged-read projections: the metadata skeleton, the page read, and the closed-session (file) path.

use super::*;

impl Core {
    pub(crate) fn snapshot(&self, live: &LiveSession) -> Result<Snapshot, ProtocolError> {
        let workspace = self.workspace(&live.meta.lock().unwrap().workspace)?;
        // The live session's own store serves the snapshot from its
        // in-memory log (verified at open; only it appends to the file):
        // no file re-open per snapshot — one open serves it all.
        let (sized, leaf) = live
            .agent
            .with_task_store(|store: &mut SessionStore| {
                // An external writer (the 10k fixture) grows the file out
                // from under the log: one re-open keeps the view current.
                store.refresh_if_grown()?;
                let sized = store.entries_range_cached(0, usize::MAX)?;
                let leaf = store.leaf_cached()?;
                Ok::<_, crate::session::Error>((sized, leaf))
            })
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        let entries: Vec<Entry> = sized.iter().map(|(e, _)| e.clone()).collect();
        let usage = entries
            .iter()
            .rev()
            .find(|e| e.kind == crate::agent::KIND_ASSISTANT)
            .and_then(|e| e.payload.get("usage"))
            .and_then(usage_of);
        let tasks = crate::task::fold_entries(&entries);
        let entries = sized.iter().map(|(e, l)| entry_meta(e, *l)).collect();
        let meta = live.meta.lock().unwrap().clone();
        Ok(Snapshot {
            workspace,
            session: SessionMeta { usage, ..meta },
            entries,
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
                queue: live.queue.lock().unwrap().clone(),
                turn: if live.turn.load(Ordering::SeqCst) {
                    TurnState::Running
                } else {
                    TurnState::Idle
                },
                subagents: live
                    .agent
                    .subagents()
                    .map(|sup| {
                        sup.handles()
                            .iter()
                            .filter_map(|h| sup.state_info(h).map(|i| info_to_protocol(&i)))
                            .collect()
                    })
                    .unwrap_or_default(),
                // The session's tasks (per-session store, spec §5.3): the
                // GUI's tasks panel; active ones ride their resume contract.
                tasks,
            },
            cursor: leaf.map(|e| e.id).unwrap_or_default(),
        })
    }

    pub(crate) fn entries(
        &self,
        live: &LiveSession,
        since: Option<String>,
        range: Option<tau_protocol::snapshot::EntryRange>,
    ) -> Result<Vec<ViewEntry>, ProtocolError> {
        // The live session's own store serves the page from its in-memory
        // log: the viewport's recompute storm is a cache hit, not a
        // whole-file re-open per read.
        let entries: Vec<Entry> = live
            .agent
            .with_task_store(|store| {
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
        Ok(entries
            .iter()
            .map(|e| ViewEntry {
                id: e.id.clone(),
                parent: e.parent.clone(),
                kind: e.kind.clone(),
                timestamp: e.timestamp,
                payload: e.payload.clone(),
                blob: e.blob.as_ref().map(|b| tau_protocol::snapshot::BlobRef {
                    id: b.id.clone(),
                    size: b.size,
                    hash: b.hash.clone(),
                }),
                first_kept: e.first_kept_entry_id.clone(),
            })
            .collect())
    }

    /// A closed session is a file: open it read-only and project it the way
    /// a live session is (idle live state, no OM, no queue).
    pub(crate) fn snapshot_from_disk(&self, id: &str) -> Result<Snapshot, ProtocolError> {
        let (workspace, meta) = self
            .workspaces
            .lock()
            .unwrap()
            .values()
            .find_map(|w| {
                self.session_access(w)
                    .into_iter()
                    .find(|m| m.id == id && !m.archived)
                    .map(|m| (w.clone(), m))
            })
            .ok_or_else(|| ProtocolError::Other {
                message: "unknown session".into(),
            })?;
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // One open serves the projection: the in-memory log the open just
        // filled, not a second whole-file read.
        let sized =
            store
                .entries_range_cached(0, usize::MAX)
                .map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
        let entries: Vec<Entry> = sized.iter().map(|(e, _)| e.clone()).collect();
        let usage = entries
            .iter()
            .rev()
            .find(|e| e.kind == crate::agent::KIND_ASSISTANT)
            .and_then(|e| e.payload.get("usage"))
            .and_then(usage_of);
        let leaf = store.leaf_cached().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        Ok(Snapshot {
            workspace,
            session: SessionMeta { usage, ..meta },
            entries: sized.iter().map(|(e, l)| entry_meta(e, *l)).collect(),
            om: OmSnapshot::default(),
            live: LiveState {
                queue: vec![],
                turn: TurnState::Idle,
                subagents: vec![],
                tasks: crate::task::fold_entries(&entries),
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
            .workspaces
            .lock()
            .unwrap()
            .values()
            .find(|w| {
                self.session_access(w)
                    .iter()
                    .any(|m| m.id == id && !m.archived)
            })
            .map(|w| w.cwd.clone())
            .ok_or_else(|| ProtocolError::Other {
                message: "unknown session".into(),
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
        Ok(entries
            .iter()
            .map(|e| ViewEntry {
                id: e.id.clone(),
                parent: e.parent.clone(),
                kind: e.kind.clone(),
                timestamp: e.timestamp,
                payload: e.payload.clone(),
                blob: e.blob.as_ref().map(|b| tau_protocol::snapshot::BlobRef {
                    id: b.id.clone(),
                    size: b.size,
                    hash: b.hash.clone(),
                }),
                first_kept: e.first_kept_entry_id.clone(),
            })
            .collect())
    }
}
/// The entry tree's one-line preview: the entry's first text line, cut at
/// 80 chars (the snapshot carries previews, never payloads, spec §8).
pub(crate) fn preview(entry: &Entry) -> String {
    let text = entry
        .payload
        .get("text")
        .or_else(|| entry.payload.get("note"))
        .or_else(|| entry.payload.get("state"))
        .or_else(|| entry.payload.get("output"))
        .and_then(Value::as_str)
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
        status: if e.kind == crate::agent::KIND_ASSISTANT
            && e.payload.get("interrupted") == Some(&Value::Bool(true))
        {
            EntryStatus::Interrupted
        } else {
            EntryStatus::Ok
        },
    }
}

pub(crate) fn usage_of(value: &Value) -> Option<Usage> {
    let u: CoreUsage = serde_json::from_value(value.clone()).ok()?;
    Some(Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
        cached_prompt_tokens: u
            .prompt_tokens_details
            .map(|d| d.cached_tokens)
            .unwrap_or(0),
    })
}

/// The quiet entry a model change appends: `model: <old> → <new>` (the
/// first change of a session that had no model records `model: <new>`).
pub(crate) fn model_note(old: &Option<String>, new: &str) -> String {
    match old {
        Some(o) => format!("model: {o} → {new}"),
        None => format!("model: {new}"),
    }
}

/// The session's current model from its file: the last `model:` note on the
/// active branch (a closed session's header carries no model, so the quiet
/// entries are the record).
pub(crate) fn last_model_note(store: &mut SessionStore) -> Option<String> {
    let entries: Vec<Entry> = store
        .entries_range_cached(0, usize::MAX)
        .ok()?
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    let leaf = store.leaf_cached().ok()?.map(|e| e.id);
    crate::om_integration::branch_entries(&entries, leaf.as_deref())
        .iter()
        .rev()
        .find(|e| {
            e.kind == crate::agent::KIND_SYSTEM
                && e.payload
                    .get("note")
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| n.starts_with("model: "))
        })
        .and_then(|e| {
            let n = e.payload.get("note")?.as_str()?;
            let rest = n.strip_prefix("model: ")?;
            Some(rest.rsplit(" → ").next()?.to_owned())
        })
}

pub(crate) fn delete_session_files(cwd: &Path, session: &str) {
    // The session file plus its sidecar blobs (blobs are keyed by entry id,
    // which is session-scoped — ADR-0005). The store must be opened before
    // the file goes: it is the only way to enumerate the session's blobs.
    let root = cwd.join(".tau");
    let mut store = SessionStore::for_workspace(cwd, session);
    let blobs = if store.open().is_ok() {
        store
            .entries_range_cached(0, usize::MAX)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|(e, _)| e)
                    .filter_map(|e| e.blob.map(|b| b.id))
                    .collect()
            })
            .unwrap_or_default()
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
