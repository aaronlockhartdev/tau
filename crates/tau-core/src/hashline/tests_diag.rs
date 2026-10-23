//! Diagnostics parity tests: the shape-specific and rich diagnostics ported
//! from the reference (G1–G5 of the surface audit).
use super::*;

#[test]
fn bad_ref_diagnostic_names_the_shape() {
    // Parity with the reference's `diagRef`: each malformed shape gets its
    // own repair text, not one generic "expected 4 letters".
    let empty = EditError::BadAnchor("  ".into()).to_string();
    assert!(empty.contains("[E_BAD_REF]"), "{empty}");
    assert!(empty.contains("expected a 4-character anchor"), "{empty}");

    let numbered = EditError::BadAnchor("5".into()).to_string();
    assert!(numbered.contains("no line numbers"), "{numbered}");

    let suffixed = EditError::BadAnchor("Hasu│x".into()).to_string();
    assert!(suffixed.contains("drop everything from"), "{suffixed}");

    let other = EditError::BadAnchor("H1su".into()).to_string();
    assert!(other.contains("expected a 4-character anchor"), "{other}");
}

#[test]
fn bad_ref_diagnostic_unwraps_a_pasted_block() {
    // The block shape: the message names the block's own first/last anchors
    // as the intended from/to, and points the pasted content at `content`.
    let msg = EditError::BadAnchor("AbCd│first line\nXyZw│second line".into()).to_string();
    assert!(msg.contains("Received 2 line(s)"), "{msg}");
    assert!(msg.contains("\"AbCd\" as from"), "{msg}");
    assert!(msg.contains("\"XyZw\" as to"), "{msg}");
    assert!(msg.contains("in content"), "{msg}");
}

#[test]
fn pasted_diff_markers_are_stripped_in_lenient_mode() {
    // The model pasted a diff preview: `+HASH│`/`-HASH│` rows. Lenient mode
    // strips the markers with a `[W_INVALID_PATCH]` warning.
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "BBBB".into()]),
        demote: false,
    };
    let planned = plan_edits(
        "one\ntwo\n",
        line_endings::Ending::Lf,
        &[Edit {
            from: "AAAA".into(),
            to: "BBBB".into(),
            content: "+ CcDd│new one\n- EeFf│new two".into(),
        }],
        &opts,
    )
    .expect("lenient: the markers are stripped, the edit applies");
    assert!(
        planned
            .warnings
            .iter()
            .any(|w| w.contains("[W_INVALID_PATCH]")),
        "{:?}",
        planned.warnings
    );
    assert_eq!(
        planned.lines,
        vec!["new one".to_owned(), "new two".to_owned()]
    );
}

#[test]
fn pasted_diff_markers_are_rejected_in_strict_mode() {
    let opts = PlanOptions {
        strict: true,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "BBBB".into()]),
        demote: false,
    };
    let err = plan_edits(
        "one\ntwo\n",
        line_endings::Ending::Lf,
        &[Edit {
            from: "AAAA".into(),
            to: "BBBB".into(),
            content: "+ CcDd│new one".into(),
        }],
        &opts,
    )
    .unwrap_err();
    assert!(matches!(err, EditError::InvalidPatch { at: 0 }), "{err:?}");
}

#[test]
fn plain_markdown_lists_are_not_diff_markers() {
    // `- item` has no anchor after the dash: it is content, not a pasted
    // diff row. The blanked-anchor form (`-` + 4 spaces + `│`) is the only
    // dash shape we strip.
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into()]),
        demote: false,
    };
    let planned = plan_edits(
        "one\n",
        line_endings::Ending::Lf,
        &[Edit {
            from: "AAAA".into(),
            to: "AAAA".into(),
            content: "- a markdown item\n-    │blanked row".into(),
        }],
        &opts,
    )
    .expect("applies");
    assert_eq!(
        planned.lines,
        vec!["- a markdown item".to_owned(), "blanked row".to_owned()]
    );
    assert!(
        planned
            .warnings
            .iter()
            .any(|w| w.contains("[W_INVALID_PATCH]")),
        "{:?}",
        planned.warnings
    );
}

