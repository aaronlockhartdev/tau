use super::*;
use rusqlite::params;

fn store(dir: &Path, policy: GcPolicy) -> PinStore {
    PinStore::new(dir, policy)
}

fn gc(policy: GcPolicy) -> GcPolicy {
    policy
}

fn no_gc() -> GcPolicy {
    GcPolicy {
        retention_days: 0,
        gc_on_start: false,
    }
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

fn ws(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("pin-store-{name}-{}", std::process::id()));
    p
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn save_then_load_roundtrips() {
    let dir = ws("rt");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    let fp = pins("a.rs", &["ABCD", "EFGH"], &["c1", "c2"]);
    s.save(&fp).expect("save");
    let loaded = s.load("a.rs").expect("load");
    assert_eq!(loaded.path, "a.rs");
    assert_eq!(loaded.line_count, 2);
    assert_eq!(loaded.pins, fp.pins);
    assert_eq!(loaded.checksum, fp.checksum);
    // A fresh store on the same file sees the persisted row.
    let again = store(&dir, no_gc());
    assert_eq!(again.load("a.rs").expect("load"), loaded);
    cleanup(&dir);
}

#[test]
fn load_missing_is_none_not_err() {
    let dir = ws("missing");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    assert!(s.load("never-seen.rs").is_none());
    cleanup(&dir);
}

#[test]
fn remove_drops_the_record() {
    let dir = ws("rm");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    s.save(&pins("gone.rs", &["ABCD"], &["c1"])).expect("save");
    s.remove("gone.rs");
    assert!(s.load("gone.rs").is_none());
    assert!(s.paths().is_empty());
    cleanup(&dir);
}

#[test]
fn file_path_is_the_workspace_sqlite_file() {
    let dir = ws("fp");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    assert_eq!(
        s.file_path("anything.rs"),
        dir.join("anchors.sqlite"),
        "the db file is workspace-scoped, not per-path"
    );
    cleanup(&dir);
}

#[test]
fn gc_missing_file_sweep_prunes_rows_for_absent_files() {
    let dir = ws("gc-missing");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    // Both files exist while their rows are written.
    let a = dir.join("a.rs");
    let b = dir.join("b.rs");
    fs::write(&a, "one\n").expect("write a");
    fs::write(&b, "two\n").expect("write b");
    let s = store(&dir, no_gc());
    s.save(&pins(a.to_str().expect("utf8"), &["AAAA"], &["c1"]))
        .expect("save a");
    s.save(&pins(b.to_str().expect("utf8"), &["BBBB"], &["c2"]))
        .expect("save b");
    // b.rs is deleted from disk between sessions; the session-start sweep
    // prunes its row. a.rs exists and survives.
    fs::remove_file(&b).expect("rm b");
    drop(s);
    let s = store(
        &dir,
        gc(GcPolicy {
            retention_days: 0,
            gc_on_start: true,
        }),
    );
    let paths = s.paths();
    assert_eq!(paths, vec![a.to_str().expect("utf8").to_owned()]);
    assert!(
        s.load(a.to_str().expect("utf8")).is_some(),
        "existing file keeps its pins"
    );
    cleanup(&dir);
}

#[test]
fn gc_disabled_does_not_prune() {
    let dir = ws("gc-off");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let b = dir.join("b.rs");
    fs::write(&b, "two\n").expect("write b");
    let s = store(&dir, no_gc());
    s.save(&pins(b.to_str().expect("utf8"), &["BBBB"], &["c2"]))
        .expect("save");
    fs::remove_file(&b).expect("rm b");
    drop(s);
    // gc_on_start = false: the sweep does not run, the row stays.
    let s = store(&dir, no_gc());
    assert_eq!(s.paths(), vec![b.to_str().expect("utf8").to_owned()]);
    cleanup(&dir);
}

#[test]
fn gc_age_expiry_drops_stale_records() {
    let dir = ws("gc-age");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let a = dir.join("a.rs");
    fs::write(&a, "one\n").expect("write a");
    let s = store(&dir, no_gc());
    s.save(&pins(a.to_str().expect("utf8"), &["AAAA"], &["c1"]))
        .expect("save");
    drop(s);
    // Backdate the row three days; a 1-day retention must drop it.
    let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
    db.execute(
        "UPDATE anchors SET updated_at = ?1",
        params![now_ms() - 3 * 24 * 60 * 60 * 1000],
    )
    .expect("backdate");
    drop(db);
    let s = store(
        &dir,
        gc(GcPolicy {
            retention_days: 1,
            gc_on_start: true,
        }),
    );
    assert!(
        s.paths().is_empty(),
        "a record older than retention_days is pruned"
    );
    cleanup(&dir);
}

#[test]
fn gc_age_expiry_disabled_at_zero() {
    let dir = ws("gc-age-off");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let a = dir.join("a.rs");
    fs::write(&a, "one\n").expect("write a");
    let s = store(&dir, no_gc());
    s.save(&pins(a.to_str().expect("utf8"), &["AAAA"], &["c1"]))
        .expect("save");
    drop(s);
    let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
    db.execute(
        "UPDATE anchors SET updated_at = ?1",
        params![now_ms() - 300 * 24 * 60 * 60 * 1000],
    )
    .expect("backdate");
    drop(db);
    // retention_days = 0: the age sweep is off (the missing-file sweep
    // still runs, but a.rs exists, so nothing is pruned).
    let s = store(
        &dir,
        gc(GcPolicy {
            retention_days: 0,
            gc_on_start: true,
        }),
    );
    assert_eq!(s.paths(), vec![a.to_str().expect("utf8").to_owned()]);
    cleanup(&dir);
}

#[test]
fn version_mismatch_nukes_the_anchors_table() {
    let dir = ws("ver");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let s = store(&dir, no_gc());
    s.save(&pins("a.rs", &["AAAA"], &["c1"])).expect("save");
    drop(s);
    // Simulate a future schema bump landing in this file.
    let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
    db.execute(
        "UPDATE meta SET value = '2' WHERE key = 'version'",
        params![],
    )
    .expect("bump version");
    drop(db);
    let s = store(&dir, no_gc());
    assert!(
        s.paths().is_empty(),
        "a version mismatch drops every row (the store is a cache: re-mint)"
    );
    // And the store is still usable after the nuke.
    s.save(&pins("b.rs", &["BBBB"], &["c2"]))
        .expect("save after nuke");
    assert_eq!(s.paths(), vec!["b.rs".to_owned()]);
    cleanup(&dir);
}

#[test]
fn corrupt_db_is_quarantined_and_rebuilt() {
    let dir = ws("quar");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let s = store(&dir, no_gc());
    s.save(&pins("a.rs", &["AAAA"], &["c1"])).expect("save");
    drop(s);
    // Overwrite the db file with garbage so quick_check fails.
    fs::write(
        dir.join("anchors.sqlite"),
        b"this is not a sqlite database at all\n",
    )
    .expect("corrupt");
    let s = store(&dir, no_gc());
    // The corrupt file is quarantined (renamed, original gone) and a
    // fresh db is in its place.
    let quarantined: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("anchors.sqlite.corrupt"))
        })
        .collect();
    assert_eq!(quarantined.len(), 1, "the corrupt db was renamed aside");
    assert!(!dir.join("anchors.sqlite").exists() || quarantined.len() == 1);
    // The store is usable again after the rebuild.
    s.save(&pins("b.rs", &["BBBB"], &["c2"]))
        .expect("save after quarantine");
    assert_eq!(s.paths(), vec!["b.rs".to_owned()]);
    cleanup(&dir);
}

