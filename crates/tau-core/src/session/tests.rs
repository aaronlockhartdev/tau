use super::*;

pub(super) fn store(dir: &Path, id: &str) -> SessionStore {
    SessionStore::for_workspace(dir, id)
}

pub(super) fn seeded(dir: &Path) -> (SessionStore, Vec<Entry>) {
    let mut s = store(dir, "s1");
    s.create().unwrap();
    let e1 = s
        .append(
            "message",
            serde_json::json!({"role": "user", "text": "one"}),
            None,
        )
        .unwrap();
    let e2 = s
        .append(
            "message",
            serde_json::json!({"role": "assistant", "text": "two"}),
            Some(&e1.id),
        )
        .unwrap();
    let e3 = s
        .append(
            "message",
            serde_json::json!({"role": "user", "text": "three"}),
            Some(&e2.id),
        )
        .unwrap();
    (s, vec![e1, e2, e3])
}

#[test]
fn create_writes_a_versioned_header() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = store(tmp.path(), "s1");
    s.create().unwrap();
    let line = fs::read_to_string(s.path()).unwrap();
    let h: Header = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(h.version, FILE_VERSION);
    assert_eq!(h.id, "s1");
    assert!(s.path().starts_with(tmp.path().join(".tau")));
}

#[test]
fn append_round_trip_builds_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, entries) = seeded(tmp.path());
    assert_eq!(entries[0].parent, None);
    assert_eq!(entries[1].parent, Some(entries[0].id.clone()));
    assert_eq!(entries[2].parent, Some(entries[1].id.clone()));
    let leaf = s.leaf().unwrap().unwrap();
    assert_eq!(leaf.id, entries[2].id);
    let again = s.entry(&entries[1].id).unwrap();
    assert_eq!(again, entries[1]);
}

#[test]
fn branch_appends_a_child_line() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, entries) = seeded(tmp.path());
    // Branch off the first entry — an append, no rewrite.
    let e4 = s
        .append(
            "message",
            serde_json::json!({"text": "branch"}),
            Some(&entries[0].id),
        )
        .unwrap();
    assert_eq!(e4.parent, Some(entries[0].id.clone()));
    assert_eq!(s.leaf().unwrap().unwrap().id, e4.id);
    let since = s.entries_since(&entries[0].id).unwrap();
    assert_eq!(
        since.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec![
            entries[1].id.as_str(),
            entries[2].id.as_str(),
            e4.id.as_str()
        ]
    );
}

#[test]
fn set_leaf_rewrites_only_the_header() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, entries) = seeded(tmp.path());
    let before = fs::read_to_string(s.path()).unwrap();
    s.set_leaf(&entries[0].id).unwrap();
    let after = fs::read_to_string(s.path()).unwrap();
    let (_b_h, b_rest) = before.split_once('\n').unwrap();
    let (a_h, a_rest) = after.split_once('\n').unwrap();
    assert_eq!(b_rest, a_rest);
    let h: Header = serde_json::from_str(a_h).unwrap();
    assert_eq!(h.leaf, Some(entries[0].id.clone()));
    assert!(!s.path().with_extension("tmp").exists());
    assert_eq!(s.leaf().unwrap().unwrap().id, entries[0].id);
}

#[test]
fn set_title_persists_in_the_header() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, _entries) = seeded(tmp.path());
    s.set_title("Brave Otter").unwrap();
    assert_eq!(s.title(), Some("Brave Otter"));
    let mut reopened = SessionStore::for_workspace(tmp.path(), s.id());
    reopened.open().unwrap();
    assert_eq!(reopened.title(), Some("Brave Otter"));
}

#[test]
fn set_parent_persists_in_the_header() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, _entries) = seeded(tmp.path());
    s.set_parent("18d63f408cee5080").unwrap();
    assert_eq!(s.parent(), Some("18d63f408cee5080"));
    let mut reopened = SessionStore::for_workspace(tmp.path(), s.id());
    reopened.open().unwrap();
    assert_eq!(reopened.parent(), Some("18d63f408cee5080"));
}

#[test]
fn crc_catches_a_flipped_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, entries) = seeded(tmp.path());
    let mut raw = fs::read_to_string(s.path()).unwrap().into_bytes();
    // Flip a byte inside a *value* on the second entry line (file line
    // 3) so the JSON stays valid and only the CRC can catch it.
    let text = String::from_utf8(raw.clone()).unwrap();
    let lines: Vec<&str> = text.split('\n').collect();
    let start = lines[..2].iter().map(|l| l.len() + 1).sum::<usize>();
    let off = start + lines[2].find("\"two\"").unwrap() + 3;
    raw[off] = b'i';
    fs::write(s.path(), &raw).unwrap();
    let err = s.entries_range(1, 2).unwrap_err();
    match err {
        Error::Corrupt { line, .. } => assert_eq!(line, 3),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    // The uncorrupted entry still reads.
    assert_eq!(s.entry(&entries[0].id).unwrap().id, entries[0].id);
}

