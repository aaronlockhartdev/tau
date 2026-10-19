use super::*;

#[test]
fn encoding_is_a_bijection_over_the_full_space() {
    // 52^4 slots, 4 letters: encode every slot and require a lossless decode.
    for idx in 0..HASH_SPACE {
        let h = idx_to_hash(idx);
        assert_eq!(h.len(), 4);
        assert!(
            h.bytes().all(|b| ALPHA.as_bytes().contains(&b)),
            "letters only: {h}"
        );
        assert_eq!(
            hash_to_idx(&h),
            Some(idx),
            "round-trip failed at {idx} ({h})"
        );
    }
    // A digit or a 3-char anchor is outside the alphabet.
    assert_eq!(hash_to_idx("a1c4"), None);
    assert_eq!(hash_to_idx("abc"), None);
}

#[test]
fn stride_is_coprime_with_the_space() {
    fn gcd(mut a: usize, mut b: usize) -> usize {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    }
    assert_eq!(gcd(STRIDE, HASH_SPACE), 1);
}

#[test]
fn hash_source_truncates_at_500_bytes_on_a_char_boundary() {
    let long = "a".repeat(600);
    assert_eq!(hash_source(&long), "a".repeat(500));
    // A multibyte char straddling the 500-byte mark stays intact.
    let straddling = "é".repeat(250); // 500 bytes exactly
    assert_eq!(hash_source(&straddling), straddling);
    let straddling_plus = format!("{straddling}é"); // 502 bytes
    assert_eq!(hash_source(&straddling_plus).len(), 500);
    // Short lines are unaffected.
    assert_eq!(hash_source("x"), "x");
}

#[test]
fn lines_sharing_a_500_byte_canon_prefix_share_a_base_slot() {
    let prefix = "b".repeat(500);
    let a = format!("{prefix}one");
    let b = format!("{prefix}two");
    // Same base slot, distinct allocated anchors.
    let content = format!("{a}\n{b}\n");
    let hashes = line_hashes(&content).unwrap();
    assert_ne!(hashes[0], hashes[1]);
    // A third line with the same prefix probes onward: still unique.
    let c = format!("{prefix}three");
    let hashes = line_hashes(&format!("{a}\n{b}\n{c}\n")).unwrap();
    assert_ne!(hashes[0], hashes[1]);
    assert_ne!(hashes[1], hashes[2]);
    assert_ne!(hashes[0], hashes[2]);
}

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
        from: "zzzz".into(),
        to: "zzzz".into(),
        content: "x".into(),
    };
    match apply_edit(content, &edit) {
        Err(EditError::Stale { anchor }) => assert_eq!(anchor, "zzzz"),
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn malformed_anchor_is_rejected() {
    for bad in ["abc", "abcde", "ab│", "a1c4"] {
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

#[test]
fn apply_edits_multiple_disjoint_ranges() {
    let content = "one\ntwo\nthree\nfour\nfive\n";
    let hashes = line_hashes(content).unwrap();
    let edits = vec![
        Edit {
            from: hashes[0].clone(),
            to: hashes[0].clone(),
            content: "ONE".into(),
        },
        Edit {
            from: hashes[2].clone(),
            to: hashes[2].clone(),
            content: "THREE".into(),
        },
    ];
    let edited = apply_edits(content, &edits).unwrap();
    assert_eq!(edited.content, "ONE\ntwo\nTHREE\nfour\nfive\n");
    // Survivors keep their anchors.
    assert_eq!(edited.hashes[1], hashes[1]);
    assert_eq!(edited.hashes[3], hashes[3]);
    // Removed lines are the union of both ranges, in order.
    assert_eq!(edited.removed_lines, vec!["one", "three"]);
}

#[test]
fn apply_edits_rejects_overlapping_ranges() {
    let content = "one\ntwo\nthree\nfour\n";
    let hashes = line_hashes(content).unwrap();
    let edits = vec![
        Edit {
            from: hashes[0].clone(),
            to: hashes[1].clone(),
            content: "A".into(),
        },
        Edit {
            from: hashes[1].clone(),
            to: hashes[2].clone(),
            content: "B".into(),
        },
    ];
    assert!(matches!(
        apply_edits(content, &edits),
        Err(EditError::Overlapping { .. })
    ));
}

#[test]
fn apply_edits_rejects_adjacent_ranges() {
    // Adjacent (line 0, then line 1) is "touching" — rejected.
    // The allow condition is a.to+1 < b.from (at least one gap line).
    let content = "one\ntwo\nthree\nfour\n";
    let hashes = line_hashes(content).unwrap();
    let edits = vec![
        Edit {
            from: hashes[0].clone(),
            to: hashes[0].clone(),
            content: "A".into(),
        },
        Edit {
            from: hashes[1].clone(),
            to: hashes[1].clone(),
            content: "B".into(),
        },
    ];
    assert!(matches!(
        apply_edits(content, &edits),
        Err(EditError::Overlapping { .. })
    ));
}

#[test]
fn apply_edits_rejects_true_overlap() {
    let content = "one\ntwo\nthree\nfour\n";
    let hashes = line_hashes(content).unwrap();
    let edits = vec![
        Edit {
            from: hashes[0].clone(),
            to: hashes[1].clone(),
            content: "A".into(),
        },
        Edit {
            from: hashes[1].clone(),
            to: hashes[1].clone(),
            content: "B".into(),
        },
    ];
    // First edit covers lines 0-1, second covers line 1 — true overlap.
    assert!(matches!(
        apply_edits(content, &edits),
        Err(EditError::Overlapping { .. })
    ));
}

#[test]
fn apply_edits_stale_anchor_in_one_of_many() {
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let edits = vec![
        Edit {
            from: hashes[0].clone(),
            to: hashes[0].clone(),
            content: "A".into(),
        },
        Edit {
            from: "zzzz".into(),
            to: hashes[2].clone(),
            content: "B".into(),
        },
    ];
    assert!(matches!(
        apply_edits(content, &edits),
        Err(EditError::Stale { .. })
    ));
}

#[test]
fn apply_edits_rejects_anchored_content() {
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    // The model pasted read output verbatim: every line carries a HASH│
    // prefix, first line's anchor = from, last line's = to.
    let anchored = format!("{}│one\n{}│two", hashes[0], hashes[1]);
    let edits = vec![Edit {
        from: hashes[0].clone(),
        to: hashes[1].clone(),
        content: anchored,
    }];
    assert!(matches!(
        apply_edits(content, &edits),
        Err(EditError::AnchoredContent { at: 0 })
    ));
}

#[test]
fn apply_edits_partial_anchored_content_is_not_caught() {
    // Only some lines are anchored → the check does NOT fire; the call
    // proceeds to normal resolution.
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let partial = format!("{}│one\ntwo", hashes[0]);
    let edits = vec![Edit {
        from: hashes[0].clone(),
        to: hashes[1].clone(),
        content: partial,
    }];
    let result = apply_edits(content, &edits);
    assert!(
        !matches!(result, Err(EditError::AnchoredContent { .. })),
        "partial anchoring must not trigger the anchored-content check"
    );
}

#[test]
fn apply_edits_bookend_mismatch_is_not_caught() {
    // All lines anchored but the first/last don't match from/to.
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let mismatched = format!("{}│one\n{}│two", hashes[2], hashes[0]);
    let edits = vec![Edit {
        from: hashes[0].clone(),
        to: hashes[1].clone(),
        content: mismatched,
    }];
    let result = apply_edits(content, &edits);
    assert!(
        !matches!(result, Err(EditError::AnchoredContent { .. })),
        "bookend mismatch must not trigger the anchored-content check"
    );
}
