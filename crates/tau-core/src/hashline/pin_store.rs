//! The persistent anchor pinning store (spec §5.4, #207): the step up from
//! session-scoped, in-memory anchor state to *persistent* state. The
//! in-memory registry (item 2) is gone with the process; this store persists
//! a file's pinned anchor assignments across sessions, so a line's anchor
//! stays stable while its content is unchanged — even when neighbors change
//! or the file is reformatted. That is what stateless `line_hashes` cannot
//! do: its allocation depends on the whole preceding file, so one change
//! churns every anchor after it.
//!
//! **Store technology (see `docs/research/anchor-pin-store.md`).** A plain
//! JSONL sidecar, not SQLite: the workload is per-file KB-range records,
//! whole-record load on every `read`, small writes after every `edit`, no
//! server, and "human-readable files have real value in a dev tool" (the
//! session-format research rule). JSONL matches the session-storage format
//! (`session/entry.rs`) — a CRC32 over each record, an atomic temp-file+
//! rename write for crash-safety, and no new dependency. SQLite would buy
//! durability we do not need at this scale and cost a binary dependency.
//!
//! **Pin identity (the last churn source, #207 §2).** A pin is
//! *content-addressed*: a line's anchor is tied to its line checksum
//! (xxh64 of the canon), not to its position. Re-anchoring after an edit
//! matches the new file's lines against the previous pins **by checksum, in
//! order** (the same LCS-over-checksums the registry uses, item 6): an
//! unchanged line keeps its exact anchor no matter where it moved; a changed
//! line gets a fresh anchor; a deleted line's slot is tombstoned, not
//! recycled. Inserting or deleting a line shifts the *positions* after it,
//! but the surviving pins do so by content, so the shift does not churn them.
//!
//! Placement: `{workspace}/.tau/anchors/{path-hash}.jsonl`, the workspace
//! scope the session store uses (`session.rs`, spec §3). One JSON record per
//! file, each CRC32-guarded.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use xxhash_rust::xxh3::xxh3_64;

/// One pinned anchor: the 4-letter anchor, the checksum of the line it was
/// minted for (the pin's content identity), and a tombstone flag — a freed
/// slot is kept (not recycled) for the session/file lifetime (#207 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub anchor: String,
    pub checksum: String,
    /// The slot was freed (its line deleted) and must not be re-minted.
    #[serde(default)]
    pub tombstone: bool,
}

/// A file's pinned anchor set: one record in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePins {
    /// Path (as the tool layer keys it) the pins belong to.
    pub path: String,
    /// Checksum of the whole file content the pins were computed for; a
    /// mismatch means the file drifted and the pins must re-anchor.
    pub checksum: String,
    pub line_count: usize,
    pub pins: Vec<Pin>,
    /// CRC32 (hex) of the canonical record with this field empty.
    #[serde(
        default,
        serialize_with = "serialize_crc",
        deserialize_with = "deserialize_crc"
    )]
    pub crc: Option<String>,
}

#[allow(clippy::ref_option)] // serde `serialize_with` requires `&T` where T is the field type
fn serialize_crc<S: Serializer>(crc: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crc.clone().unwrap_or_default())
}

fn deserialize_crc<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = String::deserialize(d)?;
    Ok(if v.is_empty() { None } else { Some(v) })
}

impl FilePins {
    /// The canonical record (the JSON line as stored) with the `crc` field
    /// populated — sign the record, then verify on load.
    fn signed_line(&self) -> String {
        let mut clone = self.clone();
        clone.crc = None;
        let unsigned = serde_json::to_string(&clone).expect("FilePins is always serializable");
        clone.crc = Some(crc32_hex(unsigned.as_bytes()));
        serde_json::to_string(&clone).expect("FilePins is always serializable")
    }
    /// `true` if the record's embedded CRC matches its canonical body — the
    /// load-time integrity check (a corrupt record is dropped, not trusted).
    fn verified(&self) -> bool {
        let mut clone = self.clone();
        let stored = clone.crc.clone();
        clone.crc = None;
        let unsigned = serde_json::to_string(&clone).expect("FilePins is always serializable");
        stored.as_deref() == Some(crc32_hex(unsigned.as_bytes()).as_str())
    }

