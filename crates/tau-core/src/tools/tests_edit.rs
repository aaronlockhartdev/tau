use super::impls::{edit, read, write};
use super::tests::{detail, fresh_registry, text};
use super::*;
use std::fmt::Write as _;

#[tokio::test]
async fn stale_anchor_diagnostic_does_not_touch_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "s.txt", "content": "keep\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(edit(
        dir.path(),
        &json!({"path": "s.txt", "edits": [{"from": "zzzz", "to": "zzzz", "content": "x"}]}),
        Some(&mut *reg),
        false,
    ));
    assert!(
        out.contains("stale anchor") || out.contains("E_STALE_ANCHOR"),
        "{out}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("s.txt")).unwrap(),
        "keep\n"
    );
}

#[tokio::test]
async fn crlf_files_preserve_their_line_endings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    let mut reg = fresh_registry();
    let out = text(read(
        dir.path(),
        &json!({"path": "w.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    // The read normalizes for the model (no \r in the anchor rows).
    assert!(!out.contains('\r'), "{out}");
    let rows: Vec<&str> = out.lines().collect();
    let anchor = rows[0].split('│').next().unwrap().to_string();
    edit(
        dir.path(),
        &json!({"path": "w.txt", "edits": [{"from": anchor, "to": anchor, "content": "ONE"}]}),
        Some(&mut *reg),
        false,
    );
    // The written file keeps its CRLF endings — untouched lines included.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("w.txt")).unwrap(),
        "ONE\r\ntwo\r\nthree\r\n"
    );
}

#[tokio::test]
async fn edit_output_is_bounded_to_the_changed_region() {
    let dir = tempfile::tempdir().unwrap();
    let mut content = String::new();
    for i in 0..50 {
        let _ = writeln!(content, "line{i}");
    }
    write(
        dir.path(),
        &json!({"path": "big.txt", "content": content.as_str()}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "big.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let anchor = lines[24].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "big.txt", "edits": [{"from": anchor, "to": anchor, "content": "LINE25"}]}),
        Some(&mut *reg),
        false,
    );
    // #243: the diff rides the detail channel; the model sees the ack.
    assert!(
        text(res.clone()).starts_with("Successfully edited "),
        "edit failed: {res:?}"
    );
    // The changed region plus two context lines per side — never the 50-line file.
    let out = detail(res).expect("an edit success carries its diff in ui_detail");
    let shown = out.lines().count() - 1;
    assert!(shown <= 7, "output had {shown} rows: {out}");
    assert!(out.contains("LINE25"), "{out}");
}

#[tokio::test]
async fn edit_pure_deletion_reports_the_removed_line_not_no_change() {
    // #235: a deletion-only edit wrote the file correctly but reported
    // "(no change)" — the diff was blank. It must show the removed line.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "del.txt", "content": "alpha\nbeta\ngamma\ndelta\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "del.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    // Delete "beta" (the second line).
    let anchor = lines[1].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "del.txt", "edits": [{"from": anchor, "to": anchor, "content": ""}]}),
        Some(&mut *reg),
        false,
    );
    // #243: the model ack counts the change; the diff keeps the removed line.
    assert_eq!(
        text(res.clone()),
        format!(
            "Successfully edited {}. Added 0 line(s), removed 1 line(s).",
            dir.path().join("del.txt").display()
        )
    );
    let out = detail(res).expect("an edit success carries its diff in ui_detail");
    assert!(
        !out.contains("(no change)"),
        "a pure deletion must not report no change: {out}"
    );
    assert!(
        out.contains("-beta"),
        "removed line missing from the diff: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("del.txt")).unwrap(),
        "alpha\ngamma\ndelta\n"
    );
}

