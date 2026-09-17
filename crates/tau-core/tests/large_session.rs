//! 10k-entry session fixture — seeds the performance item for acceptance
//! ticket #27 (a ~10k-entry session must paginate in bounded, measured time;
//! spec §3: no full-dump read path).

use std::time::Instant;
use tau_core::session::SessionStore;

/// Write `n` chained entries of ~200 B payloads and report the measured
/// read times for a full open, a tail read, and a mid-file page.
fn run(n: u64) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(tmp.path(), "big");
    store.create().unwrap();

    let write = Instant::now();
    let mut prev: Option<String> = None;
    for i in 0..n {
        let text = format!("entry {i:05} ") + &"p".repeat(150);
        let e = store
            .append(
                "message",
                serde_json::json!({"role": "user", "text": text}),
                prev.as_deref(),
            )
            .unwrap();
        prev = Some(e.id);
    }
    let write_ms = write.elapsed().as_millis();
    let file_kb = std::fs::read(store.path()).unwrap().len() / 1024;

    let mut reader = SessionStore::for_workspace(tmp.path(), "big");
    let t = Instant::now();
    reader.open().unwrap();
    let open_ms = t.elapsed().as_millis();

    let cursor = format!("{n:08}");
    let t = Instant::now();
    let tail = reader.entries_since(&cursor).unwrap();
    let since_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let page = reader
        .entries_range((n / 2) as usize, (n / 2 + 100) as usize)
        .unwrap();
    let range_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let tail100 = reader.entries_since(&format!("{:08}", n - 100)).unwrap();
    let since100_ms = t.elapsed().as_millis();

    eprintln!(
        "10k fixture: {file_kb} KB — write {write_ms} ms, open {open_ms} ms, \
         entries_since(last) {since_ms} ms, entries_since(tail-100) {since100_ms} ms ({} entries), \
         entries_range(middle, 100) {range_ms} ms",
        tail100.len()
    );
    assert_eq!(tail.len(), 0, "the last cursor has nothing after it");
    assert_eq!(tail100.len(), 100);
    assert_eq!(page.len(), 100);
    assert!(open_ms < 500, "open took {open_ms} ms");
    assert!(since_ms < 250, "entries_since took {since_ms} ms");
    assert!(
        since100_ms < 250,
        "entries_since(tail-100) took {since100_ms} ms"
    );
    assert!(range_ms < 250, "entries_range took {range_ms} ms");
}

#[test]
fn ten_k_entry_session_paginates_in_bounded_time() {
    run(10_000);
}
