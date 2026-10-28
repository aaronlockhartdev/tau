//! The pin record: one file's anchor slots and the generation math that
//! produces the next set (#207 §2, #310).
//!
//! **Pin identity.** A pin is *content-addressed*: a line's anchor is tied to
//! its line checksum (xxh64 of the canon), not to its position. Re-anchoring
//! after an edit matches the new file's lines against the previous pins **by
//! checksum, in order**: an unchanged line keeps its exact anchor no matter
//! where it moved; a changed line gets a fresh anchor; a deleted line's slot
//! is tombstoned, not recycled. Inserting or deleting a line shifts the
//! *positions* after it, but the surviving pins do so by content, so the
//! shift does not churn them.
//!
//! **The record is bounded (#310).** Every generation appends the slots it
//! freed, and #304 made that claim last as long as the record does, so the
//! retired slots are capped: a record carries at most `MAX_TOMBSTONES` of
//! them, oldest first out. Size is bounded, so a file rewritten a thousand
//! times never carries a million pins; the claim of a dropped slot is not
//! bounded by the record any more, only by the session that freed it.

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

/// The most retired slots one record carries (#310). #304 made a retired
/// slot's claim last as long as the record does, and a file that is edited
/// every day never ages out, so the record is what needs the bound: past the
/// cap the oldest freed slots are dropped and a later session may re-mint
/// them. That is a legal degradation — the store is a cache and correctness
/// does not depend on it (ADR-0033) — and the session that freed them keeps
/// every claim in `Registry::freed`.
///
/// The case the ownership gate does not catch: a line deleted and later
/// re-added with identical content, whose freed slot was dropped, re-mints
/// the same content-addressed anchor in a later session — the integrity
/// check passes, so the aliasing is silent. Retuning the cap changes how
/// often that window opens.
pub const MAX_TOMBSTONES: usize = 256;

/// One pinned anchor: the 4-letter anchor, the checksum of the line it was
/// minted for (the pin's content identity), and a tombstone flag — a freed
/// slot is kept (not recycled) while the record carries it, and a record
/// carries at most `MAX_TOMBSTONES` of them (#207 §2, #310).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub anchor: String,
    pub checksum: String,
    /// The slot was freed (its line deleted); the mint skips it while the
    /// record carries the claim.
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
    /// The JSONL sidecar's CRC32 field; kept so the serde shape is stable
    /// for the in-memory flow — the SQLite row carries no integrity field
    /// (WAL + atomic page writes replace the per-record CRC).
    #[serde(default)]
    pub crc: Option<String>,
}

impl FilePins {
    /// The live slots, in file order. A tombstone is a freed slot: it points
    /// at no line, so it is neither a seed line nor a re-anchor candidate
    /// (#207 §2).
    pub fn live(&self) -> impl Iterator<Item = &Pin> {
        self.pins.iter().filter(|p| !p.tombstone)
    }

    /// The live slots' per-line checksums, in file order — the re-anchor base.
    #[must_use]
    pub fn checksums(&self) -> Vec<String> {
        self.live().map(|p| p.checksum.clone()).collect()
    }

    /// The live slots' anchors, in file order — what a first read seeds from.
    #[must_use]
    pub fn anchors(&self) -> Vec<String> {
        self.live().map(|p| p.anchor.clone()).collect()
    }

    /// The retired slots' anchors: claimed, pointing at nothing. Minting skips
    /// these for as long as the record is durable (#207 §2).
    #[must_use]
    pub fn retired_anchors(&self) -> Vec<String> {
        self.pins
            .iter()
            .filter(|p| p.tombstone)
            .map(|p| p.anchor.clone())
            .collect()
    }

    /// Drops the retired slots past `MAX_TOMBSTONES`, oldest first (#310).
    ///
    /// Precondition: the live slots precede the retired run, and the run is
    /// newest-first — `next_pins` and `retire` both build that shape — so the
    /// tail is the oldest claim. A record that breaks the shape (a hand-built
    /// tombstone among the live slots) drops an arbitrary claim instead.
    pub(super) fn cap_retired(&mut self) {
        let mut kept = 0usize;
        self.pins.retain(|p| {
            if !p.tombstone {
                return true;
            }
            kept += 1;
            kept <= MAX_TOMBSTONES
        });
    }
}

/// The whole-file checksum a pin set is computed for: xxh3-64 over the
/// per-line checksums in order (content-addressed, position-sensitive — any
/// line change moves it).
#[must_use]
pub fn file_checksum(checksums: &[String]) -> String {
    let joined = checksums.join("\n");
    format!("{:16x}", xxh3_64(joined.as_bytes()))
}

/// Re-anchor a new file's lines against a previous pin set: a line whose
/// checksum matches a previous live pin **in order** pairs with that pin (the
/// content-addressed, position-independent identity); every other line is
/// fresh. Returns the in-order LCS pairing — the previous live pin index for
/// each new line, `None` where the line is fresh. The pairing (not a
/// per-checksum lookup) is what keeps repeated lines unique: two new copies
/// of one old line pair with two *distinct* previous pins. Retired slots are
/// not in the base: a line reintroduced with a deleted line's content takes a
/// fresh anchor, not the freed one (#207 §2).
#[must_use]
pub fn reanchor(prev: &FilePins, checksums: &[String]) -> Vec<Option<usize>> {
    use crate::hashline::registry::match_anchors;
    let prev_sums = prev.checksums();
    match_anchors(&prev_sums, checksums)
}

/// Build the next `FilePins` from a previous set and the re-anchor pairing:
/// survivors keep the anchor of their paired previous pin, fresh lines get
/// `minted[j]`, and a live pin the pairing did not reach is tombstoned (kept,
/// not recycled) alongside the slots already retired. `paired` must be
/// index-aligned with `checksums`: `Some(i)` marks new index j as a survivor
/// keeping the anchor of live pin `i`, `None` takes `minted[j]`.
#[must_use]
pub fn next_pins(
    prev: &FilePins,
    paired: &[Option<usize>],
    minted: &[String],
    checksums: &[String],
) -> FilePins {
    let live: Vec<&Pin> = prev.live().collect();
    let mut pins = Vec::with_capacity(checksums.len());
    let mut used_prev: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (j, sum) in checksums.iter().enumerate() {
        let pin = match &paired[j] {
            Some(i) => {
                used_prev.insert(*i);
                Pin {
                    anchor: live[*i].anchor.clone(),
                    checksum: sum.clone(),
                    tombstone: false,
                }
            }
            None => Pin {
                anchor: minted[j].clone(),
                checksum: sum.clone(),
                tombstone: false,
            },
        };
        pins.push(pin);
    }
    // A live slot the pairing did not reach is freed: tombstone it. A slot
    // already retired stays retired — its anchor is claimed for the record's
    // lifetime, not for one generation (#207 §2).
    let mut live_i = 0usize;
    for p in &prev.pins {
        let freed = if p.tombstone {
            true
        } else {
            let vanished = !used_prev.contains(&live_i);
            live_i += 1;
            vanished
        };
        if freed {
            pins.push(Pin {
                anchor: p.anchor.clone(),
                checksum: p.checksum.clone(),
                tombstone: true,
            });
        }
    }
    let mut next = FilePins {
        path: prev.path.clone(),
        checksum: file_checksum(checksums),
        line_count: checksums.len(),
        pins,
        crc: None,
    };
    next.cap_retired();
    next
}
#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
