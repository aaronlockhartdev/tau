//! The store's own lifecycle: the workspace file it opens, the session-start
//! sweeps that keep its rows honest (#225), and the schema guards.

use super::testkit::{cleanup, no_gc, pins, store, ws};
use super::{GcPolicy, PathBuf, now_ms};
use rusqlite::params;
use std::fs;
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
        GcPolicy {
            retention_days: 0,
            gc_on_start: true,
        },
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
        GcPolicy {
            retention_days: 1,
            gc_on_start: true,
        },
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
        GcPolicy {
            retention_days: 0,
            gc_on_start: true,
        },
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
fn gc_prunes_a_retired_record_past_retention() {
    // The age sweep keys on the row, and a retire refreshes it: a record that
    // stops being written ages out, one that is fresh does not.
    let dir = ws("gc-retired");
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let a = dir.join("a.rs");
    fs::write(&a, "one\n").expect("write a");
    let path = a.to_str().expect("utf8").to_owned();
    let s = store(&dir, no_gc());
    s.save(&pins(&path, &["AAAA"], &["c1"])).expect("save");
    s.retire(&path);
    drop(s);
    let fresh = store(
        &dir,
        GcPolicy {
            retention_days: 1,
            gc_on_start: true,
        },
    );
    assert_eq!(
        fresh.paths().len(),
        1,
        "a retired record written today keeps its claims"
    );
    drop(fresh);
    let db = rusqlite::Connection::open(dir.join("anchors.sqlite")).expect("open");
    db.execute(
        "UPDATE anchors SET updated_at = ?1",
        params![now_ms() - 3 * 24 * 60 * 60 * 1000],
    )
    .expect("backdate");
    drop(db);
    let aged = store(
        &dir,
        GcPolicy {
            retention_days: 1,
            gc_on_start: true,
        },
    );
    assert!(
        aged.paths().is_empty(),
        "an all-retired record past retention is pruned, ending its claims"
    );
    cleanup(&dir);
}
