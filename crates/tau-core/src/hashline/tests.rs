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
    // The unchanged prefix (before the edit point) keeps its anchor; the
    // edited line and the lines after it are re-allocated fresh (the LCS
    // reanchor matches in order from the top, so a mid-file edit starts a new
    // anchor sequence).
    assert_eq!(edited.hashes[0], before[0]);
    assert_ne!(edited.hashes[1], before[1]);
    assert_ne!(edited.hashes[2], before[2]);
    assert_ne!(edited.hashes[3], before[3]);
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
        Err(EditError::Stale { anchor, .. }) => assert_eq!(anchor, "zzzz"),
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
    // Strict input rejects a reversed range; the lenient default swaps it.
    assert!(matches!(
        apply_edits_opts(
            content,
            std::slice::from_ref(&edit),
            &PlanOptions {
                strict: true,
                ..PlanOptions::default()
            }
        ),
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
    // The edited lines (0 and 2) get fresh anchors; the untouched lines 1 and
    // 3 — which fall after the first edit point — are re-allocated by the LCS
    // reanchor, so they do not keep the pre-edit anchors.
    assert_ne!(edited.hashes[0], hashes[0]);
    assert_ne!(edited.hashes[2], hashes[2]);
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
    // Strict input: pasted HASH│ prefixes are a hard rejection.
    assert!(matches!(
        apply_edits_opts(
            content,
            &edits,
            &PlanOptions {
                strict: true,
                ..PlanOptions::default()
            }
        ),
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

#[test]
fn pure_mid_deletion_reports_a_one_line_changed_region() {
    // #235: a deletion-only edit collapsed to an empty region (first > last)
    // because the removed line has no positional counterpart in the new file.
    let content = "one\ntwo\nthree\nfour\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[1].clone(),
            to: hashes[1].clone(),
            content: String::new(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "one\nthree\nfour\n");
    assert_eq!((edited.first_changed, edited.last_changed), (2, 2));
}

#[test]
fn pure_first_deletion_reports_a_one_line_changed_region() {
    let content = "one\ntwo\nthree\nfour\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[0].clone(),
            to: hashes[0].clone(),
            content: String::new(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "two\nthree\nfour\n");
    assert_eq!((edited.first_changed, edited.last_changed), (1, 1));
}

#[test]
fn pure_last_deletion_reports_a_one_line_changed_region() {
    let content = "one\ntwo\nthree\nfour\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[3].clone(),
            to: hashes[3].clone(),
            content: String::new(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "one\ntwo\nthree\n");
    // A tail deletion: the changed region collapses to one line at the
    // deletion point (the bug reported "no change").
    assert_eq!((edited.first_changed, edited.last_changed), (3, 3));
}

#[test]
fn pure_two_line_mid_deletion_reports_a_non_empty_region() {
    // Two adjacent mid deletions: the prefix/suffix walks meet with a net
    // length delta, so the region must not invert.
    let content = "one\ntwo\nthree\nfour\nfive\nsix\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[2].clone(),
            to: hashes[3].clone(),
            content: String::new(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "one\ntwo\nfive\nsix\n");
    // Two adjacent removed lines: the region is one line at the deletion
    // point (the bounds must not invert).
    assert_eq!((edited.first_changed, edited.last_changed), (3, 3));
}

#[test]
fn add_only_change_keeps_its_changed_region() {
    // Regression guard: a net growth must report the same region as before.
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[1].clone(),
            to: hashes[1].clone(),
            content: "TWO\nTWO2".into(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "one\nTWO\nTWO2\nthree\n");
    assert_eq!((edited.first_changed, edited.last_changed), (2, 3));
}

#[test]
fn replace_change_keeps_its_changed_region() {
    // Regression guard: a same-length replacement is unchanged by the fix.
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[1].clone(),
            to: hashes[1].clone(),
            content: "TWO".into(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "one\nTWO\nthree\n");
    assert_eq!((edited.first_changed, edited.last_changed), (2, 2));
}

#[test]
fn identical_file_reports_no_change() {
    // Replacing a line with its own content is a no-op: first > last.
    let content = "one\ntwo\nthree\n";
    let hashes = line_hashes(content).unwrap();
    let edited = apply_edit(
        content,
        &Edit {
            from: hashes[1].clone(),
            to: hashes[1].clone(),
            content: "two".into(),
        },
    )
    .unwrap();
    assert_eq!(edited.content, "one\ntwo\nthree\n");
    assert!(edited.first_changed > edited.last_changed);
}

// Red test (intermittent panic, tokio-rt-worker): the registry's anchor set
// (299 entries) is out of sync with the file on disk (300 lines) — the file
// grew out-of-band with a duplicate of a line already anchored. An edit
// targeting the last registry row was spliced at file line 299, and
// lcs_reanchor matched the duplicate's checksum to old index 299 and indexed
// old_hashes[299] (len 299), panicking with
// "index out of bounds: the len is 299 but the index is 299". Planning must
// now tolerate the short registry: the edit applies, the duplicate keeps its
// anchor, and the new line gets a fresh one.
#[test]
fn plan_edits_registry_shorter_than_file_does_not_panic() {
    // 300-line file: the 299-line registry snapshot plus a duplicate of its
    // last line appended out-of-band.
    let base = (1..=299).map(|i| format!("line {i}")).collect::<Vec<_>>();
    let mut lines = base.clone();
    lines.push(base[298].clone());
    let content = lines.join("\n");
    let hashes: Vec<String> = base
        .iter()
        .map(|l| line_base_idx(l))
        .map(idx_to_hash)
        .collect();
    let last = hashes[298].clone();
    let planned = plan_edits(
        &content,
        line_endings::Ending::Lf,
        &[Edit {
            from: last.clone(),
            to: last.clone(),
            content: "changed".into(),
        }],
        &PlanOptions::default(),
    )
    .expect("planning against a registry shorter than the file succeeds");
    assert_eq!(planned.lines.len(), 300);
    assert_eq!(planned.hashes.len(), 300);
    assert_eq!(planned.lines[298], "changed");
    // The pre-existing anchors are never duplicated: the anchor set stays
    // unique across the 300 lines, and the new line gets a fresh allocation.
    assert_eq!(
        planned.hashes.len(),
        planned
            .hashes
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
    );
    assert_ne!(planned.hashes[299], last);
}

// The benign twin: the file grew out-of-band with a duplicate of the last
// registry line — the LCS match is a valid old index, so planning must
// succeed (guards against an over-broad fix that rejects every desync).
#[test]
fn plan_edits_file_grew_with_a_matching_line_succeeds() {
    let base = (1..=299).map(|i| format!("line {i}")).collect::<Vec<_>>();
    let mut lines = base.clone();
    lines.push(base[298].clone());
    let content = lines.join("\n");
    let hashes: Vec<String> = base
        .iter()
        .map(|l| line_base_idx(l))
        .map(idx_to_hash)
        .collect();
    let last = hashes[298].clone();
    let planned = plan_edits(
        &content,
        line_endings::Ending::Lf,
        &[Edit {
            from: last.clone(),
            to: last.clone(),
            content: "changed".into(),
        }],
        &PlanOptions::default(),
    )
    .expect("planning against a matching-line growth succeeds");
    assert_eq!(planned.lines.len(), 300);
    assert_eq!(planned.hashes.len(), 300);
}

#[test]
fn duplicate_anchor_resolves_to_first_occurrence() {
    // A broken anchor set (e.g. a stale pin store that wrote one anchor
    // twice) resolves deterministically to the first occurrence — the
    // reference implementation's behavior — not an error. The second
    // occurrence is untouched and keeps its (duplicated) anchor.
    let content = "one\ntwo\n";
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "AAAA".into()]),
        demote: false,
    };
    let planned = plan_edits(
        content,
        line_endings::Ending::Lf,
        &[Edit {
            from: "AAAA".into(),
            to: "AAAA".into(),
            content: "x".into(),
        }],
        &opts,
    )
    .expect("a duplicated anchor resolves to its first occurrence");
    assert_eq!(planned.lines, vec!["x".to_owned(), "two".to_owned()]);
    // The broken duplicate does not propagate: the new set is unique again
    // (survivors and new lines are anchored against the used set).
    let mut seen = std::collections::HashSet::new();
    for h in &planned.hashes {
        assert!(
            seen.insert(h.clone()),
            "duplicate anchor {h} in the new set"
        );
    }
}

#[test]
fn duplicate_anchor_at_a_later_line_is_unreachable_by_reference() {
    // First-occurrence resolution means the second occurrence of a broken
    // set's anchor is only reachable by line number — the same deterministic
    // semantics the reference has (`hashMatches[0]`), asserted here through
    // a range whose start bound is the duplicated anchor.
    let content = "one\ntwo\nthree\n";
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "BBBB".into(), "AAAA".into()]),
        demote: false,
    };
    let planned = plan_edits(
        content,
        line_endings::Ending::Lf,
        &[Edit {
            from: "AAAA".into(),
            to: "BBBB".into(),
            content: "x".into(),
        }],
        &opts,
    )
    .expect("planning against a broken set succeeds");
    // The "from" bound took the FIRST "AAAA" (line 1), so the range is
    // lines 1–2 and the second occurrence (line 3) is left in place.
    assert_eq!(planned.lines, vec!["x".to_owned(), "three".to_owned()]);
    // The broken duplicate does not propagate: the new set is unique again.
    let mut seen = std::collections::HashSet::new();
    for h in &planned.hashes {
        assert!(
            seen.insert(h.clone()),
            "duplicate anchor {h} in the new set"
        );
    }
}

