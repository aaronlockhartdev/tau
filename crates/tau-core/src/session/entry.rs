//! Session entries: the JSONL line type, its CRC32 integrity machinery
//! (spec §3 hardening 1), and session-id minting.

use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write as _;

use super::{Error, SessionStore};

/// Out-of-band sidecar blob (ADR-0005 hardening 2): raw bytes stored
/// zstd-compressed in `blobs/`, referenced from the owning entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobRef {
    /// Blob file name (the owning entry's id).
    pub id: String,
    /// Uncompressed payload size in bytes.
    pub size: u64,
    /// hex xxh3-64 of the uncompressed payload.
    pub hash: String,
}

/// One session entry: one JSONL line (spec §3). `kind` is free-form — the
/// agent-loop and OM tickets define the concrete kinds on top of this storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    /// Previous entry in the branch; `None` for the first entry.
    #[serde(rename = "parentId", default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Epoch milliseconds.
    pub timestamp: u64,
    #[serde(rename = "type")]
    pub kind: String,
    /// Inline payload; `Value::Null` when the data lives in a sidecar blob.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobRef>,
    /// Compaction span reference (spec §3, pi's `firstKeptEntryId` pattern):
    /// the first entry kept after this record's compressed span.
    #[serde(
        default,
        rename = "firstKeptEntryId",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_kept_entry_id: Option<String>,
    /// CRC32 (hex) of the canonical line with this field empty.
    #[serde(
        default,
        serialize_with = "serialize_crc",
        deserialize_with = "deserialize_crc"
    )]
    pub crc: Option<String>,
}

#[allow(clippy::ref_option)] // serde `serialize_with` requires `&T` where T is the field type (`Option<String>`)
fn serialize_crc<S: serde::Serializer>(crc: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crc.clone().unwrap_or_default())
}

fn deserialize_crc<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = String::deserialize(d)?;
    Ok(if v.is_empty() { None } else { Some(v) })
}

/// The CRC32 table (IEEE, reflected): the bitwise algorithm unrolled to
/// one table lookup per byte — bitwise-identical values, no 8-way inner
/// loop (a full-file verify pass is then linear without the per-bit cost).
const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
        // `try_from` is not const-stable; the loop bounds i to < 256
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

pub(super) fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC_TABLE[usize::try_from((crc ^ u32::from(b)) & 0xFF).expect("masked to 8 bits")]
            ^ (crc >> 8);
    }
    !crc
}

impl Entry {
    pub(super) fn canonical_line(&self) -> String {
        serde_json::to_string(self).expect("Entry is always serializable")
    }

    /// The line's CRC (everything but the crc field). `pub(crate)` so the
    /// 80 GB cyclic-chain regression test can build a corrupt file with
    /// valid CRCs.
    pub(crate) fn compute_crc(&self) -> String {
        let mut without = self.clone();
        without.crc = None;
        format!("{:08x}", crc32(without.canonical_line().as_bytes()))
    }

    /// A CRC mismatch means the line was corrupted after the write
    /// (spec §3 hardening 1).
    pub(super) fn verify(&self, line: usize) -> Result<(), Error> {
        let expected = self.compute_crc();
        match &self.crc {
            Some(c) if *c == expected => Ok(()),
            _ => Err(Error::Corrupt {
                line,
                expected,
                actual: self.crc.clone().unwrap_or_default(),
            }),
        }
    }
}

impl SessionStore {
    /// A new session id: 12 hex digits (48 bits) of random. The id only
    /// needs to be unique — the namespace is per machine (the file lives
    /// under the workspace's .tau/sessions/), and the session list sorts on
    /// the created timestamp, not the id. The birthday bound on 2^48 is
    /// negligible at machine-scale session counts.
    #[must_use]
    pub fn new_session_id() -> String {
        let bytes: [u8; 6] = rand::rng().random();
        let mut out = String::new();
        for &b in &bytes {
            let _ = write!(out, "{b:02x}");
        }
        out
    }
}

impl std::str::FromStr for Entry {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}
