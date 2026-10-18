use super::impls::{bash, bash_inner, edit, image_media_type, read, write};
use super::*;
use base64::Engine;
use std::fmt::Write as _;

/// The text of a tool output: these tests assert on text, and an image
/// block on these paths is a bug.
fn text(out: ToolResult) -> String {
    match out.output {
        ToolOutput::Text(t) => t,
        ToolOutput::Image { .. } => panic!("expected text, got an image block"),
    }
}
#[tokio::test]
async fn write_then_read_roundtrip_with_anchors() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "a.txt", "content": "one\ntwo\n"}),
    )
    .await;
    let out = text(read(dir.path(), &json!({"path": "a.txt"}), None).await);
    let rows: Vec<&str> = out.lines().collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].ends_with("one"));
    assert!(rows[1].ends_with("two"));
    let from = rows[0].split('│').next().unwrap().to_string();
    let to = rows[1].split('│').next().unwrap().to_string();
    let out = text(
        edit(
            dir.path(),
            &json!({"path": "a.txt", "from": from, "to": to, "content": "ONE AND TWO"}),
        )
        .await,
    );
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
    let out = text(
        read(
            dir.path(),
            &json!({"path": "p.txt", "offset": 2, "limit": 2}),
            None,
        )
        .await,
    );
    assert!(out.ends_with("lines 2–3 of 4)"), "{out}");
    assert!(out.contains("l2") && out.contains("l3"));
    assert!(!out.contains("l1") && !out.contains("l4"));
}

#[tokio::test]
async fn stale_anchor_diagnostic_does_not_touch_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &json!({"path": "s.txt", "content": "keep\n"})).await;
    let out = text(
        edit(
            dir.path(),
            &json!({"path": "s.txt", "from": "zzz", "to": "zzz", "content": "x"}),
        )
        .await,
    );
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
    let out = text(read(dir.path(), &json!({"path": "w.txt"}), None).await);
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
    let out = text(
        bash(
            dir.path(),
            &json!({"command": "sleep 5; touch marker", "timeout_secs": 1}),
        )
        .await,
    );
    assert!(out.contains("timed out after 1s"), "{out}");
    // If the child survived the timeout it would finish its sleep and
    // write the marker; give it time to prove it is dead.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!marker.exists(), "the timed-out child kept running");
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
    )
    .await;
    let read_out = text(read(dir.path(), &json!({"path": "big.txt"}), None).await);
    let lines: Vec<&str> = read_out.lines().collect();
    let anchor = lines[24].split('│').next().unwrap().to_string();
    let out = text(
        edit(
            dir.path(),
            &json!({"path": "big.txt", "from": anchor, "to": anchor, "content": "LINE25"}),
        )
        .await,
    );
    // The changed region plus two context lines per side — never the 50-line file.
    let shown = out.lines().count() - 1;
    assert!(shown <= 7, "output had {shown} rows: {out}");
    assert!(out.contains("LINE25"), "{out}");
}

#[tokio::test]
async fn bash_captures_output_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let out = text(bash(dir.path(), &json!({"command": "echo hello"})).await);
    assert!(out.starts_with("exit 0"), "{out}");
    assert!(out.contains("hello"), "{out}");
    let out = text(bash(dir.path(), &json!({"command": "echo oops >&2; exit 3"})).await);
    assert!(out.starts_with("exit 3"), "{out}");
    assert!(out.contains("oops"), "{out}");
}

#[tokio::test]
async fn bash_child_path_includes_common_tool_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let out = text(bash(dir.path(), &json!({"command": "echo $PATH"})).await);
    // bash_path() augments the GUI-minimal inherited PATH with the common
    // per-user tool dirs, so bare commands resolve (#172).
    assert!(out.contains("/usr/local/bin"), "{out}");
    assert!(out.contains("/opt/homebrew/bin"), "{out}");
}

#[tokio::test]
async fn bash_stop_interrupts_a_running_command() {
    let dir = tempfile::tempdir().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopper = std::sync::Arc::clone(&stop);
    let setter = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stopper.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let out = text(bash_inner(dir.path(), &json!({"command": "sleep 10"}), &stop).await);
    let elapsed = started.elapsed();
    // The stop cut the command short, not the sleep running to completion (#174).
    assert!(out.contains("interrupted by user stop"), "{out}");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "stop did not interrupt in time: {elapsed:?}"
    );
    setter.await.unwrap();
}

