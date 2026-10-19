//! The per-session anchor registry: the owner of which anchors this session
//! minted, where they point, and which of them it *showed* the model.
//! In-memory for the session's lifetime (a new ADR; the pinning store that
//! persists it is the next ticket). The LRU clock bounds the anchor pool so a
//! read past the 7,311,616-line cap reclaims the coldest file's anchors with
//! a warning, instead of a hard failure.
//!
//! The registry is the *single source of truth* for a file's anchors: `read`
//! shows the model the file's allocated anchors, and `edit` resolves against
//! them (never a re-derived `line_hashes` — the two would disagree). A re-read
//! re-anchors by LCS over per-line checksums, so unchanged lines keep their
//! exact anchors across reads and edits (item 6).

use std::collections::{HashMap, VecDeque};

use crate::hashline::pin_store::{self, FilePins, Pin, PinStore};
use crate::hashline::{hash_to_idx, idx_to_hash, line_checksum, split_lines};
/// The warning code the model sees when the registry had to free anchors to
/// make room for a read.
pub const ANCHOR_RECLAIM_WARNING: &str = "[W_ANCHOR_RECLAIMED]";

/// How many reclaimed paths a warning lists before "and N more".
const MAX_RECLAIMED_REPORTED: usize = 10;

/// One owned anchor: which file it points at.
#[derive(Debug, Clone)]
struct OwnedEntry {
    path: String,
}

/// A file's most recent allocation: the anchors (in line order) and the
/// checksum of the line each was minted for. A re-read re-anchors against
/// this.
#[derive(Debug, Clone)]
struct FileState {
    anchors: Vec<String>,
    checksums: Vec<String>,
}

/// The registry state, held behind the session's lock.
pub struct Registry {
    /// Every anchor this session owns, and what it points at.
    owned: HashMap<String, OwnedEntry>,
    /// Per-file allocation state (anchors + checksums), the re-anchor base.
    file_state: HashMap<String, FileState>,
    /// What the model was shown: anchor → line checksum at serve time. The
    /// integrity check (item 1) compares these against the file.
    served: HashMap<String, HashMap<String, String>>,
    /// LRU clock: the order files were last read or edited.
    last_used: HashMap<String, u64>,
    usage_clock: u64,
    /// The probe cursor for allocation (continues across files).
    probe: usize,
    /// Anchors reclaimed most recently, drained by `take_reclaimed`.
    reclaimed: VecDeque<String>,
    /// The persistent pin store: seeds a file's first read with its pinned
    /// anchors (cross-session stability, #207) and persists refreshed pins
    /// after an edit. `None` in tests and the ACP path (no workspace root).
    store: Option<PinStore>,
}

