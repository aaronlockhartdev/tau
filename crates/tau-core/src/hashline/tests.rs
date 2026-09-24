use super::*;
#[test]
fn canon_strips_all_whitespace() {
    assert_eq!(canon("  a \t b \r\n "), "ab");
    assert_eq!(canon(""), "");
}

#[test]
fn duplicate_lines_get_distinct_anchors() {
    // Identical canon -> identical base index; the allocator must
    // spread the duplicates by probing — uniqueness is allocation, not
    // the hash function (the pi-better-edit scheme).
    let content = "x\nx\nx\nx\ny\n";
    let hashes = line_hashes(content).unwrap();
    assert_eq!(hashes.len(), 5);
    assert_ne!(hashes[0], hashes[1]);
    assert_ne!(hashes[1], hashes[2]);
    assert_ne!(hashes[2], hashes[3]);
    assert_ne!(hashes[0], hashes[4]);
    let rendered: Vec<String> = render(content).unwrap();
    let mut sorted = rendered.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), rendered.len(), "anchors are unique per line");
}

#[test]
fn anchors_are_deterministic_per_content() {
    let a = line_hashes("one\ntwo\nthree\n").unwrap();
    let b = line_hashes("one\ntwo\nthree\n").unwrap();
    assert_eq!(a, b);
}

#[test]
fn survivor_anchors_stay_put_across_an_edit() {
    let content = "alpha\nbeta\ngamma\ndelta\n";
    let before = line_hashes(content).unwrap();
    let edit = Edit {
        from: before[1].clone(),
        to: before[1].clone(),
        content: "BETA".into(),
    };
    let edited = apply_edit(content, &edit).unwrap();
    assert_eq!(edited.content, "alpha\nBETA\ngamma\ndelta\n");
    assert_eq!(edited.hashes[0], before[0]);
    assert_eq!(edited.hashes[2], before[2]);
    assert_eq!(edited.hashes[3], before[3]);
    // The edited line gets a (new) anchor of its own.
    assert_ne!(edited.hashes[1], before[0]);
}

#[test]
fn stale_anchor_is_rejected_not_fuzzy_matched() {
    let content = "one\ntwo\n";
    let edit = Edit {
        from: "zzz".into(),
        to: "zzz".into(),
        content: "x".into(),
    };
    match apply_edit(content, &edit) {
        Err(EditError::Stale { anchor }) => assert_eq!(anchor, "zzz"),
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn malformed_anchor_is_rejected() {
    for bad in ["ab", "abcde", "ab│", ""] {
        let edit = Edit {
            from: bad.into(),
            to: bad.into(),
            content: "x".into(),
        };
        let result = apply_edit("one\ntwo\n", &edit);
        // Any resolution of a malformed anchor must fail, never apply.
        assert!(result.is_err(), "anchor {bad:?} must be rejected");
    }
}

#[test]
fn reversed_range_is_rejected() {
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let edit = Edit {
        from: hashes[2].clone(),
        to: hashes[0].clone(),
        content: "x".into(),
    };
    assert!(matches!(
        apply_edit(content, &edit),
        Err(EditError::Reversed { .. })
    ));
}

#[test]
fn range_deletion_joins_the_surrounding_lines() {
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let edit = Edit {
        from: hashes[1].clone(),
        to: hashes[1].clone(),
        content: String::new(),
    };
    let edited = apply_edit(content, &edit).unwrap();
    assert_eq!(edited.content, "one\nthree\n");
}

#[test]
fn whole_file_deletion_requires_write() {
    let content = "only line\n";
    let hashes = line_hashes(content).unwrap();
    let edit = Edit {
        from: hashes[0].clone(),
        to: hashes[0].clone(),
        content: String::new(),
    };
    assert!(matches!(
        apply_edit(content, &edit),
        Err(EditError::EmptyFile)
    ));
}

#[test]
fn edit_over_the_cap_is_diagnosed_as_too_large_not_a_bad_anchor() {
    let content = "x\n".repeat(HASH_SPACE + 1);
    let edit = Edit {
        from: "abc".into(),
        to: "abc".into(),
        content: "y".into(),
    };
    match apply_edit(&content, &edit) {
        Err(EditError::TooLarge) => {}
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn over_the_cap_falls_back_to_write() {
    let content = "x\n".repeat(HASH_SPACE + 1);
    assert!(line_hashes(&content).is_err());
}
