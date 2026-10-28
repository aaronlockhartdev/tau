//! Shared fixtures for the pin store's tests: a store over a temp workspace,
//! a GC policy, and a hand-built record.

use super::{FilePins, GcPolicy, Pin, PinStore, file_checksum};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn store(dir: &Path, policy: GcPolicy) -> PinStore {
    PinStore::new(dir, policy)
}

pub(super) fn no_gc() -> GcPolicy {
    GcPolicy {
        retention_days: 0,
        gc_on_start: false,
    }
}

pub(super) fn pins(path: &str, anchors: &[&str], sums: &[&str]) -> FilePins {
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

pub(super) fn ws(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("pin-store-{name}-{}", std::process::id()));
    p
}

pub(super) fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}