#[tokio::test]
async fn edit_output_is_a_bounded_before_after_diff() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "d.txt", "content": "alpha\nbeta\ngamma\ndelta\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "d.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let from = lines[1].split('│').next().unwrap().to_string();
    let to = lines[2].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "d.txt", "edits": [{"from": from, "to": to, "content": "BETA'\nGAMMA'"}]}),
        Some(&mut *reg),
        false,
    );
    // #243: the model gets the ack; the bounded diff rides the detail channel.
    assert_eq!(
        text(res.clone()),
        format!(
            "Successfully edited {}. Added 2 line(s), removed 2 line(s).",
            dir.path().join("d.txt").display()
        )
    );
    // The one-line summary, then a bounded body of marked lines.
    let out = detail(res).expect("an edit success carries its diff in ui_detail");
    assert!(
        out.starts_with("edited ") && out.contains("(lines 2–3 of 4)"),
        "summary: {out}"
    );
    let body: Vec<&str> = out.lines().skip(1).collect();
    // Removed lines are '-'-prefixed (no anchor — they are gone).
    assert!(body.contains(&"-beta"), "missing -beta: {out}");
    assert!(body.contains(&"-gamma"), "missing -gamma: {out}");
    // Added lines are '+ <fresh-anchor>│<line>'.
    assert!(
        body.iter()
            .any(|l| l.starts_with("+ ") && l.ends_with("BETA'")),
        "added line missing its fresh anchor: {out}"
    );
    assert!(
        body.iter()
            .any(|l| l.starts_with("+ ") && l.ends_with("GAMMA'")),
        "added line missing its fresh anchor: {out}"
    );
    // Unchanged neighbours are ' <line>' context.
    assert!(body.contains(&" alpha"), "missing context: {out}");
    assert!(body.contains(&" delta"), "missing context: {out}");
    assert!(body.len() <= 8, "diff not bounded to the region: {out}");
}

#[tokio::test]
async fn edit_diff_over_the_cap_still_renders_the_head() {
    // #220: a change over the diff cap shows the head plus a truncation
    // marker — the diff is never blanked out.
    let dir = tempfile::tempdir().unwrap();
    let content: String = (0..300)
        .map(|i| format!("old {i}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write(
        dir.path(),
        &json!({"path": "big.txt", "content": content}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "big.txt", "offset": "1", "limit": 220}),
        None,
        None,
        Some(&mut *reg),
    ));
    let rows: Vec<&str> = read_out.lines().collect();
    let from = rows[0].split('│').next().unwrap().to_string();
    let to = rows[219].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "big.txt", "edits": [{"from": from, "to": to, "content": "NEW"}]}),
        Some(&mut *reg),
        true,
    );
    // #243: the model gets the ack; the capped diff rides the detail channel.
    assert!(
        text(res.clone()).starts_with("Successfully edited "),
        "edit failed: {res:?}"
    );
    let out = detail(res).expect("an edit success carries its diff in ui_detail");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.len() > 201,
        "head of the diff missing: {} lines",
        lines.len()
    );
    // The truncation marker names how much is hidden.
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("… ") && l.contains("more lines changed")),
        "truncation notice missing: {out}"
    );
    // Every visible body line is a complete marked line (never partial).
    assert!(
        lines[1..].iter().all(|l| l.starts_with('+')
            || l.starts_with('-')
            || l.starts_with(' ')
            || l.starts_with('…')),
        "a partial line slipped in: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("big.txt"))
            .unwrap()
            .lines()
            .count(),
        81
    );
}

#[tokio::test]
async fn edit_array_form_applies_all_edits() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "m.txt", "content": "one\ntwo\nthree\nfour\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "m.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let a0 = lines[0].split('│').next().unwrap().to_string();
    let a2 = lines[2].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "m.txt", "edits": [
            {"from": a0, "to": a0, "content": "ONE"},
            {"from": a2, "to": a2, "content": "THREE"}
        ]}),
        Some(&mut *reg),
        false,
    );
    // #243: the ack counts net change; the diff (in ui_detail) shows both.
    assert_eq!(
        text(res.clone()),
        format!(
            "Successfully edited {}. Added 2 line(s), removed 2 line(s).",
            dir.path().join("m.txt").display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.txt")).unwrap(),
        "ONE\ntwo\nTHREE\nfour\n"
    );
}

#[tokio::test]
async fn edit_overlapping_ranges_demotes_the_colliding_member() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "o.txt", "content": "one\ntwo\nthree\nfour\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "o.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let a0 = lines[0].split('│').next().unwrap().to_string();
    let a1 = lines[1].split('│').next().unwrap().to_string();
    // Demotion is the default posture (#207 §5): the non-overlapping member
    // applies, the colliding one is demoted and reported with its reason.
    let res = edit(
        dir.path(),
        &json!({"path": "o.txt", "edits": [
            {"from": a0, "to": a1, "content": "A"},
            {"from": a1, "to": a1, "content": "B"}
        ]}),
        Some(&mut *reg),
        false,
    );
    // #243: the demotion notice rides the model ack — the model needs it to
    // retry the skipped member; the diff is UI-only.
    let out = text(res);
    assert!(
        out.starts_with("Successfully edited "),
        "the valid member applies under demotion: {out}"
    );
    assert!(
        out.contains("[H_DEMOTED]") && (out.contains("overlap") || out.contains("E_OVERLAPPING")),
        "the colliding member is demoted with its reason: {out}"
    );
    // The first member's change is on disk; the demoted second is not.
    let on_disk = std::fs::read_to_string(dir.path().join("o.txt")).unwrap();
    assert!(
        on_disk.contains('A'),
        "the valid member is written: {on_disk}"
    );
    assert!(
        !on_disk.contains("B\n"),
        "the demoted member is not: {on_disk}"
    );
}