#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("owned", &self.owned.len())
            .field("served_files", &self.served.len())
            .finish()
    }
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            owned: HashMap::new(),
            file_state: HashMap::new(),
            served: HashMap::new(),
            last_used: HashMap::new(),
            usage_clock: 0,
            probe: 0,
            reclaimed: VecDeque::new(),
            store: None,
        }
    }
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry backed by a persistent pin store (cross-session anchor
    /// stability, #207). The store is consulted on a file's first read in the
    /// session and persisted after each edit.
    #[must_use]
    pub fn with_store(store: PinStore) -> Self {
        Self {
            store: Some(store),
            ..Self::default()
        }
    }
    fn touch(&mut self, path: &str) {
        self.usage_clock += 1;
        self.last_used.insert(path.to_owned(), self.usage_clock);
    }

    /// Allocate (or re-anchor) the file's anchor set for a read. A first read
    /// mints fresh anchors; a re-read reuses the file's owned anchors for
    /// unchanged lines (LCS over checksums) and mints replacements.
    ///
    /// Returns `(hashes, reclaim_warning)`; the warning is `Some` when the
    /// pool had to be reclaimed to make room.
    pub fn allocate(
        &mut self,
        path: &str,
        content: &str,
    ) -> Result<(Vec<String>, Option<String>), crate::hashline::TooManyLines> {
        self.touch(path);
        let lines = split_lines(content);
        if lines.len() > crate::hashline::HASH_SPACE {
            return Err(crate::hashline::TooManyLines);
        }
        let checksums: Vec<String> = lines.iter().map(|l| line_checksum(l)).collect();

        let prev = self
            .file_state
            .get(path)
            .map(|fs| (fs.anchors.clone(), fs.checksums.clone()))
            .or_else(|| self.seed_from_store(path, &checksums));
        let matched = match &prev {
            Some((_anchors, checksums_prev)) => match_anchors(checksums_prev, &checksums),
            None => vec![None; lines.len()],
        };

        let mut new_anchors: Vec<String> = Vec::with_capacity(lines.len());
        for m in matched {
            match m {
                Some(j) => new_anchors.push(
                    prev.as_ref()
                        .expect("a match implies a previous allocation")
                        .0[j]
                        .clone(),
                ),
                None => new_anchors.push(self.mint(path)?),
            }
        }

        self.evict_path(path);
        for a in &new_anchors {
            self.owned.insert(
                a.clone(),
                OwnedEntry {
                    path: path.to_owned(),
                },
            );
        }
        self.file_state.insert(
            path.to_owned(),
            FileState {
                anchors: new_anchors.clone(),
                checksums,
            },
        );

        Ok((new_anchors, self.drain_reclaimed_warning()))
    }

    /// Record what the model was shown: the integrity baseline.
    pub fn serve(&mut self, path: &str, rows: &[(String, String)]) {
        self.touch(path);
        let served = self.served.entry(path.to_owned()).or_default();
        for (anchor, checksum) in rows {
            served.insert(anchor.clone(), checksum.clone());
        }
    }

    /// Adopt a set of anchors as owned + served for a file — used to refresh
    /// the baseline in place (a range-stale diagnostic's fresh anchors) so the
    /// model can retry without a re-read.
    pub fn adopt(&mut self, path: &str, rows: &[(String, String)]) {
        self.touch(path);
        for (anchor, _checksum) in rows {
            self.owned.insert(
                anchor.clone(),
                OwnedEntry {
                    path: path.to_owned(),
                },
            );
        }
        let served = self.served.entry(path.to_owned()).or_default();
        for (anchor, checksum) in rows {
            served.insert(anchor.clone(), checksum.clone());
        }
    }

    /// The file's served anchors and their checksums (the integrity check's
    /// baseline); `None` if nothing of this file was ever shown.
    #[must_use]
    pub fn served_map(&self, path: &str) -> Option<&HashMap<String, String>> {
        self.served.get(path)
    }

    /// The file's current anchor set (in line order); `None` if the file has
    /// never been allocated in this session.
    #[must_use]
    pub fn anchors_for(&self, path: &str) -> Option<&[String]> {
        self.file_state.get(path).map(|fs| fs.anchors.as_slice())
    }

    /// Re-read after a successful write: the edited file's anchors are the new
    /// set; the old set is freed (its lines are gone or moved).
    pub fn refresh_owned(&mut self, path: &str, content: &str, hashes: &[String]) {
        self.evict_path(path);
        let checksums: Vec<String> = split_lines(content)
            .iter()
            .map(|l| line_checksum(l))
            .collect();
        for h in hashes {
            self.owned.insert(
                h.clone(),
                OwnedEntry {
                    path: path.to_owned(),
                },
            );
        }
        self.file_state.insert(
            path.to_owned(),
            FileState {
                anchors: hashes.to_vec(),
                checksums: checksums.clone(),
            },
        );
        self.touch(path);
        self.persist_pins(path, hashes, &checksums);
    }

    /// Persist the file's refreshed pins to the store after a successful edit
    /// (#207): a line unchanged since the previous allocation keeps its exact
    /// anchor, a changed/new line takes the freshly served anchor, and a
    /// deleted line's slot is tombstoned (kept, not recycled). No-op without
    /// a store; a load/save failure is best-effort (the in-memory state is the
    /// source of truth for this session).
    fn persist_pins(&mut self, path: &str, hashes: &[String], checksums: &[String]) {
        let Some(store) = self.store.as_mut() else {
            return
        };
        let prev = match store.load(path) {
            Some(p) => p,
            None => {
                // First edit this store has seen: record the current pins.
                let _ = store.save(&FilePins {
                    path: path.to_owned(),
                    checksum: pin_store::file_checksum(checksums),
                    line_count: checksums.len(),
                    pins: hashes
                        .iter()
                        .zip(checksums.iter())
                        .map(|(a, s)| Pin {
                            anchor: a.clone(),
                            checksum: s.clone(),
                            tombstone: false,
                        })
                        .collect(),
                    crc: None,
                });
                return
            }
        };
        let (matched, _fresh) = pin_store::reanchor(&prev, checksums);
        let next = pin_store::next_pins(&prev, &matched, hashes, checksums);
        let _ = store.save(&next);
    }

    /// Free a file's anchors (after a `write`, which voids every anchor): the
    /// in-memory set is evicted and the store's record for the file is dropped
    /// (its pins no longer point at anything).
    pub fn free(&mut self, path: &str) {
        self.evict_path(path);
        if let Some(store) = self.store.as_mut() {
            store.remove(path);
        }
    }

    fn mint(&mut self, protect: &str) -> Result<String, crate::hashline::TooManyLines> {
        loop {
            for step in 1..=crate::hashline::HASH_SPACE {
                let candidate = idx_to_hash((self.probe + step) % crate::hashline::HASH_SPACE);
                if !self.owned.contains_key(&candidate) {
                    self.probe = (self.probe + step) % crate::hashline::HASH_SPACE;
                    return Ok(candidate);
                }
            }
            if self.owned.len() >= crate::hashline::HASH_SPACE - 3
                && self.reclaim(protect).is_some()
            {
                continue;
            }
            return Err(crate::hashline::TooManyLines);
        }
    }

    /// Seed a file's anchors from the persistent pin store on its first read
    /// in this session (#207): the store's pinned anchors are the `prev`
    /// allocation the LCS re-anchor matches against, so a line unchanged since
    /// a previous session keeps its exact anchor. `None` when there is no
    /// store, no record, or a corrupt record (the caller then mints fresh).
    fn seed_from_store(
        &mut self,
        path: &str,
        checksums: &[String],
    ) -> Option<(Vec<String>, Vec<String>)> {
        let prev = self.store.as_ref()?.load(path)?;
        // Reuse only pins whose checksum matches a current line; the caller's
        // LCS decides which in-order. A drifted file (checksum mismatch) seeds
        // nothing — its pins are stale.
        (prev.checksum == pin_store::file_checksum(checksums))
            .then_some((prev.anchors(), prev.checksums()))
    }

    fn reclaim(&mut self, protect: &str) -> Option<String> {
        let victim = self
            .last_used
            .iter()
            .filter(|(path, _)| **path != protect)
            .min_by_key(|(_, t)| *t)
            .map(|(path, _)| path.to_owned())?;
        self.evict_path(&victim);
        self.reclaimed.push_back(victim.clone());
        Some(victim)
    }

    fn evict_path(&mut self, path: &str) {
        self.owned.retain(|_, e| e.path != path);
        self.file_state.remove(path);
        self.served.remove(path);
        self.last_used.remove(path);
    }

    fn drain_reclaimed_warning(&mut self) -> Option<String> {
        let paths: Vec<String> = self.reclaimed.drain(..).collect();
        let unique: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        if unique.is_empty() {
            return None;
        }
        let mut sorted: Vec<&str> = unique.iter().copied().collect();
        sorted.sort_unstable();
        let shown = sorted
            .iter()
            .take(MAX_RECLAIMED_REPORTED)
            .copied()
            .collect::<Vec<_>>();
        let more = sorted.len() - shown.len();
        let list = if more > 0 {
            format!("{} (+{more} more)", shown.join(", "))
        } else {
            shown.join(", ")
        };
        Some(format!(
            "{ANCHOR_RECLAIM_WARNING} The session's anchor quota is nearly exhausted; freed all anchors of {list}, the files least recently read or edited. Read each freed file again before editing it."
        ))
    }

    /// Resolve an anchor to a line in `hashes` (the file's current anchor
    /// set), with the ownership diagnostics a stale-anchor read gets.
    pub fn resolve_offset(
        &self,
        anchor: &str,
        path: &str,
        hashes: &[String],
    ) -> Result<usize, String> {
        let Some(existing) = self.owned.get(anchor) else {
            return Err(format!(
                "[E_STALE_ANCHOR] \"{anchor}\" is not owned in this session. Use a line number, or read again for fresh anchors."
            ));
        };
        if existing.path != path {
            return Err(format!(
                "[E_STALE_ANCHOR] \"{anchor}\" is owned by {}. Use a line number, or read {} to use that anchor.",
                existing.path, path
            ));
        }
        match hash_to_idx(anchor) {
            Some(_) => (0..hashes.len())
                .find(|&i| hashes[i] == anchor)
                .ok_or_else(|| {
                    format!(
                        "[E_STALE_ANCHOR] \"{anchor}\" no longer matches the file at {path}. Use a line number, or read again for fresh anchors."
                    )
                }),
            None => Err(format!(
                "[E_BAD_SHAPE] \"{anchor}\" is not a 4-letter anchor."
            )),
        }
    }
}
/// In-order LCS over line checksums: the indices of old lines that match new
/// lines in order. Uses a standard DP table — a changed line in the middle
/// does not prevent later unchanged lines from being matched. Shared with the
/// pin store (`pin_store::reanchor`) so both re-anchor identically.
pub(crate) fn match_anchors(prev_checksums: &[String], checksums: &[String]) -> Vec<Option<usize>> {
    let n = prev_checksums.len();
    let m = checksums.len();
    if n == 0 || m == 0 {
        return vec![None; m];
    }
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if prev_checksums[i] == checksums[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut matched: Vec<Option<usize>> = vec![None; m];
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if prev_checksums[i] == checksums[j] {
            matched[j] = Some(i);
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashline::line_hashes;

    #[test]
    fn allocate_is_idempotent_for_unchanged_content() {
        let mut reg = Registry::new();
        let (a1, w1) = reg.allocate("a", "one\ntwo\n").expect("allocate");
        assert!(w1.is_none());
        let (a2, w2) = reg.allocate("a", "one\ntwo\n").expect("allocate");
        assert!(w2.is_none());
        assert_eq!(a1, a2, "a re-read of unchanged content reuses the anchors");
        assert_eq!(reg.owned.len(), a1.len(), "no duplicate ownership");
    }

    #[test]
    fn reread_after_change_reuses_unchanged_lines() {
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("a", "one\ntwo\nthree\n").expect("allocate");
        let (b, _) = reg.allocate("a", "one\nTWO\nthree\n").expect("allocate");
        assert_eq!(b[0], a[0], "unchanged line keeps its anchor");
        assert_eq!(b[2], a[2], "unchanged line keeps its anchor");
        assert_ne!(b[1], a[1], "changed line gets a fresh anchor");
    }

    #[test]
    fn serve_records_the_integrity_baseline() {
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("a", "one\ntwo\n").expect("allocate");
        reg.serve(
            "a",
            &[(a[0].clone(), "c1".into()), (a[1].clone(), "c2".into())],
        );
        let m = reg.served_map("a").expect("served");
        assert_eq!(m.get(&a[0]).map(String::as_str), Some("c1"));
        assert_eq!(m.get(&a[1]).map(String::as_str), Some("c2"));
        assert!(reg.served_map("other").is_none());
    }

    #[test]
    fn adopt_adds_owned_and_served() {
        let mut reg = Registry::new();
        reg.adopt("a", &[("AAAA".into(), "c1".into())]);
        assert!(reg.owned.contains_key("AAAA"));
        assert_eq!(
            reg.served_map("a")
                .expect("served")
                .get("AAAA")
                .map(String::as_str),
            Some("c1")
        );
    }

    #[test]
    fn free_drops_ownership_and_served() {
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("a", "one\n").expect("allocate");
        reg.serve("a", &[(a[0].clone(), "c".into())]);
        reg.free("a");
        assert!(reg.owned.is_empty());
        assert!(reg.served_map("a").is_none());
        assert!(reg.anchors_for("a").is_none());
    }

    #[test]
    fn resolve_offset_enforces_ownership() {
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("a", "one\ntwo\n").expect("allocate");
        let hashes = reg.anchors_for("a").expect("anchors").to_vec();
        assert_eq!(reg.resolve_offset(&a[0], "a", &hashes).expect("ok"), 0);
        assert!(
            reg.resolve_offset(&a[0], "b", &hashes)
                .expect_err("cross-file")
                .contains("owned by a")
        );
        assert!(
            reg.resolve_offset("zzzz", "a", &hashes)
                .expect_err("unowned")
                .contains("not owned")
        );
        let changed = line_hashes("one\nX\n").expect("hashes");
        assert!(
            reg.resolve_offset(&a[1], "a", &changed)
                .expect_err("stale")
                .contains("no longer matches")
        );
    }
}