    /// The per-line checksums in file order — the re-anchor base.
    #[must_use]
    pub fn checksums(&self) -> Vec<String> {
        self.pins.iter().map(|p| p.checksum.clone()).collect()
    }

    /// The per-line anchors in file order.
    #[must_use]
    pub fn anchors(&self) -> Vec<String> {
        self.pins.iter().map(|p| p.anchor.clone()).collect()
    }
}

/// A pin-store error: the store is best-effort, so callers degrade to the
/// in-memory registry path (fresh allocation) rather than failing the tool.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StoreError {}

/// The persistent pin store: one sidecar per workspace, one record per file.
#[derive(Debug)]
pub struct PinStore {
    root: PathBuf,
}

impl PinStore {
    /// Open (creating) the store rooted at `root` (the workspace's `.tau`
    /// dir; the store uses its `anchors/` subdir).
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.join("anchors"),
        }
    }

    /// The store file for `path`: a hash-named sidecar (the raw path is not
    /// a safe file name).
    fn file_path(&self, path: &str) -> PathBuf {
        let h = format!("{:16x}", xxh3_64(path.as_bytes()));
        self.root.join(format!("{h}.jsonl"))
    }

    fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root)
    }

    /// Load `path`'s pins, verifying the record's CRC. `None` if the file
    /// has no record (first contact → the caller mints fresh) or the record
    /// is corrupt (dropped, the caller re-mints). A missing record is not an
    /// error.
    #[must_use]
    pub fn load(&self, path: &str) -> Option<FilePins> {
        let bytes = fs::read(self.file_path(path)).ok()?;
        let text = std::str::from_utf8(&bytes).ok()?;
        let record: FilePins = serde_json::from_str(text.trim()).ok()?;
        (record.path == path && record.verified()).then_some(record)
    }

    /// Persist `pins` for `path`: CRC32-sign the record, then atomic-write
    /// (temp file + rename) so a crash mid-write never leaves a torn record.
    pub fn save(&self, pins: &FilePins) -> Result<(), StoreError> {
        self.ensure_dir().map_err(|e| StoreError(e.to_string()))?;
        let record = pins.signed_line();
        atomic_write(&self.file_path(&pins.path), record.as_bytes())
            .map_err(|e| StoreError(e.to_string()))
    }

    /// Drop `path`'s record (a `write` voids every anchor).
    pub fn remove(&self, path: &str) {
        let _ = fs::remove_file(self.file_path(path));
    }

    /// Every path this store has a sidecar for (for tests / tooling).
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.root) {
            for e in rd.flatten() {
                if let Some(name) = e.file_name().to_str()
                    && let Some(stem) = name.strip_suffix(".jsonl")
                {
                    out.push(stem.to_owned());
                }
            }
        }
        out.sort();
        out
    }
}

/// CRC32 (IEEE, reflected) of `data`, hex — the same integrity primitive the
/// session store uses (`session/entry.rs`); re-implemented here so the store
/// module is self-contained.
pub(crate) fn crc32_hex(data: &[u8]) -> String {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC_TABLE[usize::try_from((crc ^ u32::from(b)) & 0xFF).expect("masked to 8 bits")]
            ^ (crc >> 8);
    }
    format!("{:08x}", !crc)
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
        // `try_from` is not const-stable; the loop bounds i to < 256.
        let mut c = i as u32;
        let mut n = 0;
        while n < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            n += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = crc_table();

/// The whole-file checksum a pin set is computed for: xxh3-64 over the
/// per-line checksums in order (content-addressed, position-sensitive — any
/// line change moves it).
#[must_use]
pub fn file_checksum(checksums: &[String]) -> String {
    let joined = checksums.join("\n");
    format!("{:16x}", xxh3_64(joined.as_bytes()))
}

