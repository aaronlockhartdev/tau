//! The in-memory log read path (spec §3): a live session's paged reads are
//! served from the store's in-memory log — no file re-open and no per-line
//! CRC re-verify (the entries were verified at `open`, and only this store
//! appends to the file — one writer per session). Paging indexes match the
//! file-backed reads' exactly (empty lines keep their slots). A
//! not-yet-opened store falls back to the file-backed read.

use super::*;

impl SessionStore {
    /// Re-open when the file has grown past the length the in-memory log
    /// is current to (an external writer — the 10k fixture test appends via
    /// its own store): the log is refilled from the file, so a live read
    /// never serves a stale view. A current file is a `stat`.
    pub fn refresh_if_grown(&mut self) -> Result<(), Error> {
        let len = fs::metadata(self.path())
            .map(|m| m.len())
            .map_err(|e| Error::Other(e.to_string()))?;
        if !self.loaded || len > self.file_len {
            self.open()?;
        }
        Ok(())
    }

    /// A paged read by 0-based entry index, served from the in-memory log
    /// when the store is loaded; the file-backed read otherwise.
    pub fn entries_range_cached(
        &self,
        start: usize,
        end: usize,
    ) -> Result<Vec<(Entry, u64)>, Error> {
        if !self.loaded {
            return self
                .entries_range(start, end)
                .map(|v| v.into_iter().map(|e| (e, 0)).collect());
        }
        let mut out = Vec::new();
        for (i, slot) in self.entries.iter().enumerate() {
            if slot.is_none() {
                continue;
            }
            if i >= end {
                break;
            }
            if i >= start {
                out.push((slot.as_ref().unwrap().clone(), self.entry_len[i] as u64));
            }
        }
        Ok(out)
    }

    /// The in-memory twin of `entries_since` (the live session's paged
    /// reads; see `entries_range_cached`).
    pub fn entries_since_cached(&self, cursor: &str) -> Result<Vec<Entry>, Error> {
        if !self.loaded {
            return self.entries_since(cursor);
        }
        let mut out = Vec::new();
        let mut past = false;
        for slot in self.entries.iter() {
            let Some(entry) = slot else {
                continue;
            };
            if past {
                out.push(entry.clone());
            } else if entry.id == cursor {
                past = true;
            }
        }
        Ok(out)
    }

    /// The in-memory twin of `entry` (the live session's read path).
    pub fn entry_cached(&self, id: &str) -> Result<Entry, Error> {
        if !self.loaded {
            return self.entry(id);
        }
        self.entries
            .iter()
            .flatten()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("no entry {id}")))
    }

    /// The in-memory twin of `leaf` (the live session's read path): the
    /// persisted header choice, else the last entry line in the log.
    pub fn leaf_cached(&mut self) -> Result<Option<Entry>, Error> {
        if !self.loaded {
            return self.leaf();
        }
        if let Some(l) = &self.leaf {
            return self.entry_cached(l).map(Some);
        }
        Ok(self.entries.iter().rev().flatten().next().cloned())
    }

    /// Adopt a branch choice another store just persisted (fork/branch
    /// rewrite the file header with their own store): the live session's
    /// store takes the new leaf in memory, without a second header rewrite.
    pub fn adopt_leaf(&mut self, leaf_id: &str) {
        self.leaf = Some(leaf_id.to_owned());
    }
}