#[tokio::test]
async fn edit_batch_with_one_valid_and_one_invalid_demotes_the_invalid() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "d.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "d.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let a0 = lines[0].split('│').next().unwrap().to_string();
    let a2 = lines[2].split('│').next().unwrap().to_string();
    // One member targets a real anchor; the other targets a stale (unknown)
    // anchor. Demotion applies the valid member and reports the invalid one.
    let res = edit(
        dir.path(),
        &json!({"path": "d.txt", "edits": [
            {"from": a0, "to": a0, "content": "ONE!"},
            {"from": "Zzzz", "to": a2, "content": "nope"}
        ]}),
        Some(&mut *reg),
        false,
    );
    // #243: the demotion notice rides the model ack.
    let out = text(res);
    assert!(
        out.starts_with("Successfully edited "),
        "valid member applies: {out}"
    );
    assert!(
        out.contains("[H_DEMOTED]") && out.contains("E_STALE_ANCHOR"),
        "invalid member is demoted with a stale-anchor reason: {out}"
    );
    let on_disk = std::fs::read_to_string(dir.path().join("d.txt")).unwrap();
    assert!(on_disk.contains("ONE!"), "valid change on disk: {on_disk}");
    assert!(
        !on_disk.contains("nope"),
        "invalid change not on disk: {on_disk}"
    );
}