#[test]
fn duplicate_set_still_rejects_unknown_anchors() {
    // The first-occurrence rule changes nothing about staleness: an anchor
    // absent from a (broken) set is Stale, never fuzzy-matched.
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "AAAA".into()]),
        demote: false,
    };
    let err = plan_edits(
        "one\ntwo\n",
        line_endings::Ending::Lf,
        &[Edit {
            from: "ZZZZ".into(),
            to: "ZZZZ".into(),
            content: "x".into(),
        }],
        &opts,
    )
    .unwrap_err();
    assert!(
        matches!(err, EditError::Stale { ref anchor, .. } if anchor == "ZZZZ"),
        "expected Stale, got {err:?}"
    );
}

#[test]
fn ownership_gates_render_their_e_codes() {
    // The ownership gate's typed errors display with their E-codes (the
    // model-facing diagnostics), each with its fix hint.
    let msg = EditError::NotOwned {
        anchor: "XyZa".into(),
        case_hint: " Anchors are case-sensitive; \"XYZA\" differs only in case.".into(),
    }
    .to_string();
    assert!(
        msg.starts_with("[E_STALE_ANCHOR] \"XyZa\" is not owned in this session."),
        "{msg}"
    );
    assert!(
        msg.contains("Anchors are case-sensitive; \"XYZA\" differs only in case."),
        "{msg}"
    );
    assert!(
        msg.ends_with("Call read() on the target file first."),
        "{msg}"
    );

    let msg = EditError::NotOwned {
        anchor: "XyZa".into(),
        case_hint: String::new(),
    }
    .to_string();
    assert!(
        !msg.contains("case-sensitive"),
        "no hint when there is no candidate: {msg}"
    );

    let msg = EditError::CrossFile {
        files: vec!["a.rs".into(), "b.rs".into()],
    }
    .to_string();
    assert!(msg.starts_with("[E_BAD_SHAPE]"), "{msg}");
    assert!(
        msg.contains("owned by different files (a.rs, b.rs)"),
        "{msg}"
    );
    assert!(msg.ends_with("edit one file per call."), "{msg}");

    let msg = EditError::OwnedElsewhere {
        anchors: vec!["AAAA".into()],
        owner: "other.rs".into(),
    }
    .to_string();
    assert!(msg.starts_with("[E_STALE_ANCHOR]"), "{msg}");
    assert!(msg.contains("\"AAAA\" is owned by other.rs"), "{msg}");

    let msg = EditError::OwnedElsewhere {
        anchors: vec!["AAAA".into(), "BBBB".into()],
        owner: "other.rs".into(),
    }
    .to_string();
    assert!(
        msg.contains("\"AAAA\", \"BBBB\" are owned by other.rs"),
        "{msg}"
    );
}