#[test]
fn stale_diagnostic_carries_the_other_bound_context() {
    // Parity with the reference's `fmtMismatchWithHashes`: one bound stale,
    // the other resolved → the diagnostic shows the resolved bound's ±1-line
    // neighbourhood with fresh anchors.
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "BBBB".into(), "CCCC".into()]),
        demote: false,
    };
    let err = plan_edits(
        "one\ntwo\nthree\n",
        line_endings::Ending::Lf,
        &[Edit {
            from: "ZZZZ".into(),
            to: "BBBB".into(),
            content: "x".into(),
        }],
        &opts,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("[E_STALE_ANCHOR]"), "{msg}");
    assert!(
        msg.contains("Current context around resolved anchor \"BBBB\" (line 2):"),
        "{msg}"
    );
    assert!(msg.contains("1: AAAA│one"), "{msg}");
    assert!(msg.contains("2: BBBB│two"), "{msg}");
    assert!(msg.contains("3: CCCC│three"), "{msg}");
}

#[test]
fn strict_mode_still_rejects_the_batch_on_a_stale_bound() {
    // With the early reject out of lenient_fixes, strict mode gets its
    // atomic batch rejection from the resolution phase: one stale member
    // rejects the whole batch (the valid member is not applied).
    let opts = PlanOptions {
        strict: true,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into(), "BBBB".into(), "CCCC".into()]),
        demote: false,
    };
    let err = plan_edits(
        "one\ntwo\nthree\n",
        line_endings::Ending::Lf,
        &[
            Edit {
                from: "AAAA".into(),
                to: "AAAA".into(),
                content: "ok".into(),
            },
            Edit {
                from: "ZZZZ".into(),
                to: "BBBB".into(),
                content: "x".into(),
            },
        ],
        &opts,
    )
    .unwrap_err();
    assert!(matches!(err, EditError::Stale { .. }), "{err:?}");
}

#[test]
fn literal_unicode_escape_in_content_is_flagged() {
    // Parity with the reference's `warnUnicodeEsc`: a literal `\uDDDD` left
    // in the content is a paste artifact — a warning, never an autocorrection.
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: None,
        hashes: Some(vec!["AAAA".into()]),
        demote: false,
    };
    let planned = plan_edits(
        "one\n",
        line_endings::Ending::Lf,
        &[Edit {
            from: "AAAA".into(),
            to: "AAAA".into(),
            content: "ab\\uDDDD".into(),
        }],
        &opts,
    )
    .expect("applies");
    assert!(
        planned
            .warnings
            .iter()
            .any(|w| w.contains("Detected literal \\uDDDD")),
        "{:?}",
        planned.warnings
    );
}

#[test]
fn range_stale_cap_hint_names_the_offset() {
    // Parity with the reference's `assertRangeServed` cap: over
    // MAX_RANGE_STALE_LINES, the hint says how many lines the range has, how
    // many are shown, and the exact offset to continue with.
    let n = MAX_RANGE_STALE_LINES + 5;
    let mut content = String::new();
    for i in 0..n {
        use std::fmt::Write;
        let _ = writeln!(content, "line{i}");
    }
    let hashes = line_hashes(&content).expect("hashes");
    // A served baseline that no longer matches: every checksum wrong.
    let served: HashMap<String, String> = hashes
        .iter()
        .zip(content.lines())
        .map(|(h, l)| (h.clone(), format!("stale-{l}")))
        .collect();
    let opts = PlanOptions {
        strict: false,
        path: "a.txt".into(),
        served: Some(served),
        hashes: Some(hashes.clone()),
        demote: false,
    };
    let err = plan_edits(
        &content,
        line_endings::Ending::Lf,
        &[Edit {
            from: hashes[0].clone(),
            to: hashes[n - 1].clone(),
            content: "x".into(),
        }],
        &opts,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("[E_RANGE_STALE]"), "{msg}");
    assert!(
        msg.contains(&format!(
            "[The range has {n} lines; showing the first {MAX_RANGE_STALE_LINES}. Call read() on a.txt with offset={} to see the rest.]",
            MAX_RANGE_STALE_LINES + 1
        )),
        "{msg}"
    );
}