#[tokio::test]
async fn edit_anchored_content_is_stripped_in_lenient_mode() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let from = lines[0].split('│').next().unwrap().to_string();
    let to = lines[1].split('│').next().unwrap().to_string();
    // The model pasted the read output verbatim (HASH│ prefixes on every line).
    let anchored = format!("{from}│one\n{to}│two");
    let res = edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [{"from": from, "to": to, "content": anchored}]}),
        Some(&mut *reg),
        false,
    );
    // #243: the lenient-fix warning rides the model ack.
    let out = text(res);
    // Lenient (the default): the prefixes are stripped, the edit applies.
    assert!(
        out.starts_with("Successfully edited "),
        "lenient edit failed: {out}"
    );
    assert!(out.contains("W_BARE_HASH_PREFIX"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[tokio::test]
async fn edit_anchored_content_is_rejected_in_strict_mode() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let from = lines[0].split('│').next().unwrap().to_string();
    let to = lines[1].split('│').next().unwrap().to_string();
    let anchored = format!("{from}│one\n{to}│two");
    let out = text(edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [{"from": from, "to": to, "content": anchored}], "strict": true}),
        Some(&mut *reg),
        true,
    ));
    assert!(
        !out.starts_with("edited"),
        "strict edit must be rejected: {out}"
    );
    assert!(out.contains("E_ANCHORED_CONTENT"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[tokio::test]
async fn reversed_range_is_swapped_in_lenient_mode() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "r.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "r.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let a0 = lines[0].split('│').next().unwrap().to_string();
    let a1 = lines[1].split('│').next().unwrap().to_string();
    // from after to: lenient swaps the range, with a warning.
    let res = edit(
        dir.path(),
        &json!({"path": "r.txt", "edits": [{"from": a1, "to": a0, "content": "TWO\nONE"}]}),
        Some(&mut *reg),
        false,
    );
    // #243: the lenient-fix warning rides the model ack.
    let out = text(res);
    assert!(
        out.starts_with("Successfully edited "),
        "lenient swap failed: {out}"
    );
    assert!(out.contains("W_RANGE_REVERSED"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("r.txt")).unwrap(),
        "TWO\nONE\nthree\n"
    );
}

#[tokio::test]
async fn reversed_range_is_rejected_in_strict_mode() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "r.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let read_out = text(read(
        dir.path(),
        &json!({"path": "r.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let a0 = lines[0].split('│').next().unwrap().to_string();
    let a1 = lines[1].split('│').next().unwrap().to_string();
    let out = text(edit(
        dir.path(),
        &json!({"path": "r.txt", "edits": [{"from": a1, "to": a0, "content": "x"}], "strict": true}),
        Some(&mut *reg),
        true,
    ));
    assert!(
        !out.starts_with("edited"),
        "strict reversed range must be rejected: {out}"
    );
    assert!(out.contains("E_RANGE_REVERSED"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("r.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[tokio::test]
async fn range_stale_shows_fresh_anchors_and_retry_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "s.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    // The session reads the file and is served the anchors.
    let read_out = text(read(
        dir.path(),
        &json!({"path": "s.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let lines: Vec<&str> = read_out.lines().collect();
    let from = lines[0].split('│').next().unwrap().to_string();
    let to = lines[2].split('│').next().unwrap().to_string();
    // The file drifts out from under the session (a bash edit).
    std::fs::write(dir.path().join("s.txt"), "one\nTWO-changed\nthree\n").unwrap();
    // Now an edit against the served range is stale. Under demotion (#207 §5)
    // the stale member is *demoted* — reported with its range-stale reason and
    // fresh anchors inline — and the (empty) remainder of the batch is applied,
    // leaving the file untouched.
    let err = text(edit(
        dir.path(),
        &json!({"path": "s.txt", "edits": [{"from": from, "to": to, "content": "X"}]}),
        Some(&mut *reg),
        false,
    ));
    assert!(
        err.contains("[H_DEMOTED]") && err.contains("E_RANGE_STALE"),
        "expected the demoted member to carry the range-stale reason: {err}"
    );
    assert!(
        err.contains("fresh anchors"),
        "range-stale must show fresh anchors: {err}"
    );
    // The file is untouched.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("s.txt")).unwrap(),
        "one\nTWO-changed\nthree\n"
    );
}

#[tokio::test]
async fn an_edit_error_is_never_an_empty_result() {
    // #220: every EditError surfaces its message — a tool result must never
    // carry zero visible text (the transcript's blank code block).
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "e.txt", "content": "one\ntwo\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    // Bad anchor (not in the file).
    let out = text(edit(
        dir.path(),
        &json!({"path": "e.txt", "edits": [{"from": "1234", "to": "1234", "content": "x"}], "strict": true}),
        Some(&mut *reg),
        true,
    ));
    assert!(
        !out.trim().is_empty(),
        "an edit failure must carry its message"
    );
    assert!(out.contains("E_BAD_ANCHOR"), "{out}");
    // Stale anchor: the file changed after the read.
    let read_out = text(read(
        dir.path(),
        &json!({"path": "e.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let anchor = read_out
        .lines()
        .next()
        .unwrap()
        .split('│')
        .next()
        .unwrap()
        .to_string();
    std::fs::write(dir.path().join("e.txt"), "changed\n").unwrap();
    let out = text(edit(
        dir.path(),
        &json!({"path": "e.txt", "edits": [{"from": anchor, "to": anchor, "content": "x"}]}),
        Some(&mut *reg),
        false,
    ));
    assert!(
        !out.trim().is_empty(),
        "a stale anchor must carry its message"
    );
    assert!(out.contains("E_STALE"), "{out}");
    // Missing fields.
    let out = text(edit(
        dir.path(),
        &json!({"path": "e.txt", "edits": [{"from": "aaaa"}]}),
        Some(&mut *reg),
        false,
    ));
    assert!(
        !out.trim().is_empty(),
        "a missing-field failure must carry its message"
    );
    assert!(out.contains("requires"), "{out}");
}

#[tokio::test]
async fn edit_with_unowned_anchor_is_a_typed_ownership_error() {
    // Parity with the reference's `resolveEditTarget`: for a file the
    // registry knows, an anchor the session does not own is rejected before
    // planning, with the reference's message — not the "stale, re-read"
    // message (that one is for owned anchors that no longer match).
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    );
    let out = text(edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [{"from": "Qqqq", "to": "Qqqq", "content": "x"}]}),
        Some(&mut *reg),
        false,
    ));
    assert!(
        out.contains("[E_STALE_ANCHOR] \"Qqqq\" is not owned in this session."),
        "{out}"
    );
    assert!(
        out.contains("Call read() on the target file first."),
        "{out}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\ntwo\n",
        "nothing was written"
    );
}

#[tokio::test]
async fn edit_unowned_anchor_case_hint_names_the_owned_variant() {
    // A case-mangled anchor is pointed at the owned variant it differs
    // from (the reference's case-folding hint) — a one-token fix, not a
    // re-read.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let owned = out
        .lines()
        .next()
        .unwrap()
        .split('│')
        .next()
        .unwrap()
        .to_string();
    let ref_ = if owned.chars().any(char::is_lowercase) {
        owned.to_uppercase()
    } else {
        owned.to_lowercase()
    };
    assert_ne!(ref_, owned);
    let out = text(edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [{"from": ref_, "to": ref_, "content": "x"}]}),
        Some(&mut *reg),
        false,
    ));
    assert!(out.contains("not owned in this session"), "{out}");
    assert!(
        out.contains(&format!(
            "Anchors are case-sensitive; \"{owned}\" differs only in case."
        )),
        "{out}"
    );
}

#[tokio::test]
async fn edit_cross_file_anchors_are_a_bad_shape() {
    // The from bound is owned by a.txt, the to bound by b.txt: one file per
    // call (parity with the reference's `resolveEditTarget`). Strict mode:
    // the whole batch rejects, nothing is written.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\n"}),
        None,
    )
    .await;
    write(
        dir.path(),
        &json!({"path": "b.txt", "content": "two\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let ra = text(read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let rb = text(read(
        dir.path(),
        &json!({"path": "b.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let from = ra
        .lines()
        .next()
        .unwrap()
        .split('│')
        .next()
        .unwrap()
        .to_string();
    let to = rb
        .lines()
        .next()
        .unwrap()
        .split('│')
        .next()
        .unwrap()
        .to_string();
    let out = text(edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [{"from": from, "to": to, "content": "x"}]}),
        Some(&mut *reg),
        true,
    ));
    assert!(out.contains("[E_BAD_SHAPE]"), "{out}");
    assert!(out.contains("owned by different files"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
}

#[tokio::test]
async fn edit_batch_demotes_unowned_members_and_applies_the_rest() {
    // Demotion mode (the default): a member with an unowned anchor is
    // reported with its typed error on the demotion channel, and the valid
    // members apply.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let rows: Vec<&str> = out.lines().collect();
    let from = rows[0].split('│').next().unwrap().to_string();
    let to = rows[1].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [
            {"from": from, "to": to, "content": "ONE AND TWO"},
            {"from": "Qqqq", "to": "Qqqq", "content": "nope"}
        ]}),
        Some(&mut *reg),
        false,
    );
    let out = text(res);
    assert!(out.starts_with("Successfully edited"), "{out}");
    assert!(out.contains("[H_DEMOTED] edits[1] not applied:"), "{out}");
    assert!(out.contains("not owned in this session"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "ONE AND TWO\n"
    );
}

#[tokio::test]
async fn edit_demotion_labels_use_the_original_batch_indices() {
    // A batch with a gated member (skipped before planning) and an internally
    // demoted one (overlapping): the [H_DEMOTED] labels must use the model's
    // original edit indices, not the filtered list's.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\nthree\nfour\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(read(
        dir.path(),
        &json!({"path": "a.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let rows: Vec<&str> = out.lines().collect();
    let anchor = |i: usize| rows[i].split('│').next().unwrap().to_string();
    let res = edit(
        dir.path(),
        &json!({"path": "a.txt", "edits": [
            {"from": anchor(0), "to": anchor(1), "content": "X"},
            {"from": "Qqqq", "to": "Qqqq", "content": "nope"},
            {"from": anchor(1), "to": anchor(2), "content": "Y"}
        ]}),
        Some(&mut *reg),
        false,
    );
    let out = text(res);
    assert!(out.starts_with("Successfully edited"), "{out}");
    assert!(
        out.contains("[H_DEMOTED] edits[1] not applied: [E_STALE_ANCHOR]"),
        "the gated member keeps its original index: {out}"
    );
    assert!(
        out.contains("[H_DEMOTED] edits[2] not applied: [E_OVERLAPPING]"),
        "the overlapping member keeps its original index: {out}"
    );
    assert!(!out.contains("[H_DEMOTED] edits[0]"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "X\nthree\nfour\n",
        "only the valid member applies"
    );
}
