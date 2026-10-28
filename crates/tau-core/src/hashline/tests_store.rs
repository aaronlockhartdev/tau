//! The durable half of the anchor contract: what the pin store's record lets
//! a session re-use, and what it must never hand out again (#207 §2).

use crate::hashline::pin_store::{GcPolicy, PinStore};
use crate::hashline::registry::Registry;

fn store(dir: &std::path::Path) -> PinStore {
    PinStore::new(
        dir,
        GcPolicy {
            retention_days: 0,
            gc_on_start: false,
        },
    )
}

#[test]
fn a_retired_anchor_is_never_minted_again() {
    // The store's guarantee, end to end: a `write` frees the file's anchors,
    // and a re-read of the same content mints fresh ones — never the freed.
    let dir = tempfile::tempdir().unwrap();
    let mut reg = Registry::with_store(store(dir.path()));
    let (first, _) = reg.allocate("a.rs", "one\ntwo\n").expect("allocate");
    reg.refresh_owned("a.rs", "one\ntwo\n", &first);
    reg.free("a.rs");
    let (again, _) = reg.allocate("a.rs", "one\ntwo\n").expect("re-allocate");
    assert_eq!(again.len(), first.len());
    for anchor in &again {
        assert!(
            !first.contains(anchor),
            "a retired slot's anchor is handed out again: {anchor}"
        );
    }
    let record = store(dir.path()).load("a.rs").expect("the record survives");
    assert_eq!(
        record.retired_anchors(),
        first,
        "the record is what keeps the freed anchors claimed"
    );
}

#[test]
fn a_retired_anchor_stays_claimed_when_the_file_drifted() {
    // The drift check seeds nothing, and must not un-claim a freed slot: a
    // reintroduced line takes a fresh anchor, not the one it used to own.
    let dir = tempfile::tempdir().unwrap();
    let mut reg = Registry::with_store(store(dir.path()));
    let (first, _) = reg.allocate("a.rs", "one\n").expect("allocate");
    reg.refresh_owned("a.rs", "one\n", &first);
    reg.free("a.rs");
    let (again, _) = reg.allocate("a.rs", "one\none\n").expect("re-allocate");
    assert_eq!(again.len(), 2);
    assert_ne!(again[0], first[0], "the freed anchor is not re-minted");
}

#[test]
fn a_retired_anchor_stays_claimed_past_the_read_that_freed_it() {
    // The read that seeds the claim evicts the path's owned set when it
    // commits; the claim has to outlive that, or a later read re-mints a freed
    // anchor and the record's tombstones become lies.
    let dir = tempfile::tempdir().unwrap();
    let mut reg = Registry::with_store(store(dir.path()));
    let (first, _) = reg.allocate("a.rs", "one\ntwo\n").expect("allocate");
    reg.refresh_owned("a.rs", "one\ntwo\n", &first);
    reg.free("a.rs");
    let (mid, _) = reg.allocate("a.rs", "one\nX\n").expect("second read");
    let (again, _) = reg.allocate("a.rs", "one\ntwo\n").expect("third read");
    for anchor in mid.iter().chain(again.iter()) {
        assert!(
            !first.contains(anchor),
            "a freed anchor came back on a later read: {anchor}"
        );
    }
}

#[test]
fn a_retired_anchor_is_never_minted_by_a_later_session() {
    // The record is the durable claim: a session that never saw the write must
    // still skip the freed slots on its first read, and on every read after.
    let dir = tempfile::tempdir().unwrap();
    let mut writer = Registry::with_store(store(dir.path()));
    let (first, _) = writer.allocate("a.rs", "one\ntwo\n").expect("allocate");
    writer.refresh_owned("a.rs", "one\ntwo\n", &first);
    writer.free("a.rs");
    drop(writer);
    let mut later = Registry::with_store(store(dir.path()));
    let (reread, _) = later
        .allocate("a.rs", "one\ntwo\n")
        .expect("re-read in a new session");
    let (again, _) = later.allocate("a.rs", "one\nX\n").expect("second read");
    for anchor in reread.iter().chain(again.iter()) {
        assert!(
            !first.contains(anchor),
            "a freed anchor outlived its retirement: {anchor}"
        );
    }
}
