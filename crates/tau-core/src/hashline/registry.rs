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

use std::collections::{HashMap, HashSet, VecDeque};

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

#[derive(Default)]
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
    /// Per-file allocation hint: the next collision probe starts where this
    /// file's last mint left off — the registry's mirror of the pure path's
    /// running hint. It lives within a single read (the read's
    /// `evict_path` drops it afterwards); across reads `probe` is the
    /// fallback.
    file_hint: HashMap<String, usize>,
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
        // #239: minting is content-addressed — a line's first anchor is its
        // content-derived base slot (shared with the pure `line_hashes`), so
        // two fresh sessions over identical content agree without the store.
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

        // The anchors minted for this read: the per-allocation collision
        // domain. The registry's global `owned` set is only refreshed when
        // the read commits, so a same-content pair in one read would mint
        // the same anchor twice without it.
        let mut fresh: HashSet<String> = HashSet::new();
        let mut new_anchors: Vec<String> = Vec::with_capacity(lines.len());
        for (i, m) in matched.into_iter().enumerate() {
            if let Some(j) = m {
                let prev_anchor = prev
                    .as_ref()
                    .expect("a match implies a previous allocation")
                    .0[j]
                    .clone();
                // Ownership gate (parity with the reference's
                // `alignOwnershipWithSpans`): a pinned anchor this session
                // already owns for a *different* file is not reused — it
                // would mint a cross-file duplicate. Mint fresh instead.
                if self.owned.get(&prev_anchor).is_some_and(|e| e.path != path) {
                    let anchor = self.mint(path, Some(lines[i].as_str()), &fresh)?;
                    fresh.insert(anchor.clone());
                    new_anchors.push(anchor);
                } else {
                    new_anchors.push(prev_anchor);
                }
            } else {
                let anchor = self.mint(path, Some(lines[i].as_str()), &fresh)?;
                fresh.insert(anchor.clone());
                new_anchors.push(anchor);
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
            return;
        };
        if let Some(prev) = store.load(path) {
            let paired = pin_store::reanchor(&prev, checksums);
            let next = pin_store::next_pins(&prev, &paired, hashes, checksums);
            let _ = store.save(&next);
        } else {
            // First edit this store has seen: record the current pins.
            let _ = store.save(&Self::current_pins(path, hashes, checksums));
        }
    }

    /// The current pins for a file: every line pinned to the anchor it was
    /// just served, no tombstones.
    fn current_pins(path: &str, hashes: &[String], checksums: &[String]) -> FilePins {
        FilePins {
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
        }
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

    /// Mint one free anchor. With a `line` (the usual path) the probe starts
    /// at the line's content-derived base slot and walks forward by stride —
    /// the same walk the pure allocator takes — against the registry's
    /// global `owned` set plus this read's `fresh` mints (uniqueness is
    /// session-wide and per-read, the base is content-local). Without a
    /// `line` it falls back to the file's running hint, then the monotonic
    /// probe cursor (slot 0 stays untouchable).
    fn mint(
        &mut self,
        protect: &str,
        line: Option<&str>,
        fresh: &HashSet<String>,
    ) -> Result<String, crate::hashline::TooManyLines> {
        // The probe start, mirroring the pure allocator: the line's content
        // base when free, the file's running hint when the base is owned
        // (collisions continue where the file left off), else the global
        // cursor (the no-line fallback).
        let taken =
            |this: &Self, anchor: &str| this.owned.contains_key(anchor) || fresh.contains(anchor);
        let start = match line {
            Some(l) => {
                let base = crate::hashline::line_base_idx(l);
                if taken(self, &idx_to_hash(base)) {
                    self.file_hint.get(protect).copied().unwrap_or(self.probe)
                } else {
                    base
                }
            }
            None => self.file_hint.get(protect).copied().unwrap_or(self.probe),
        };
        loop {
            for step in 0..=crate::hashline::HASH_SPACE {
                let idx = (start + step * (crate::hashline::HASH_SPACE - 3))
                    % crate::hashline::HASH_SPACE;
                let candidate = idx_to_hash(idx);
                if !taken(self, &candidate) {
                    let after =
                        (idx + crate::hashline::HASH_SPACE - 3) % crate::hashline::HASH_SPACE;
                    self.probe = after;
                    if line.is_some() {
                        self.file_hint.insert(protect.to_owned(), after);
                    }
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
        self.file_hint.remove(path);
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
            let folded = self.owners_differing_only_by_case(anchor, None);
            return Err(format!(
                "[E_STALE_ANCHOR] \"{anchor}\" is not owned in this session.{} Use a line number, or read again for fresh anchors.",
                case_fold_hint(&folded)
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

    /// The session's owned anchors that differ from `anchor` only in case
    /// (the reference's `ownersDifferingOnlyByCase`): the candidates a
    /// case-mangled anchor gets pointed at. `paths`, when given, restricts
    /// the candidates to those files. Sorted for deterministic output.
    #[must_use]
    pub fn owners_differing_only_by_case(
        &self,
        anchor: &str,
        paths: Option<&HashSet<String>>,
    ) -> Vec<(String, String)> {
        let lower = anchor.to_lowercase();
        let mut out: Vec<(String, String)> = self
            .owned
            .iter()
            .filter(|(owned, _entry)| *owned != anchor && owned.to_lowercase() == lower)
            .filter(|(_, entry)| paths.is_none_or(|p| p.is_empty() || p.contains(&entry.path)))
            .map(|(owned, entry)| (owned.clone(), entry.path.clone()))
            .collect();
        out.sort();
        out
    }

    /// The pre-plan ownership gate for one edit member (parity with the
    /// reference's `resolveEditTarget`): `None` when the file is unknown to
    /// the registry (content-addressed resolution applies) or every anchor
    /// is owned by the target file; the typed error otherwise.
    #[must_use]
    pub fn edit_target_error(
        &self,
        path: &str,
        from: &str,
        to: &str,
    ) -> Option<crate::hashline::EditError> {
        self.anchors_for(path)?;
        let owners = [self.owned.get(from), self.owned.get(to)];
        // Gate order mirrors the reference: an unowned bound shadows the
        // cross-file and owned-elsewhere checks.
        let Some(err) = self.unowned_bound_error(owners, from, to) else {
            return Self::owned_but_wrong_file(owners, path, from, to);
        };
        Some(err)
    }

    /// The gate's later checks, once both bounds are owned: the cross-file
    /// pair, then the pair owned by a file other than the target.
    fn owned_but_wrong_file(
        owners: [Option<&OwnedEntry>; 2],
        path: &str,
        from: &str,
        to: &str,
    ) -> Option<crate::hashline::EditError> {
        use crate::hashline::EditError;
        let mut distinct = Vec::new();
        for o in owners.iter().flatten() {
            if !distinct.contains(&o.path) {
                distinct.push(o.path.clone());
            }
        }
        if distinct.len() > 1 {
            return Some(EditError::CrossFile { files: distinct });
        }
        let owner = owners[0]
            .expect("both bounds owned: unowned_bound_error returned None")
            .path
            .clone();
        if owner != path {
            let mut anchors = Vec::new();
            for a in [from, to] {
                if !anchors.iter().any(|x| x == a) {
                    anchors.push(a.to_owned());
                }
            }
            return Some(EditError::OwnedElsewhere { anchors, owner });
        }
        None
    }

    /// The gate's first check: the first bound the session does not own,
    /// as the typed `NotOwned` error. The case-fold hint is scoped to the
    /// paths the other bounds own; an empty scope means no filter (the
    /// reference's `paths.size > 0`).
    fn unowned_bound_error(
        &self,
        owners: [Option<&OwnedEntry>; 2],
        from: &str,
        to: &str,
    ) -> Option<crate::hashline::EditError> {
        use crate::hashline::EditError;
        let i = (0..2).find(|&i| owners[i].is_none())?;
        let anchor = if i == 0 { from } else { to };
        let known: HashSet<String> = owners.iter().flatten().map(|o| o.path.clone()).collect();
        let folded = self.owners_differing_only_by_case(anchor, Some(&known));
        Some(EditError::NotOwned {
            anchor: anchor.to_owned(),
            case_hint: case_fold_hint(&folded),
        })
    }
}
/// The "Anchors are case-sensitive; …" clause the reference appends to an
/// unowned-anchor diagnostic ("" when there is no case-folded candidate).
fn case_fold_hint(candidates: &[(String, String)]) -> String {
    if candidates.is_empty() {
        return String::new();
    }
    let list = candidates
        .iter()
        .map(|(a, _)| format!("\"{a}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" Anchors are case-sensitive; {list} differs only in case.")
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
    use crate::hashline::{
        EditError, HASH_SPACE, idx_to_hash, line_base_idx, line_checksum, line_hashes,
    };

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

    #[test]
    fn first_read_allocates_content_derived_anchors() {
        // #239: the registry's first allocation for a line is the line's
        // content-derived base slot when free — the same base the pure
        // `line_hashes` takes, so a fresh session's anchors are a function
        // of content, not of allocation history.
        let (a, _) = Registry::new()
            .allocate("f", "one\ntwo\n")
            .expect("allocate");
        assert_eq!(a[0], idx_to_hash(line_base_idx("one")));
        assert_eq!(a[1], idx_to_hash(line_base_idx("two")));
    }

    #[test]
    fn collision_probes_forward_from_the_content_base() {
        // A free base takes the base; the next line's base being owned
        // (identical content) makes the probe continue from the file's
        // running hint — in lockstep with the pure path's allocation.
        let (anchors, _) = Registry::new().allocate("f", "x\nx\n").expect("allocate");
        let base = line_base_idx("x");
        assert_eq!(anchors[0], idx_to_hash(base));
        let stride = HASH_SPACE - 3;
        assert_eq!(anchors[1], idx_to_hash((base + stride) % HASH_SPACE));
        // The registry's allocation matches the pure path exactly.
        assert_eq!(anchors, line_hashes("x\nx\n").expect("hashes"));
    }

    #[test]
    fn two_fresh_registries_over_identical_content_agree() {
        // #239's acceptance criterion: with no pin store, two fresh
        // registries over identical content must allocate identical anchors
        // (the monotonic cursor made this a coin flip).
        let content = "one\ntwo\nthree\n";
        let (a, _) = Registry::new().allocate("f", content).expect("allocate");
        let (b, _) = Registry::new().allocate("f", content).expect("allocate");
        assert_eq!(a, b, "fresh registries over identical content agree");
        assert_eq!(a, line_hashes(content).expect("hashes"));
    }

    #[test]
    fn cross_file_mint_probes_the_global_owned_set() {
        // "two"'s content base is owned by file a (its line took the base
        // slot); file b's identical line must probe forward from the base,
        // not land on it — uniqueness is global, the base is content-local.
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("a", "one\ntwo\n").expect("allocate");
        let (b, _) = reg.allocate("b", "two\n").expect("allocate");
        let base = line_base_idx("two");
        assert_eq!(
            idx_to_hash(base),
            a[1],
            "file a's line owns the content base"
        );
        assert_ne!(b[0], a[1], "the colliding line probes off the base");
        assert!(reg.owned.contains_key(&b[0]));
    }

    #[test]
    fn mint_advances_the_monotonic_cursor_past_the_base() {
        // The no-content fallback path keeps stepping from the probe cursor
        // (slot 0 stays untouchable); a content-based mint advances the
        // cursor past the base so a later fallback mint does not collide.
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("f", "one\n").expect("allocate");
        assert_eq!(a[0], idx_to_hash(line_base_idx("one")));
        let probe_after = (line_base_idx("one") + (HASH_SPACE - 3)) % HASH_SPACE;
        assert_eq!(reg.probe, probe_after, "the cursor advances past the base");
    }

    #[test]
    fn store_seeded_pin_owned_by_another_file_is_reminted() {
        // Parity with the reference's ownership alignment: a pinned anchor
        // that this session already owns for a *different* file is not
        // reused when the file is seeded from the store — reusing it would
        // mint a cross-file duplicate (two files, one anchor).
        use crate::hashline::pin_store::{FilePins, GcPolicy, Pin, PinStore};
        let dir = tempfile::tempdir().unwrap();
        let store = PinStore::new(
            dir.path(),
            GcPolicy {
                retention_days: 0,
                gc_on_start: false,
            },
        );
        let sum = line_checksum("two");
        store
            .save(&FilePins {
                path: "b".into(),
                checksum: crate::hashline::pin_store::file_checksum(std::slice::from_ref(&sum)),
                line_count: 1,
                pins: vec![Pin {
                    anchor: "XyZa".into(),
                    checksum: sum.clone(),
                    tombstone: false,
                }],
                crc: None,
            })
            .expect("save");
        let mut reg = Registry::with_store(store);
        // The session owns "XyZa" for a different file.
        reg.adopt("a", &[("XyZa".into(), "c".into())]);
        let (b, _) = reg.allocate("b", "two\n").expect("allocate");
        assert_ne!(
            b[0], "XyZa",
            "the cross-file collision is re-minted, not reused"
        );
        assert_eq!(
            reg.owned.get("XyZa").expect("owned").path,
            "a",
            "the first file keeps its ownership"
        );
        assert!(reg.owned.contains_key(&b[0]), "the fresh mint is owned");
    }

    #[test]
    fn store_seeded_pin_owned_by_the_same_file_is_reused() {
        // The ownership gate must not clobber the normal re-read case: a
        // pin owned by the file itself is reused.
        use crate::hashline::pin_store::{FilePins, GcPolicy, Pin, PinStore};
        let dir = tempfile::tempdir().unwrap();
        let store = PinStore::new(
            dir.path(),
            GcPolicy {
                retention_days: 0,
                gc_on_start: false,
            },
        );
        let sum = line_checksum("two");
        store
            .save(&FilePins {
                path: "a".into(),
                checksum: crate::hashline::pin_store::file_checksum(std::slice::from_ref(&sum)),
                line_count: 1,
                pins: vec![Pin {
                    anchor: "XyZa".into(),
                    checksum: sum.clone(),
                    tombstone: false,
                }],
                crc: None,
            })
            .expect("save");
        let mut reg = Registry::with_store(store);
        reg.adopt("a", &[("XyZa".into(), "c".into())]);
        let (a, _) = reg.allocate("a", "two\n").expect("allocate");
        assert_eq!(a[0], "XyZa", "the same-file pin is reused");
    }

    #[test]
    fn resolve_offset_hints_on_case_folded_anchors() {
        // The reference's case-folding hint: a case-mangled anchor is
        // pointed at the owned anchor it differs from, not left to guess.
        let mut reg = Registry::new();
        reg.adopt("a", &[("abcd".into(), "c".into())]);
        let hashes = vec!["abcd".into()];
        let err = reg
            .resolve_offset("ABCD", "a", &hashes)
            .expect_err("a case-folded ref is not owned");
        assert!(err.contains("not owned in this session"), "{err}");
        assert!(
            err.contains("Anchors are case-sensitive; \"abcd\" differs only in case."),
            "{err}"
        );
        // No case-folded candidate: no hint.
        let err = reg
            .resolve_offset("WXYZ", "a", &hashes)
            .expect_err("unowned");
        assert!(!err.contains("case-sensitive"), "{err}");
    }

    #[test]
    fn edit_target_error_is_typed_per_failure_mode() {
        let mut reg = Registry::new();
        let (a, _) = reg.allocate("a", "one\ntwo\n").expect("allocate");
        let (b, _) = reg.allocate("b", "three\n").expect("allocate");
        // Happy path: both bounds owned by the target file.
        assert!(reg.edit_target_error("a", &a[0], &a[1]).is_none());
        // Unowned anchor, no case-folded candidate: empty hint.
        match reg.edit_target_error("a", &a[0], "zzzz").expect("gated") {
            EditError::NotOwned {
                ref anchor,
                ref case_hint,
            } => {
                assert_eq!(anchor, "zzzz");
                assert_eq!(case_hint, "");
            }
            other => panic!("expected NotOwned, got {other:?}"),
        }
        // Unowned anchor with a case-folded candidate: the hint names it.
        reg.adopt("a", &[("MnOp".into(), "c".into())]);
        match reg.edit_target_error("a", &a[0], "mnop").expect("gated") {
            EditError::NotOwned {
                ref anchor,
                ref case_hint,
            } => {
                assert_eq!(anchor, "mnop");
                assert!(case_hint.contains("\"MnOp\""), "{case_hint}");
            }
            other => panic!("expected NotOwned, got {other:?}"),
        }
        // A from/to pair owned by two different files: one file per call.
        match reg.edit_target_error("a", &a[0], &b[0]).expect("gated") {
            EditError::CrossFile { ref files } => {
                assert_eq!(files, &vec!["a".to_owned(), "b".to_owned()]);
            }
            other => panic!("expected CrossFile, got {other:?}"),
        }
        // Both bounds owned by one file, but not the target: point at the owner.
        match reg.edit_target_error("b", &a[0], &a[1]).expect("gated") {
            EditError::OwnedElsewhere {
                ref anchors,
                ref owner,
            } => {
                assert_eq!(anchors, &vec![a[0].clone(), a[1].clone()]);
                assert_eq!(owner, "a");
            }
            other => panic!("expected OwnedElsewhere, got {other:?}"),
        }
        // A file the registry has never seen: the gate does not apply
        // (content-addressed resolution proceeds, as before).
        assert!(reg.edit_target_error("unknown", &a[0], &b[0]).is_none());
    }
}