#[test]
fn reanchor_matches_by_checksum_in_order() {
    let prev = pins("a", &["A", "B", "C"], &["s1", "s2", "s3"]);
    // New file: s4 at the front, then s1 and s2 in their original
    // relative order (s3 deleted). Re-anchoring is an in-order LCS: s1
    // and s2 keep their pins, s4 is fresh, s3's slot is tombstoned
    // downstream.
    let new = vec!["s4".to_owned(), "s1".to_owned(), "s2".to_owned()];
    let paired = reanchor(&prev, &new);
    assert!(paired[0].is_none(), "s4 is new");
    assert_eq!(paired[1], Some(0), "s1 keeps its own pin");
    assert_eq!(paired[2], Some(1), "s2 keeps its own pin");
}

#[test]
fn reanchor_reverse_order_is_not_matched() {
    // A three-way reversal: any *pair* of the survivors is in the
    // opposite relative order, so the in-order LCS matches only one of
    // the three and re-mints the other two. (A two-element reversal has
    // LCS 1 — a single element is trivially in-order — so the negative
    // needs three.)
    let prev = pins("a", &["A", "B", "C"], &["s1", "s2", "s3"]);
    let new = vec!["s3".to_owned(), "s2".to_owned(), "s1".to_owned()];
    let paired = reanchor(&prev, &new);
    let matched = paired.iter().filter(|p| p.is_some()).count();
    assert_eq!(matched, 1, "a full reversal matches only one survivor");
    assert_eq!(paired.len() - matched, 2);
}

#[test]
fn next_pins_tombstones_deleted_and_keeps_survivors() {
    let prev = pins("a", &["A", "B"], &["s1", "s2"]);
    // Delete s2 (anchor B), keep s1, add s3.
    let new = vec!["s1".to_owned(), "s3".to_owned()];
    let paired = reanchor(&prev, &new);
    let minted = vec!["A".to_owned(), "ZZZZ".to_owned()];
    let next = next_pins(&prev, &paired, &minted, &new);
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
fn next_pins_keeps_anchors_unique_for_repeated_lines() {
    // A file with two identical lines (same checksum): each copy must keep its
    // own previous pin, not the first pin sharing that checksum — otherwise the
    // persisted set holds one anchor twice and the next session's seeded read
    // serves the duplicate, tripping resolve_anchor's uniqueness invariant.
    let prev = pins("a", &["AA11", "BB22"], &["s1", "s1"]);
    let new = vec!["s1".to_owned(), "s1".to_owned()];
    let paired = reanchor(&prev, &new);
    assert_eq!(
        paired,
        vec![Some(0), Some(1)],
        "each copy pairs with its own previous pin"
    );
    let minted = vec!["ZZZZ".to_owned(), "YYYY".to_owned()];
    let next = next_pins(&prev, &paired, &minted, &new);
    let anchors = next.anchors();
    let unique: std::collections::HashSet<&str> = anchors.iter().map(String::as_str).collect();
    assert_eq!(
        unique.len(),
        anchors.len(),
        "duplicate anchors in persisted pins: {anchors:?}"
    );
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