/// The table-driven CRC32 must produce the same values as the old bitwise
/// loop (the spec §3 verification values never change): the classic
/// check vector plus a direct equivalence against the old algorithm.
#[test]
fn crc32_table_matches_the_old_bitwise_values() {
    // The classic CRC-32/IEEE check value.
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    // Deterministic pseudo-random payload: table == bitwise, byte for byte.
    fn crc32_bitwise(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & ((crc & 1) * 0xEDB8_8320));
            }
        }
        !crc
    }
    let mut data = Vec::new();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..4096 {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(i as u64);
        data.push((x >> 33) as u8);
    }
    assert_eq!(crc32(&data), crc32_bitwise(&data));
}

/// The in-memory paged reads (a live session's read path) serve the same
/// data as the file-backed verification reads: same entries, same paging
/// indexes, same sizes, same leaf — and an append lands in the log.
#[test]
fn cached_paged_reads_match_the_file_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, entries) = seeded(tmp.path());
    // A range page: the middle entry, with its file-line byte length.
    let page = s.entries_range_cached(1, 2).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].0, entries[1]);
    assert_eq!(
        page[0].1,
        serde_json::to_vec(&entries[1]).unwrap().len() as u64
    );
    // A since-read: everything after the first entry, as the file path serves it.
    assert_eq!(
        s.entries_since_cached(&entries[0].id).unwrap(),
        s.entries_since(&entries[0].id).unwrap()
    );
    // One-by-one and the leaf.
    assert_eq!(s.entry_cached(&entries[2].id).unwrap(), entries[2]);
    assert_eq!(s.leaf_cached().unwrap().unwrap().id, entries[2].id);
    // An append lands in the in-memory log too (the live path's truth).
    let e4 = s
        .append(
            "message",
            serde_json::json!({ "text": "four" }),
            Some(&entries[2].id),
        )
        .unwrap();
    let page = s.entries_range_cached(3, 4).unwrap();
    assert_eq!(page[0].0, e4);
}

#[test]
fn a_torn_last_line_is_skipped_not_repaired_on_open() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, entries) = seeded(tmp.path());
    let mut raw = fs::read_to_string(s.path()).unwrap();
    raw.push_str(&format!("{{\"id\":\"{:08}\",\"parentId\":null", 99)); // no newline: a kill mid-append
    let before = raw.clone();
    fs::write(s.path(), raw).unwrap();
    let mut reopened = store(tmp.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(reopened.leaf().unwrap().unwrap().id, entries[2].id);
    // The read path never rewrites: the file is exactly as found.
    assert_eq!(fs::read_to_string(s.path()).unwrap(), before);
}

#[test]
fn a_valid_last_line_without_newline_is_kept_not_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, entries) = seeded(tmp.path());
    let raw = fs::read_to_string(s.path()).unwrap();
    let before = raw.trim_end_matches('\n').to_string();
    fs::write(s.path(), &before).unwrap();
    let mut reopened = store(tmp.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(reopened.leaf().unwrap().unwrap().id, entries[2].id);
    // The read path never rewrites: the file is exactly as found.
    assert_eq!(fs::read_to_string(s.path()).unwrap(), before);
}

/// The writer settles a torn tail: the next append truncates the torn
/// line, so no mangled line lands mid-file and the session re-opens
/// clean.
#[test]
fn an_append_settles_a_torn_tail() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, entries) = seeded(tmp.path());
    // A kill mid-append: a torn partial line, unterminated.
    let mut raw = fs::read_to_string(s.path()).unwrap();
    raw.push_str(&format!(
        "{{\"id\":\"{:08}\",\"parentId\":\"{}\"",
        99, entries[2].id
    ));
    fs::write(s.path(), raw).unwrap();
    let e4 = s
        .append(
            "message",
            serde_json::json!({ "text": "after the tear" }),
            Some(&entries[2].id),
        )
        .unwrap();
    let mut reopened = store(tmp.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(
        reopened.leaf().unwrap().unwrap().id,
        e4.id,
        "the session re-opens clean after the settle"
    );
    let all = reopened.entries_range(0, usize::MAX).unwrap();
    assert_eq!(all.len(), 4);
}