#[test]
fn image_detection_by_magic_bytes() {
    assert_eq!(image_media_type(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
    assert_eq!(image_media_type(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
    assert_eq!(image_media_type(b"GIF89a"), Some("image/gif"));
    assert_eq!(
        image_media_type(b"RIFF\x08\x00\x00\x00WEBP"),
        Some("image/webp")
    );
    assert_eq!(
        image_media_type(b"BM\x36\x00\x00\x00\x00\x00\x00\x00\x36\x00\x00\x00"),
        Some("image/bmp")
    );
    assert_eq!(image_media_type(b"II\x2a\x00"), Some("image/tiff"));
    assert_eq!(image_media_type(b"MM\x00\x2a"), Some("image/tiff"));
    // A text file that merely starts with the BMP prefix is not an image.
    assert_eq!(image_media_type(b"BM not a bitmap, just text"), None);
    assert_eq!(image_media_type(b"hello\nworld\n"), None);
    assert_eq!(image_media_type(b""), None);
}

#[tokio::test]
async fn read_returns_an_image_block_for_a_png() {
    let dir = tempfile::tempdir().unwrap();
    let png = b"\x89PNG\r\n\x1a\nfake-png-pixels";
    std::fs::write(dir.path().join("shot.png"), png).unwrap();
    match read(dir.path(), &json!({"path": "shot.png"}), None)
        .await
        .output
    {
        ToolOutput::Image(img) => {
            assert_eq!(img.kind, "image");
            assert_eq!(img.media_type, "image/png");
            // The block carries the exact file bytes, base64-encoded.
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(&img.data_base64)
                    .unwrap(),
                png.to_vec()
            );
        }
        ToolOutput::Text(t) => panic!("expected an image block, got text: {t}"),
    }
}

#[tokio::test]
async fn read_returns_an_image_block_for_a_jpeg() {
    let dir = tempfile::tempdir().unwrap();
    let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF";
    std::fs::write(dir.path().join("shot.jpg"), jpeg).unwrap();
    match read(dir.path(), &json!({"path": "shot.jpg"}), None)
        .await
        .output
    {
        ToolOutput::Image(img) => assert_eq!(img.media_type, "image/jpeg"),
        ToolOutput::Text(t) => panic!("expected an image block, got text: {t}"),
    }
}

#[tokio::test]
async fn an_oversized_image_is_refused_loudly() {
    let dir = tempfile::tempdir().unwrap();
    // A PNG past the cap: the 8-byte signature plus 200 padding bytes.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(png.len() + 200, 0);
    std::fs::write(dir.path().join("big.png"), &png).unwrap();
    // Over the cap: a loud text refusal, not an image block.
    let out = text(read(dir.path(), &json!({"path": "big.png"}), Some(100)).await);
    assert!(out.contains("over the 100-byte cap"), "{out}");
    // At the cap: the image block comes through.
    match read(
        dir.path(),
        &json!({"path": "big.png"}),
        Some(u64::try_from(png.len()).expect("fixture size, well under u64::MAX")),
    )
    .await
    .output
    {
        ToolOutput::Image(img) => assert_eq!(img.media_type, "image/png"),
        ToolOutput::Text(t) => panic!("expected an image block, got text: {t}"),
    }
}

#[tokio::test]
async fn edit_output_is_a_bounded_before_after_diff() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "d.txt", "content": "alpha\nbeta\ngamma\ndelta\n"}),
    )
    .await;
    let read_out = text(read(dir.path(), &json!({"path": "d.txt"}), None).await);
    let lines: Vec<&str> = read_out.lines().collect();
    let from = lines[1].split('│').next().unwrap().to_string();
    let to = lines[2].split('│').next().unwrap().to_string();
    let out = text(
        edit(
            dir.path(),
            &json!({"path": "d.txt", "from": from, "to": to, "content": "BETA'\nGAMMA'"}),
        )
        .await,
    );
    // The one-line summary, then a bounded body of marked lines.
    assert!(
        out.starts_with("edited ") && out.contains("(lines 2–3 of 4)"),
        "summary: {out}"
    );
    let body: Vec<&str> = out.lines().skip(1).collect();
    // Removed lines are '-'-prefixed (no anchor — they are gone).
    assert!(body.contains(&"-beta"), "missing -beta: {out}");
    assert!(body.contains(&"-gamma"), "missing -gamma: {out}");
    // Added lines are '+ <fresh-anchor> <line>'.
    assert!(
        body.iter()
            .any(|l| l.starts_with("+ ") && l.ends_with(" BETA'")),
        "added line missing its fresh anchor: {out}"
    );
    assert!(
        body.iter()
            .any(|l| l.starts_with("+ ") && l.ends_with(" GAMMA'")),
        "added line missing its fresh anchor: {out}"
    );
    // Unchanged neighbours are ' <line>' context.
    assert!(body.contains(&" alpha"), "missing context: {out}");
    assert!(body.contains(&" delta"), "missing context: {out}");
    assert!(body.len() <= 8, "diff not bounded to the region: {out}");
}

#[tokio::test]
async fn core_tools_carry_a_typed_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    // bash: the exit code is the verdict.
    let ok = bash(cwd, &json!({"command": "true"})).await;
    assert_eq!(ok.status(), "ok");
    let err = bash(cwd, &json!({"command": "false"})).await;
    assert_eq!(err.status(), "error");
    // read: a missing file is an error; a present one is ok.
    let err = read(cwd, &json!({"path": "missing.txt"}), None).await;
    assert_eq!(err.status(), "error");
    std::fs::write(cwd.join("a.txt"), "hi\n").unwrap();
    let ok = read(cwd, &json!({"path": "a.txt"}), None).await;
    assert_eq!(ok.status(), "ok");
    // write: a clean write is ok.
    let ok = write(cwd, &json!({"path": "b.txt", "content": "x"})).await;
    assert_eq!(ok.status(), "ok");
}
