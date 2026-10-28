use super::*;

use serde_json::json;

/// Three entries written to disk by one store; the returned store is a
/// fresh handle on the same file that has never been `open`ed
/// (`loaded == false`).
fn unopened_store_on_a_file(dir: &Path) -> (SessionStore, Vec<String>) {
    let mut writer = SessionStore::for_workspace(dir, "s1");
    writer.create().unwrap();
    let mut ids = Vec::new();
    let mut parent: Option<String> = None;
    for t in ["one", "two", "three"] {
        let e = writer
            .append(
                EntryKind::User,
                json!({ "text": t, "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id.clone());
        ids.push(e.id);
    }
    (SessionStore::for_workspace(dir, "s1"), ids)
}

#[test]
fn cached_reads_fall_back_to_the_file_when_the_store_is_unopened() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, ids) = unopened_store_on_a_file(dir.path());

    // entries_range_cached: the whole file, then a paged slice.
    let all = store.entries_range_cached(0, usize::MAX).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].0.id, ids[0]);
    let page = store.entries_range_cached(1, 2).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].0.id, ids[1]);

    // entries_since_cached: everything after the cursor.
    let since = store.entries_since_cached(&ids[0]).unwrap();
    assert_eq!(since.len(), 2);
    assert_eq!(since[0].id, ids[1]);

    // entry_cached: a hit and a miss.
    assert_eq!(store.entry_cached(&ids[2]).unwrap().id, ids[2]);
    assert!(store.entry_cached("no-such-entry").is_err());

    // leaf_cached: the last entry in the file.
    let leaf = store.leaf_cached().unwrap().unwrap();
    assert_eq!(leaf.id, ids[2]);
}

#[test]
fn refresh_if_grown_reopens_when_the_file_grew() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, _ids) = unopened_store_on_a_file(dir.path());
    // Unopened: the refresh is a full open (the !loaded arm).
    store.refresh_if_grown().unwrap();
    // Now loaded and current: a no-op stat.
    store.refresh_if_grown().unwrap();
    assert_eq!(store.entries_range_cached(0, usize::MAX).unwrap().len(), 3);
}

#[test]
fn a_second_store_refreshes_before_parent_validation() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SessionStore::for_workspace(dir.path(), "s1");
    writer.create().unwrap();
    let e1 = writer
        .append(EntryKind::User, json!({ "text": "one" }), None)
        .unwrap();
    // A second store opens while the file holds one entry: its in-memory
    // id set is current only until the writer appends again.
    let mut other = SessionStore::for_workspace(dir.path(), "s1");
    other.open().unwrap();
    let e2 = writer
        .append(EntryKind::User, json!({ "text": "two" }), Some(&e1.id))
        .unwrap();
    // The stale store must refresh against the grown file before the
    // parent check, not refuse a valid parent as "unknown".
    let e3 = other
        .append(EntryKind::User, json!({ "text": "three" }), Some(&e2.id))
        .unwrap();
    assert_eq!(e3.parent.as_deref(), Some(e2.id.as_str()));
    // A genuinely unknown parent is still refused.
    let err = other
        .append(
            EntryKind::User,
            json!({ "text": "four" }),
            Some("no-such-entry"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown parent"), "{err}");
}
