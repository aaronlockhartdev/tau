use super::*;

/// A record of `n` live slots, seeded with anchors `S{index:05}`.
fn record(path: &str, n: usize) -> FilePins {
    let checksums: Vec<String> = (0..n).map(|i| format!("c0-{i}")).collect();
    FilePins {
        path: path.into(),
        checksum: file_checksum(&checksums),
        line_count: n,
        pins: checksums
            .iter()
            .enumerate()
            .map(|(i, c)| Pin {
                anchor: format!("S{i:05}"),
                checksum: c.clone(),
                tombstone: false,
            })
            .collect(),
        crc: None,
    }
}

/// One generation where every line is fresh: the pairing matches nothing, so
/// the previous generation's live slots are all freed and `A{round:05}` is
/// minted for each line.
fn rewrite(prev: &FilePins, round: usize) -> FilePins {
    let n = prev.line_count;
    let checksums: Vec<String> = (0..n).map(|i| format!("c{round}-{i}")).collect();
    let minted: Vec<String> = (0..n).map(|_| format!("A{round:05}")).collect();
    let paired = vec![None; n];
    next_pins(prev, &paired, &minted, &checksums)
}

fn retired(prev: &FilePins) -> Vec<String> {
    prev.retired_anchors()
}

#[test]
fn a_thousand_generations_stay_under_the_cap() {
    // The ticket's probe, run to a thousand cycles: the record's live set is
    // the file, and its claims never pass the cap.
    let mut prev = record("busy.rs", 3);
    for round in 1..=1_000 {
        prev = rewrite(&prev, round);
        assert_eq!(
            prev.pins.len(),
            3 + MAX_TOMBSTONES.min(3 * round),
            "round {round}: live slots plus the claims, never more"
        );
        assert_eq!(prev.live().count(), 3, "the live set is the file");
    }
    assert_eq!(retired(&prev).len(), MAX_TOMBSTONES);
}

#[test]
fn the_cap_keeps_the_newest_claims_and_drops_the_oldest() {
    let mut prev = record("busy.rs", 1);
    let overflow = MAX_TOMBSTONES + 1;
    for round in 1..=overflow {
        prev = rewrite(&prev, round);
    }
    let claims = retired(&prev);
    assert_eq!(claims.len(), MAX_TOMBSTONES, "one overflow claim dropped");
    let newest_freed = format!("A{:05}", overflow - 1);
    assert!(
        claims.contains(&newest_freed),
        "the newest freed slot ({newest_freed}) is still claimed"
    );
    assert!(
        !claims.contains(&"S00000".to_string()),
        "the oldest freed slot is the claim that goes"
    );
}

#[test]
fn a_file_with_no_freed_slots_is_untouched_by_the_cap() {
    // Negative: the bound is on claims, not on the file. A record of a
    // thousand live lines is durable as it stands.
    let mut big = record("big.rs", 1_000);
    let anchors = big.anchors();
    big.cap_retired();
    assert_eq!(big.pins.len(), 1_000);
    assert_eq!(big.anchors(), anchors, "no live slot is a cap casualty");
}

#[test]
fn a_capped_record_reanchors_by_content() {
    // The cap is a size rule, not a re-anchor rule: the live set is the
    // re-anchor base, so a capped record pairs a surviving line as usual.
    let mut prev = record("busy.rs", 1);
    for round in 1..=MAX_TOMBSTONES + 1 {
        prev = rewrite(&prev, round);
    }
    // Every rewrite retired the seed line, so no live slot carries its
    // checksum any more — and the live set is the re-anchor base.
    let paired = reanchor(&prev, &["c0-0".to_string()]);
    assert_eq!(
        paired,
        vec![None],
        "a line no live slot carries pairs with nothing"
    );
    let next = next_pins(&prev, &paired, &["ZZZZ".to_string()], &["c0-0".to_string()]);
    assert_eq!(next.pins[0].anchor, "ZZZZ", "the fresh line takes a mint");
    assert!(
        next.retired_anchors().len() <= MAX_TOMBSTONES,
        "and the record lands under the cap"
    );
}
