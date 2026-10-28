//! The on-disk ground truth of a parent's sub-agent children (ticket #250):
//! the re-adopt and the fleet's disk half read the child session files —
//! the last `subagent` state entry and the child→parent header link —
//! which survive a restart where the supervisor's in-memory registry does
//! not. The snapshot's `subagents` projection over these records lives in
//! `snapshot.rs`.

use super::states::WaitingOn;
use super::{ChildState, ContextMode};
use crate::session::{Entry, SessionStore, list_workspace};
use tau_protocol::EntryKind;

use std::borrow::Cow;
use std::path::Path;

/// The head window that carries a child's spawn record (#302): a fresh or
/// compacted child writes it as its first entry. A fork child's log is the
/// parent's copied tree, so its record sits at the tail — the tail window is
/// searched as well. Outside both windows the provenance falls back to
/// `general`/`fresh`.
pub(crate) const SPAWN_HEAD: usize = 8;

/// The tail window that carries a child's last state record (#302): every
/// drive opens with a `running` record and closes with a park, stop, fail or
/// completion, so the records after the last state record are at most one
/// interrupted turn's. Outside the window the child reads as parked — the
/// #221 post-restart fallback — and re-adopt drops it.
pub(crate) const STATE_TAIL: usize = 256;

/// One of the parent's children as reconstructed from disk alone (the
/// re-adopt's input, #250): the child's name (the file header's title), the
/// minted handle (the parent's spawn records), the child session id, the
/// spawn's provenance (agent type + context mode), and the child's last
/// recorded lifecycle state. A child with no terminal state is not a
/// re-adopt candidate and is not returned.
#[derive(Debug, Clone)]
pub struct DiskChild {
    pub name: String,
    pub handle: String,
    pub child: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
    pub state: ChildState,
    pub created: u64,
}

/// A `DiskChild`'s protocol projection (the snapshot's disk half, #250):
/// the name and the spawn-record handle are the display/identity the GUI
/// already carries for the child; the terminal state is its last recorded
/// ground truth.
pub fn disk_child_to_info(d: &DiskChild) -> tau_protocol::SubagentInfo {
    let waiting_on = match &d.state {
        ChildState::Idle { waiting_on } => Some(waiting_on.as_str().to_owned()),
        ChildState::Awaiting => Some(WaitingOn::User.as_str().to_owned()),
        _ => None,
    };
    tau_protocol::SubagentInfo {
        handle: d.handle.clone(),
        child: d.child.clone(),
        agent_type: d.agent_type.clone(),
        context_mode: match d.context_mode {
            ContextMode::Fresh => tau_protocol::ContextMode::Fresh,
            ContextMode::Compacted => tau_protocol::ContextMode::Compacted,
            ContextMode::Fork => tau_protocol::ContextMode::Fork,
        },
        state: d.state.kind().to_owned(),
        waiting_on,
        last_message: None,
        usage: None,
        created: d.created,
        resume_contract: None,
    }
}

/// The child's last recorded lifecycle state, over entries already in
/// memory (issue #280): the fleet read opens each child file once and
/// re-uses the entries for both the state check and the spawn provenance.
pub(crate) fn last_state_from(entries: &[Entry]) -> Option<ChildState> {
    entries
        .iter()
        .rev()
        .find(|e| e.kind == EntryKind::Subagent && e.payload["event"] == "state")
        .and_then(|e| serde_json::from_value::<ChildState>(e.payload.clone()).ok())
}

/// The parent's children from disk (the re-adopt's input, #250): every
/// on-disk session whose header names `parent` (and is not archived),
/// carrying its name, minted handle, spawn provenance, and last recorded
/// state — a child whose state is not terminal (or that has none) is
/// dropped: re-adopt is terminal-only (design §5).
pub fn disk_children(cwd: &Path, parent: &str, parent_entries: Option<&[Entry]>) -> Vec<DiskChild> {
    // The parent's spawn records map child session id → minted handle, so a
    // re-adopted child keeps the exact handle the GUI and the parent model
    // already address it by (not a synthesized session-id handle). The
    // open path threads the parent's already-loaded entries through (issue
    // #280); a caller without them (the `subagent_list` command, the
    // re-adopt) re-reads the parent's file.
    let mut handle_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // The threaded entries are borrowed (a `Cow::Borrowed`); a caller
    // without them re-reads the parent's file (a `Cow::Owned`).
    let spawn_records: Cow<'_, [Entry]> = if let Some(entries) = parent_entries {
        Cow::Borrowed(entries)
    } else {
        // The whole parent log by design (#302): every spawn record in the
        // parent's history names a child, and the store has no read narrower
        // than a position range (spec §3).
        let mut pstore = SessionStore::for_workspace(cwd, parent);
        Cow::Owned(
            pstore
                .open()
                .and_then(|()| pstore.entries_range(0, usize::MAX))
                .unwrap_or_default(),
        )
    };
    for e in spawn_records.iter() {
        if e.kind == EntryKind::Subagent
            && e.payload["event"] == "spawn"
            && let (Some(child), Some(handle)) = (
                e.payload.get("child").and_then(|v| v.as_str()),
                e.payload.get("handle").and_then(|v| v.as_str()),
            )
        {
            handle_of.insert(child.to_owned(), handle.to_owned());
        }
    }
    let mut out = Vec::new();
    for m in list_workspace(cwd) {
        if m.archived || m.parent.as_deref() != Some(parent) {
            continue;
        }
        // One open of the child file, two bounded reads: the spawn provenance
        // sits at the log's head, its last state record at the tail (#302 —
        // the fleet read never parses a child's whole file; #280's one open).
        let mut cstore = SessionStore::for_workspace(cwd, &m.id);
        if cstore.open().is_err() {
            continue;
        }
        let Ok(head) = cstore.entries_range(0, SPAWN_HEAD) else {
            continue;
        };
        let Ok(tail) = cstore.entries_tail(STATE_TAIL) else {
            continue;
        };
        let Some(state) = last_state_from(&tail) else {
            continue;
        };
        if !state.is_terminal() {
            continue;
        }
        let (agent_type, context_mode) = spawn_provenance(&head, &tail);
        let name = m.title.clone().unwrap_or_else(|| m.id.clone());
        let handle = handle_of
            .get(&m.id)
            .cloned()
            .unwrap_or_else(|| m.id.clone());
        out.push(DiskChild {
            name,
            handle,
            child: m.id,
            agent_type,
            context_mode,
            state,
            created: m.created,
        });
    }
    out
}

/// A child's spawn provenance (ADR-0001): the agent type and context mode
/// from its spawn record. Absent (a hand-made file) → the
/// `general`/`fresh` defaults.
fn spawn_provenance(head: &[Entry], tail: &[Entry]) -> (String, ContextMode) {
    // Fresh/compacted write the record at the log's head; a fork child's log
    // is the parent's copied tree, so its record sits at the tail (#302).
    let spawn = head
        .iter()
        .find(|e| e.kind == EntryKind::Subagent && e.payload["event"] == "spawn")
        .or_else(|| {
            tail.iter()
                .find(|e| e.kind == EntryKind::Subagent && e.payload["event"] == "spawn")
        });
    let agent_type = spawn
        .and_then(|e| e.payload["type"].as_str())
        .unwrap_or("general")
        .to_owned();
    let context_mode = spawn
        .and_then(|e| e.payload["context_mode"].as_str())
        .and_then(|s| match s {
            "compacted" => Some(ContextMode::Compacted),
            "fork" => Some(ContextMode::Fork),
            _ => None,
        })
        .unwrap_or(ContextMode::Fresh);
    (agent_type, context_mode)
}
