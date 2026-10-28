//! The `edit` ack's line counts (#301): the ack states what the file did,
//! counted with the tool's own line model on both sides of the subtraction.

use super::impls::{edit, read, write};
use super::tests::{fresh_registry, text};
use super::*;

/// The anchors of a `read` page, in row order.
fn anchors_of(read_out: &str) -> Vec<String> {
    read_out
        .lines()
        .map(|row| row.split('│').next().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_content_preserving_replacement_reports_its_counts_and_leaves_the_bytes_alone() {
    // #301: a file ending in a blank line is three lines to the tool's line
    // model; replacing two of them with the same two lines is 2 added, 2 removed.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "blank.txt", "content": "alpha\nbeta\n\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let page = text(read(
        dir.path(),
        &json!({"path": "blank.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let anchors = anchors_of(&page);
    let res = edit(
        dir.path(),
        &json!({"path": "blank.txt", "edits": [
            {"from": anchors[0].clone(), "to": anchors[1].clone(), "content": "alpha\nbeta"}
        ]}),
        Some(&mut *reg),
        false,
    );
    assert_eq!(
        text(res),
        format!(
            "Successfully edited {}. Added 2 line(s), removed 2 line(s).",
            dir.path().join("blank.txt").display()
        ),
        "a two-line payload over a two-line span"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("blank.txt")).unwrap(),
        "alpha\nbeta\n\n",
        "the bytes the file had are the bytes it has"
    );
}

#[tokio::test]
async fn a_deletion_from_a_file_ending_blank_reports_no_added_lines() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "blank.txt", "content": "alpha\nbeta\n\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let page = text(read(
        dir.path(),
        &json!({"path": "blank.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let anchors = anchors_of(&page);
    let res = edit(
        dir.path(),
        &json!({"path": "blank.txt", "edits": [
            {"from": anchors[1].clone(), "to": anchors[1].clone(), "content": ""}
        ]}),
        Some(&mut *reg),
        false,
    );
    assert_eq!(
        text(res),
        format!(
            "Successfully edited {}. Added 0 line(s), removed 1 line(s).",
            dir.path().join("blank.txt").display()
        ),
        "an empty payload adds nothing, whatever the file's tail looks like"
    );
}

#[tokio::test]
async fn a_replacement_longer_than_its_span_counts_the_payload_lines() {
    // The tool has no pure-insertion form: an edit's range is an inclusive span
    // of anchored lines, so the closest shape is a payload longer than its span.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "grow.txt", "content": "alpha\nbeta\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let page = text(read(
        dir.path(),
        &json!({"path": "grow.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let anchors = anchors_of(&page);
    let res = edit(
        dir.path(),
        &json!({"path": "grow.txt", "edits": [
            {"from": anchors[1].clone(), "to": anchors[1].clone(), "content": "one\ntwo\nthree"}
        ]}),
        Some(&mut *reg),
        false,
    );
    assert_eq!(
        text(res),
        format!(
            "Successfully edited {}. Added 3 line(s), removed 1 line(s).",
            dir.path().join("grow.txt").display()
        ),
        "three payload lines over a one-line span"
    );
}

#[tokio::test]
async fn an_edit_of_an_empty_file_counts_the_one_row_the_tool_showed() {
    // The empty file is the one input where the two line models disagree: `read`
    // shows it as a single blank row, `str::lines()` counts it as none. The ack
    // used `str::lines()` for the old side and saturated the difference, which is
    // the only reason the mixed model never blew up here (#301).
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "empty.txt", "content": ""}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let page = text(read(
        dir.path(),
        &json!({"path": "empty.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    let anchors = anchors_of(&page);
    let res = edit(
        dir.path(),
        &json!({"path": "empty.txt", "edits": [
            {"from": anchors[0].clone(), "to": anchors[0].clone(), "content": "one\ntwo"}
        ]}),
        Some(&mut *reg),
        false,
    );
    assert_eq!(
        text(res),
        format!(
            "Successfully edited {}. Added 2 line(s), removed 1 line(s).",
            dir.path().join("empty.txt").display()
        ),
        "the row the tool showed is the row the batch replaced"
    );
    assert_eq!(
        hashline::split_lines(&std::fs::read_to_string(dir.path().join("empty.txt")).unwrap())
            .len(),
        2,
        "the file gained exactly what the ack said"
    );
}

#[tokio::test]
async fn an_all_demoted_batch_reports_zero_counts() {
    // The zero case of the same arithmetic: a batch whose every member is
    // demoted changed nothing, so it added and removed nothing.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "d.txt", "content": "one\ntwo\nthree\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(edit(
        dir.path(),
        &json!({"path": "d.txt", "edits": [
            {"from": "Zzzz", "to": "Zzzz", "content": "nope"},
            {"from": "Qqqq", "to": "Qqqq", "content": "nope"}
        ]}),
        Some(&mut *reg),
        false,
    ));
    assert!(
        out.starts_with("Successfully edited "),
        "the batch is accepted and demoted: {out}"
    );
    assert!(
        out.contains("Added 0 line(s), removed 0 line(s).") && out.contains("[H_DEMOTED]"),
        "a batch that applied nothing counts nothing: {out}"
    );
}
