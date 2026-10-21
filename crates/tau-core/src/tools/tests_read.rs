use super::impls::{image_media_type, read, write};
use super::tests::{detail, fresh_registry, text};
use super::*;
use base64::Engine;

#[tokio::test]
async fn read_pages_with_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "p.txt", "content": "l1\nl2\nl3\nl4\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(read(
        dir.path(),
        &json!({"path": "p.txt", "offset": "2", "limit": 2}),
        None,
        Some(&mut *reg),
    ));
    assert!(out.contains("lines 2–3 of 4"), "{out}");
    assert!(out.contains("l2") && out.contains("l3"));
    assert!(!out.contains("l1") && !out.contains("l4"));
}

#[tokio::test]
async fn read_offset_accepts_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "p.txt", "content": "l1\nl2\nl3\nl4\n"}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let first = text(read(
        dir.path(),
        &json!({"path": "p.txt", "limit": 2}),
        None,
        Some(&mut *reg),
    ));
    // The first shown line's anchor, used as the next page's offset.
    let anchor = first
        .lines()
        .next()
        .unwrap()
        .split('│')
        .next()
        .unwrap()
        .to_string();
    let out = text(read(
        dir.path(),
        &json!({"path": "p.txt", "offset": anchor, "limit": 2}),
        None,
        Some(&mut *reg),
    ));
    // Resuming from the anchor shows that line and the one after.
    assert!(out.contains("l1") && out.contains("l2"), "{out}");
    assert!(!out.contains("l4"), "{out}");
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
    match read(dir.path(), &json!({"path": "shot.png"}), None, None).output {
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
async fn read_ui_detail_is_the_anchor_stripped_body() {
    // #243: the card renders the stripped twin — no HASH\u{2502} prefixes, the
    // same content and pagination hints the model gets.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &json!({"path": "p.txt", "content": "l1\nl2\nl3\nl4\nl5\nl6\n"}),
        None,
    )
    .await;
    let res = read(
        dir.path(),
        &json!({"path": "p.txt", "limit": 4}),
        None,
        None,
    );
    let out = text(res.clone());
    let stripped = detail(res).expect("a read success carries its stripped body");
    assert!(
        !stripped.contains('\u{2502}'),
        "anchors must not reach the card: {stripped}"
    );
    assert!(
        stripped.contains("l1") && stripped.contains("l4"),
        "{stripped}"
    );
    // The pagination hint survives the strip.
    assert!(stripped.contains("lines 1\u{2013}4 of 6"), "{stripped}");
    // The model's page is unchanged: every data row carries its anchor
    // (the pagination hint line legitimately does not).
    assert!(
        out.lines()
            .filter(|l| !l.starts_with('\u{2026}'))
            .all(|l| l.contains('\u{2502}')),
        "{out}"
    );
}
