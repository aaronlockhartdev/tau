//! The active branch of the log: the leaf's parent chain, root to leaf.
//!
//! Branch semantics belong to the log, not to any consumer of it — the audit
//! projection, the OM window, the subagent inspector and the gate all walk the
//! same chain (spec §4: the cursor is path-scoped).

use super::{Entry, Error, SessionStore};

/// The active branch, root to leaf: the leaf's parent chain walked over the
/// file-order entries.
#[must_use]
pub fn branch(entries: &[Entry], leaf_id: Option<&str>) -> Vec<Entry> {
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

impl SessionStore {
    /// The loaded log's active branch, root to leaf — the one read every
    /// consumer of the chain shares.
    ///
    /// Snapshot consistency: the leaf is resolved BEFORE the entry read. The
    /// walk runs over the entry list from that read, and the leaf's ancestors
    /// are all older entries — an append-only file guarantees they are in the
    /// list. Reading the leaf after the list lets a concurrent writer append a
    /// new leaf between the two reads and strand the walk on an id the list
    /// does not contain.
    pub fn branch(&mut self) -> Result<Vec<Entry>, Error> {
        let leaf = self.leaf_cached()?.map(|e| e.id);
        // The whole log by design (#302): the walk starts at the leaf and
        // needs every loaded entry in reach — the store reads by position, not
        // by parent chain.
        let entries: Vec<Entry> = self
            .entries_range_cached(0, usize::MAX)?
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        Ok(branch(&entries, leaf.as_deref()))
    }
}