/// Atomic write: unique temp file in the target dir + fsync + rename, so a
/// crash mid-write never leaves a torn store record (the session store's
/// `writeAtomic` scheme, spec §5.4).
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = dir.join(format!(".tmp-{}-{nanos}", std::process::id()));
    let result = (|| {
        fs::write(&temp, bytes)?;
        // fsync the temp before the rename so the bytes are durable.
        let f = fs::OpenOptions::new().write(true).open(&temp)?;
        f.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Re-anchor a new file's lines against a previous pin set: a line whose
/// checksum matches a previous pin **in order** keeps that pin's anchor
/// (content-addressed, position-independent); every other line is marked for
/// a fresh mint. Returns `(matched, fresh)` index sets over the new lines —
/// the caller does the minting (it owns the global anchor space).
#[must_use]
pub fn reanchor(prev: &FilePins, checksums: &[String]) -> (Vec<bool>, Vec<bool>) {
    use crate::hashline::registry::match_anchors;
    let prev_sums = prev.checksums();
    let matched_idx = match_anchors(&prev_sums, checksums);
    let mut matched = vec![false; checksums.len()];
    let mut fresh = vec![false; checksums.len()];
    for (j, m) in matched_idx.iter().enumerate() {
        match m {
            Some(_) => matched[j] = true,
            None => fresh[j] = true,
        }
    }
    (matched, fresh)
}

/// Build the next `FilePins` from a previous set and the re-anchor result:
/// survivors keep their anchor, fresh lines get `minted[j]`, and a pin whose
/// checksum vanished is tombstoned (kept, not recycled). `minted` must be
/// index-aligned with `checksums` (the caller mints for every line; matched
/// lines ignore their `minted` value and keep the survivor's anchor).
#[must_use]
pub fn next_pins(
    prev: &FilePins,
    matched: &[bool],
    minted: &[String],
    checksums: &[String],
) -> FilePins {
    // Map previous pins to their (old) line index by checksum; a survivor at
    // new index j reuses the pin it matched.
    let prev_sums = prev.checksums();
    let mut by_sum: HashMap<&str, (usize, &Pin)> = HashMap::new();
    for (i, s) in prev_sums.iter().enumerate() {
        by_sum.entry(s.as_str()).or_insert((i, &prev.pins[i]));
    }
    let mut pins = Vec::with_capacity(checksums.len());
    let mut used_prev: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (j, sum) in checksums.iter().enumerate() {
        let pin = if matched[j] {
            let (i, p) = by_sum
                .get(sum.as_str())
                .expect("a match implies a previous pin with this checksum");
            used_prev.insert(*i);
            Pin {
                anchor: p.anchor.clone(),
                checksum: sum.clone(),
                tombstone: false,
            }
        } else {
            Pin {
                anchor: minted[j].clone(),
                checksum: sum.clone(),
                tombstone: false,
            }
        };
        pins.push(pin);
    }
    // Tombstone the pins whose checksum is gone (their line was deleted):
    // kept in the set, flagged, and never re-minted.
    for (i, p) in prev.pins.iter().enumerate() {
        if !used_prev.contains(&i) && !p.tombstone {
            pins.push(Pin {
                anchor: p.anchor.clone(),
                checksum: p.checksum.clone(),
                tombstone: true,
            });
        }
    }
    FilePins {
        path: prev.path.clone(),
        checksum: file_checksum(checksums),
        line_count: checksums.len(),
        pins,
        crc: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> PinStore {
        PinStore::new(dir)
    }

    fn pins(path: &str, anchors: &[&str], sums: &[&str]) -> FilePins {
        FilePins {
            path: path.into(),
            checksum: file_checksum(&sums.iter().map(ToString::to_string).collect::<Vec<_>>()),
            line_count: sums.len(),
            pins: anchors
                .iter()
                .zip(sums.iter())
                .map(|(a, s)| Pin {
                    anchor: a.to_string(),
                    checksum: s.to_string(),
                    tombstone: false,
                })
                .collect(),
            crc: None,
        }
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("pin-rt-{}", std::process::id()));
        let s = store(&dir);
        let fp = pins("a.rs", &["ABCD", "EFGH"], &["c1", "c2"]);
        s.save(&fp).expect("save");
        let loaded = s.load("a.rs").expect("load");
        assert_eq!(loaded.path, "a.rs");
        assert_eq!(loaded.line_count, 2);
        assert_eq!(loaded.pins, fp.pins);
        assert_eq!(loaded.checksum, fp.checksum);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_is_none_not_err() {
        let dir = std::env::temp_dir().join(format!("pin-m-{}", std::process::id()));
        let s = store(&dir);
        assert!(s.load("never-seen.rs").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_record_is_dropped_not_trusted() {
        let dir = std::env::temp_dir().join(format!("pin-c-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("anchors")).expect("mkdir");
        // A record whose body was tampered with after signing: its embedded
        // CRC no longer matches, so load must drop it.
        let s = store(&dir);
        let fp = pins("victim.rs", &["ABCD"], &["c1"]);
        s.save(&fp).expect("save");
        let file = s.file_path("victim.rs");
        let body = fs::read_to_string(&file).expect("read");
        // Flip a pin's checksum out-of-band to break the CRC.
        let tampered = body.replace("\"c1\"", "\"EVIL\"");
        fs::write(&file, tampered).expect("write");
        assert!(s.load("victim.rs").is_none(), "corrupt record is dropped");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reanchor_matches_by_checksum_in_order() {
        let prev = pins("a", &["A", "B", "C"], &["s1", "s2", "s3"]);
        // New file: s4 at the front, then s1 and s2 in their original relative
        // order (s3 deleted). Re-anchoring is an in-order LCS: s1 and s2 keep
        // their pins, s4 is fresh, s3's slot is tombstoned downstream.
        let new = vec!["s4".to_owned(), "s1".to_owned(), "s2".to_owned()];
        let (matched, fresh) = reanchor(&prev, &new);
        assert!(fresh[0], "s4 is new");
        assert!(matched[1], "s1 keeps its pin");
        assert!(matched[2], "s2 keeps its pin");
        assert_eq!(fresh.iter().filter(|b| **b).count(), 1);
    }

    #[test]
    fn reanchor_reverse_order_is_not_matched() {
        // A three-way reversal: any *pair* of the survivors is in the opposite
        // relative order, so the in-order LCS matches only one of the three and
        // re-mints the other two. (A two-element reversal has LCS 1 — a single
        // element is trivially in-order — so the negative needs three.)
        let prev = pins("a", &["A", "B", "C"], &["s1", "s2", "s3"]);
        let new = vec!["s3".to_owned(), "s2".to_owned(), "s1".to_owned()];
        let (matched, fresh) = reanchor(&prev, &new);
        assert_eq!(
            matched.iter().filter(|b| **b).count(),
            1,
            "a full reversal matches only one survivor"
        );
        assert_eq!(fresh.iter().filter(|b| **b).count(), 2);
    }

    #[test]
    fn next_pins_tombstones_deleted_and_keeps_survivors() {
        let prev = pins("a", &["A", "B"], &["s1", "s2"]);
        // Delete s2 (anchor B), keep s1, add s3.
        let new = vec!["s1".to_owned(), "s3".to_owned()];
        let (matched, _fresh) = reanchor(&prev, &new);
        let minted = vec!["A".to_owned(), "ZZZZ".to_owned()];
        let next = next_pins(&prev, &matched, &minted, &new);
        // s1 survives with anchor A; s3 is minted ZZZZ; s2's pin B is
        // tombstoned (present, flagged), not recycled.
        let live: Vec<&Pin> = next.pins.iter().filter(|p| !p.tombstone).collect();
        assert_eq!(live.len(), 2);
        assert!(live.iter().any(|p| p.anchor == "A" && p.checksum == "s1"));
        assert!(
            live.iter()
                .any(|p| p.anchor == "ZZZZ" && p.checksum == "s3")
        );
        let tomb: Vec<&Pin> = next.pins.iter().filter(|p| p.tombstone).collect();
        assert_eq!(tomb.len(), 1);
        assert_eq!(tomb[0].anchor, "B", "the deleted line's slot is tombstoned");
    }

    #[test]
    fn file_checksum_is_content_and_order_sensitive() {
        let a = file_checksum(&["x".into(), "y".into()]);
        let same = file_checksum(&["x".into(), "y".into()]);
        let reordered = file_checksum(&["y".into(), "x".into()]);
        let changed = file_checksum(&["x".into(), "z".into()]);
        assert_eq!(a, same);
        assert_ne!(a, reordered);
        assert_ne!(a, changed);
    }
}
