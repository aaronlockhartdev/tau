//! The raw-window projection (spec §4): the active branch cut at the
//! observation cursor, the §4 leading-tool-run prune, the #334 anchor. Split
//! from `turn_end/mod.rs` at the 500-LOC soft limit (#395) — the settle
//! sequence stays there; this is the window the assembly reads.

use super::super::{Entry, OmError, OmState, SessionStore, is_raw};
use tau_protocol::EntryKind;

/// The raw window: the material a main-turn assembly sends after the OM
/// items. `tail` is the unobserved raw (the live compaction unit); `anchor` is
/// the newest user entry on the branch, present only when `tail` does not
/// already carry it (#334).
#[derive(Debug, Clone, Default)]
pub struct RawWindow {
    /// The leading user anchor — the entry that keeps the request answering a
    /// query. `None` when `tail` carries the newest user entry itself.
    pub anchor: Option<Entry>,
    /// The unobserved raw tail, §4 leading-tool-run cut applied.
    pub tail: Vec<Entry>,
}

/// The active-branch's newest user entry (file order): the live instruction
/// OM must never drop from a request (#334).
fn newest_user(branch: &[Entry]) -> Option<&Entry> {
    branch.iter().rev().find(|e| e.kind == EntryKind::User)
}

pub(in crate::om_integration) fn branch_of(
    store: &mut SessionStore,
) -> Result<Vec<Entry>, OmError> {
    store.branch().map_err(OmError::Session)
}

/// The raw entries strictly after `cursor_id` in `branch` (all of them when the
/// cursor is absent).
pub(in crate::om_integration) fn raw_after(
    branch: &[Entry],
    cursor_id: Option<&str>,
) -> Vec<Entry> {
    unobserved_slice(branch, cursor_id)
        .iter()
        .filter(|e| is_raw(e))
        .cloned()
        .collect()
}

/// The branch's entries past `cursor_id`. A cursor the branch does not carry —
/// a rewind (`Command::SessionBranch`) moves the leaf and never touches
/// `OmState`, so a durable cursor can name the abandoned side — is compared by
/// id: an entry's parent is minted before it, so a branch's ids rise root-to-leaf
/// and the cursor's number is the moment its pass reached. The branch's entries
/// at/behind that number are the shared prefix that pass covered; its entries
/// past it are this branch's own material. Taking the whole branch re-covers the
/// prefix (the volume #392 removed, reappearing on reopen); clamping to the
/// branch tip leaves the new material never observed (#395).
pub(in crate::om_integration) fn unobserved_slice<'b>(
    branch: &'b [Entry],
    cursor_id: Option<&str>,
) -> &'b [Entry] {
    let Some(id) = cursor_id else {
        return branch;
    };
    match branch.iter().position(|e| e.id == id) {
        Some(i) => &branch[i + 1..],
        None => branch_after(branch, id),
    }
}

/// The branch's suffix past a cursor that is not on it. An id that is not a
/// number (a record from a build that minted differently) takes the whole
/// branch: observe everything rather than skip material nothing can name.
fn branch_after<'b>(branch: &'b [Entry], cursor_id: &str) -> &'b [Entry] {
    let Ok(seen) = cursor_id.parse::<u64>() else {
        return branch;
    };
    match branch
        .iter()
        .position(|e| e.id.parse::<u64>().is_ok_and(|n| n > seen))
    {
        Some(i) => &branch[i..],
        None => &branch[branch.len()..],
    }
}

impl OmState {
    /// The raw entries not yet covered by a buffered run: after the buffer
    /// cursor, or the whole branch when no run has happened (the buffer
    /// cursor never lags the observation cursor — promotion advances the
    /// latter to the buffered material's end).
    pub(in crate::om_integration) fn unbuffered(
        &self,
        store: &mut SessionStore,
    ) -> Result<Vec<Entry>, OmError> {
        let branch = branch_of(store)?;
        Ok(raw_after(&branch, self.buffer_cursor.as_deref()))
    }

    /// The raw window over already-read entries (pure — no store
    /// access): the active-branch raw entries after the cursor (the
    /// unobserved tail), bounded by the observe threshold via promotion,
    /// not by a per-assembly floor prune. A leading tool run is cut so
    /// no result is orphaned from its call (spec §4). The newest user entry
    /// leads the window as `anchor` when the tail no longer carries one
    /// (#334): the cursor advances past it freely, the entry never leaves
    /// the request.
    #[must_use]
    pub fn raw_window_from(&self, entries: &[Entry], leaf_id: Option<&str>) -> RawWindow {
        let branch = crate::session::branch(entries, leaf_id);
        let unobserved = unobserved_slice(
            &branch,
            self.record.cursor.as_ref().map(|c| c.entry_id.as_str()),
        );
        let mut tail: Vec<Entry> = unobserved.iter().filter(|e| is_raw(e)).cloned().collect();
        // A window starting at a tool result would orphan it from its call:
        // advance past the leading tool run (spec §4 cut rule). No floor prune:
        // unsummarized raw stays in the window until promotion advances the cursor.
        while let Some(entry) = tail.first() {
            if entry.kind == EntryKind::Tool {
                tail.remove(0);
            } else {
                break;
            }
        }
        // A fallback, never a duplicate: when the tail carries a user entry that
        // entry is the request's query, so nothing is prepended; a tail of only
        // tool calls pulls the branch's newest user entry forward. The cut runs
        // first, so the anchor cannot orphan a tool result, and older user
        // entries stay observed (re-anchoring them would undo the §4 prune).
        let anchor = if tail.iter().any(|e| e.kind == EntryKind::User) {
            None
        } else {
            newest_user(&branch).cloned()
        };
        RawWindow { anchor, tail }
    }
}
