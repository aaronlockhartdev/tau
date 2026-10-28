//! The in-memory log read path (spec §3): a live session's paged reads are
//! served from the store's in-memory log — no file re-open and no per-line
//! CRC re-verify (the entries were verified at `open`, and only this store
//! appends to the file — one writer per session). Paging indexes match the
//! file-backed reads' exactly (empty lines keep their slots). A
//! not-yet-opened store falls back to the file-backed read.

use super::{Entry, Error, SessionStore, fs};

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
        for (i, slot) in (0..self.entries.len()).zip(&self.entries) {
            let Some(entry) = slot else {
                continue;
            };
            if i >= end {
                break;
            }
            if i >= start {
                out.push((
                    entry.clone(),
                    u64::try_from(self.entry_len[i]).expect("entry offset, well under u64::MAX"),
                ));
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
        for slot in &self.entries {
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

    /// Visit the loaded log's entries in file order with their line sizes,
    /// borrowing each entry: a projection that reads an entry's identity and
    /// metadata pays no payload clone (the delete sweep's blob enumeration,
    /// `harness::Core::delete_session_files`).
    /// The index space and the not-yet-opened fallback match
    /// `entries_range_cached`.
    pub fn visit_entries_cached(&self, mut visit: impl FnMut(&Entry, u64)) -> Result<(), Error> {
        if !self.loaded {
            for entry in self.entries_range(0, usize::MAX)? {
                visit(&entry, 0);
            }
            return Ok(());
        }
        for (i, slot) in self.entries.iter().enumerate() {
            if let Some(entry) = slot {
                visit(
                    entry,
                    u64::try_from(self.entry_len[i]).expect("entry offset, well under u64::MAX"),
                );
            }
        }
        Ok(())
    }

    /// The in-memory twin of `entries_tail`, oldest first. The caveat ticket
    /// #302 names for a tail read applies here too: it is file order, not
    /// branch order, so it is safe only where the record it seeks is the
    /// file's tail, not a branch's.
    pub fn entries_tail_cached(&self, n: usize) -> Result<Vec<Entry>, Error> {
        if !self.loaded {
            return self.entries_tail(n);
        }
        let mut out: Vec<Entry> = self
            .entries
            .iter()
            .rev()
            .flatten()
            .take(n)
            .cloned()
            .collect();
        out.reverse();
        Ok(out)
    }

    /// The loaded log's slot count, the same index space as
    /// `entries_range_cached`: a bounded read starts after it (the snapshot
    /// skeleton's cursor, #316). A not-yet-opened store reports 0, so its
    /// caller reads from the file's start.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// The store's load generation, bumped by every `open`. A cache of rows
    /// built from the loaded log keys on it, so rows from one load never
    /// describe the next file under the same session id (#316).
    #[must_use]
    pub fn load_seq(&self) -> u64 {
        self.load_seq
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