/// A complete line missing its newline is terminated by the next
/// append, not dropped or mangled.
#[test]
fn an_append_terminates_a_complete_line_missing_its_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, entries) = seeded(tmp.path());
    let raw = fs::read_to_string(s.path()).unwrap();
    fs::write(s.path(), raw.trim_end_matches('\n')).unwrap();
    let e4 = s
        .append(
            "message",
            serde_json::json!({ "text": "next" }),
            Some(&entries[2].id),
        )
        .unwrap();
    let mut reopened = store(tmp.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(reopened.leaf().unwrap().unwrap().id, e4.id);
    let all = reopened.entries_range(0, usize::MAX).unwrap();
    assert_eq!(all.len(), 4);
}

#[test]
fn small_payload_stays_inline() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = store(tmp.path(), "s1");
    s.create().unwrap();
    let e = s
        .append("message", Value::String("y".repeat(50)), None)
        .unwrap();
    assert!(e.blob.is_none());
    assert_eq!(e.payload, Value::String("y".repeat(50)));
}

#[test]
fn unsupported_version_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _) = seeded(tmp.path());
    let mut raw = fs::read_to_string(s.path()).unwrap();
    raw = raw.replacen("\"version\":1", "\"version\":9", 1);
    fs::write(s.path(), raw).unwrap();
    let mut reopened = store(tmp.path(), "s1");
    assert!(reopened.open().is_err());
}

#[test]
fn unknown_parent_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, _) = seeded(tmp.path());
    assert!(
        s.append("message", serde_json::json!({}), Some("99999999"))
            .is_err()
    );
}

/// A store whose file is deleted (or archived) after open refuses the
/// next append instead of resurrecting the file headerless (unopenable,
/// invisible to the list, orphaned on disk; ADR-0005).
#[test]
fn an_append_to_a_deleted_file_is_refused_not_resurrected() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, _) = seeded(tmp.path());
    std::fs::remove_file(s.path()).unwrap();
    let err = s
        .append("message", serde_json::json!({ "text": "ghost" }), None)
        .unwrap_err();
    assert!(
        matches!(err, Error::Other(_)),
        "expected a plain refusal: {err:?}"
    );
    assert!(
        !s.path().exists(),
        "a refused append must not recreate the file"
    );
}

#[test]
fn leaf_is_none_on_a_fresh_session_and_the_entry_on_a_used_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store(dir.path(), "s1");
    store.create().unwrap();
    assert_eq!(store.leaf().unwrap(), None);
    let e = store
        .append("user", serde_json::json!({"text": "hi"}), None)
        .unwrap();
    assert_eq!(store.leaf().unwrap().unwrap().id, e.id);
}

#[test]
fn a_fork_copies_the_entry_tree_with_stable_ids_and_crcs() {
    let dir = tempfile::tempdir().unwrap();
    let (s, entries) = seeded(dir.path());
    let _forked = SessionStore::fork_from(&s, "s2").unwrap();
    // A second independent open verifies every copied line's CRC and
    // resolves the inherited leaf.
    let mut re = store(dir.path(), "s2");
    re.open().unwrap();
    let copied = re.entries_range(0, usize::MAX).unwrap();
    assert_eq!(copied.len(), entries.len());
    for (a, b) in entries.iter().zip(&copied) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.parent, b.parent);
        assert_eq!(a.crc, b.crc);
    }
    // The fork appends against the inherited leaf, not a new root.
    let e = re
        .append(
            "user",
            serde_json::json!({"text": "after the fork"}),
            Some(&entries.last().unwrap().id),
        )
        .unwrap();
    assert_eq!(
        e.parent.as_deref(),
        Some(entries.last().unwrap().id.as_str())
    );
}

#[test]
fn a_fork_cannot_target_the_source_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (s, _) = seeded(dir.path());
    assert!(SessionStore::fork_from(&s, "s1").is_err());
}

#[test]
fn a_fork_cannot_clobber_an_existing_session_file() {
    let dir = tempfile::tempdir().unwrap();
    let (s, _) = seeded(dir.path());
    let mut taken = store(dir.path(), "s2");
    taken.create().unwrap();
    taken
        .append("user", serde_json::json!({"text": "the original"}), None)
        .unwrap();
    assert!(SessionStore::fork_from(&s, "s2").is_err());
    // The existing file is untouched.
    let mut re = store(dir.path(), "s2");
    re.open().unwrap();
    let entries = re.entries_range(0, usize::MAX).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].payload["text"], "the original");
}

#[test]
fn session_ids_are_distinct_and_well_formed() {
    let a = SessionStore::new_session_id();
    let b = SessionStore::new_session_id();
    assert_ne!(a, b);
    assert_eq!(a.len(), 12);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}
