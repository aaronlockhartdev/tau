use super::*;

#[tokio::test]
async fn write_then_read_roundtrip_with_anchors() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\n"}),
    )
    .await;
    let out = read(dir.path(), &json!({"path": "a.txt"})).await;
    let rows: Vec<&str> = out.lines().collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].ends_with("one"));
    assert!(rows[1].ends_with("two"));
    let from = rows[0].split('│').next().unwrap().to_string();
    let to = rows[1].split('│').next().unwrap().to_string();
    let out = edit(
        dir.path(),
        &json!({"path": "a.txt", "from": from, "to": to, "content": "ONE AND TWO"}),
    )
    .await;
    assert!(out.starts_with("edited"), "edit failed: {out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "ONE AND TWO\n"
    );
}

#[tokio::test]
async fn read_pages_with_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "p.txt", "content": "l1\nl2\nl3\nl4\n"}),
    )
    .await;
    let out = read(
        dir.path(),
        &json!({"path": "p.txt", "offset": 2, "limit": 2}),
    )
    .await;
    assert!(out.ends_with("lines 2–3 of 4)"), "{out}");
    assert!(out.contains("l2") && out.contains("l3"));
    assert!(!out.contains("l1") && !out.contains("l4"));
}

#[tokio::test]
async fn stale_anchor_diagnostic_does_not_touch_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &json!({"path": "s.txt", "content": "keep\n"})).await;
    let out = edit(
        dir.path(),
        &json!({"path": "s.txt", "from": "zzz", "to": "zzz", "content": "x"}),
    )
    .await;
    assert!(out.contains("stale anchor"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("s.txt")).unwrap(),
        "keep\n"
    );
}

#[tokio::test]
async fn crlf_files_are_normalized_through_read_and_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    let out = read(dir.path(), &json!({"path": "w.txt"})).await;
    assert!(!out.contains('\r'), "{out}");
    let rows: Vec<&str> = out.lines().collect();
    let anchor = rows[0].split('│').next().unwrap().to_string();
    edit(
        dir.path(),
        &json!({"path": "w.txt", "from": anchor, "to": anchor, "content": "ONE"}),
    )
    .await;
    // The written file is consistently LF — untouched lines included.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("w.txt")).unwrap(),
        "ONE\ntwo\nthree\n"
    );
}

#[tokio::test]
async fn bash_timeout_kills_the_child_not_just_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let out = bash(
        dir.path(),
        &json!({"command": "sleep 5; touch marker", "timeout_secs": 1}),
    )
    .await;
    assert!(out.contains("timed out after 1s"), "{out}");
    // If the child survived the timeout it would finish its sleep and
    // write the marker; give it time to prove it is dead.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!marker.exists(), "the timed-out child kept running");
}

#[tokio::test]
async fn edit_output_is_bounded_to_the_changed_region() {
    let dir = tempfile::tempdir().unwrap();
    let content: String = (0..50).map(|i| format!("line{i}\n")).collect();
    write(
        dir.path(),
        &json!({"path": "big.txt", "content": content.as_str()}),
    )
    .await;
    let read_out = read(dir.path(), &json!({"path": "big.txt"})).await;
    let lines: Vec<&str> = read_out.lines().collect();
    let anchor = lines[24].split('│').next().unwrap().to_string();
    let out = edit(
        dir.path(),
        &json!({"path": "big.txt", "from": anchor, "to": anchor, "content": "LINE25"}),
    )
    .await;
    // The changed region plus two context lines per side — never the 50-line file.
    let shown = out.lines().count() - 1;
    assert!(shown <= 7, "output had {shown} rows: {out}");
    assert!(out.contains("LINE25"), "{out}");
}

#[tokio::test]
async fn bash_captures_output_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let out = bash(dir.path(), &json!({"command": "echo hello"})).await;
    assert!(out.starts_with("exit 0"), "{out}");
    assert!(out.contains("hello"), "{out}");
    let out = bash(dir.path(), &json!({"command": "echo oops >&2; exit 3"})).await;
    assert!(out.starts_with("exit 3"), "{out}");
    assert!(out.contains("oops"), "{out}");
}
