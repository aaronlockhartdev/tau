use super::testkit::*;
use super::*;

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
fn retire_marks_every_slot_retired_and_keeps_the_record() {
    let dir = ws("rm");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    s.save(&pins("gone.rs", &["ABCD", "EFGH"], &["c1", "c2"]))
        .expect("save");
    s.retire("gone.rs");
    let retired = s.load("gone.rs").expect("the record survives a retire");
    assert!(
        retired.pins.iter().all(|p| p.tombstone),
        "a write retires every slot"
    );
    assert!(retired.anchors().is_empty(), "a retired slot seeds no line");
    assert!(retired.checksums().is_empty());
    assert_eq!(
        retired.retired_anchors(),
        vec!["ABCD".to_string(), "EFGH".to_string()],
        "the freed anchors stay in the record, claimed"
    );
    cleanup(&dir);
}

#[test]
fn retire_without_a_record_is_a_no_op() {
    let dir = ws("rm-absent");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    s.retire("absent.rs");
    assert!(s.load("absent.rs").is_none());
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

#[test]
fn anchors_and_checksums_skip_retired_slots() {
    let mut set = pins("a.rs", &["AAAA", "BBBB"], &["c1", "c2"]);
    set.pins.push(Pin {
        anchor: "CCCC".into(),
        checksum: "c3".into(),
        tombstone: true,
    });
    assert_eq!(
        set.anchors(),
        vec!["AAAA".to_string(), "BBBB".to_string()],
        "the freed slot is not a line"
    );
    assert_eq!(set.checksums(), vec!["c1".to_string(), "c2".to_string()]);
    assert_eq!(set.retired_anchors(), vec!["CCCC".to_string()]);
}

#[test]
fn reanchor_offers_a_retired_slot_to_no_line() {
    // "c2" was deleted and its slot retired. A line reintroduced with that
    // content takes a fresh anchor, never the freed one.
    let mut prev = pins("a.rs", &["AAAA"], &["c1"]);
    prev.pins.push(Pin {
        anchor: "FREE".into(),
        checksum: "c2".into(),
        tombstone: true,
    });
    let paired = reanchor(&prev, &["c1".into(), "c2".into()]);
    assert_eq!(paired[0], Some(0), "the live slot pairs in order");
    assert_eq!(paired[1], None, "the freed slot is not a candidate");
}

#[test]
fn next_pins_carries_retired_slots_forward() {
    let mut prev = pins("a.rs", &["AAAA", "BBBB"], &["c1", "c2"]);
    prev.pins.push(Pin {
        anchor: "FREE".into(),
        checksum: "c9".into(),
        tombstone: true,
    });
    // c1 survives; c2's line is deleted this round.
    let next = next_pins(&prev, &[Some(0)], &[], &["c1".into()]);
    assert_eq!(next.anchors(), vec!["AAAA".to_string()]);
    assert_eq!(
        next.retired_anchors(),
        vec!["BBBB".to_string(), "FREE".to_string()],
        "the newly freed slot and the slot retired a generation earlier both stay claimed"
    );
    assert_eq!(
        next.pins.iter().filter(|p| !p.tombstone).count(),
        1,
        "the live slot sits before the retired run: [live…, freed…, carried…]"
    );
    assert_eq!(
        next.pins.last().map(|p| p.anchor.as_str()),
        Some("FREE"),
        "the carried slot from the generation before is the tail, so it is the first claim the cap drops"
    );
}

#[test]
fn next_pairs_survivors_with_live_slots_when_retired_slots_sit_between() {
    // A hand-built record: `FilePins.pins` is public, so a retired slot need not
    // trail the live ones. The pairing indexes live pins, so a survivor must
    // never land on a retired slot and claim a freed anchor.
    let mut prev = pins("a.rs", &["AAAA"], &["c1"]);
    prev.pins.push(Pin {
        anchor: "FREE".into(),
        checksum: "c9".into(),
        tombstone: true,
    });
    prev.pins.push(Pin {
        anchor: "BBBB".into(),
        checksum: "c2".into(),
        tombstone: false,
    });
    let next = next_pins(&prev, &[Some(0), Some(1)], &[], &["c1".into(), "c2".into()]);
    assert_eq!(
        next.anchors(),
        vec!["AAAA".to_string(), "BBBB".to_string()],
        "the live slots keep their own anchors; no survivor inherits a freed one"
    );
    assert_eq!(next.retired_anchors(), vec!["FREE".to_string()]);
}

#[test]
fn save_drops_the_oldest_claims_on_the_way_to_disk() {
    // The bound is the store's, not each caller's: a forged record past the
    // cap is durable under it, and no live slot is lost to the drop.
    let dir = ws("cap-save");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    let mut fp = pins("busy.rs", &["AAAA", "BBBB", "CCCC"], &["c1", "c2", "c3"]);
    // Forged in the order `next_pins` builds: the slots it just freed come
    // before the ones it carried, so the vector's tail is the oldest claim.
    for g in (0..MAX_TOMBSTONES + 5).rev() {
        fp.pins.push(Pin {
            anchor: format!("R{g:04}"),
            checksum: "gone".into(),
            tombstone: true,
        });
    }
    s.save(&fp).expect("save");
    let loaded = s.load("busy.rs").expect("load");
    assert_eq!(
        loaded.retired_anchors().len(),
        MAX_TOMBSTONES,
        "the durable record is capped"
    );
    assert_eq!(loaded.anchors(), fp.anchors(), "no live slot is lost");
    assert!(
        !loaded.retired_anchors().contains(&"R0000".to_string()),
        "the oldest claim is the one dropped"
    );
    assert!(
        loaded
            .retired_anchors()
            .contains(&format!("R{:04}", MAX_TOMBSTONES + 4)),
        "the newest claim survives"
    );
    cleanup(&dir);
}

#[test]
fn retire_leaves_a_bounded_graveyard() {
    // A file with more lines than the cap, written once: every slot retires,
    // and the row carries the cap, not the file.
    let dir = ws("cap-retire");
    cleanup(&dir);
    let s = store(&dir, no_gc());
    let lines = MAX_TOMBSTONES + 44;
    let anchors: Vec<String> = (0..lines).map(|i| format!("A{i:04}")).collect();
    let sums: Vec<String> = (0..lines).map(|i| format!("c{i}")).collect();
    let anchor_refs: Vec<&str> = anchors.iter().map(String::as_str).collect();
    let sum_refs: Vec<&str> = sums.iter().map(String::as_str).collect();
    s.save(&pins("big.rs", &anchor_refs, &sum_refs))
        .expect("save");
    s.retire("big.rs");
    let loaded = s.load("big.rs").expect("the retired record stays");
    assert_eq!(
        loaded.pins.len(),
        MAX_TOMBSTONES,
        "the graveyard is the cap, not the file's line count"
    );
    assert!(loaded.live().next().is_none(), "every slot is retired");
    cleanup(&dir);
}
