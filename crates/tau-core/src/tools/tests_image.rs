use super::impls::read;
use super::tests::text;
use super::*;
use base64::Engine;

/// A real 64×64 test image saved as `name` in `dir`, for the read
/// conversion tests — no binary fixtures.
fn write_test_image(dir: &std::path::Path, name: &str) -> image::DynamicImage {
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        64,
        image::Rgba([120, 200, 80, 255]),
    ));
    img.save(dir.join(name)).expect("test image encode");
    img
}

/// A minimal 64×64 lossless WebP fixture (base64). `image` can decode
/// WebP but not encode it, so the fixture is a constant — 44 bytes, a
/// single VP8L frame of solid color.
const WEBP_B64: &str = "UklGRiQAAABXRUJQVlA4TBcAAAAvP8APAAdQ5OIVqv8BICH8f69F9D/1AwA=";

/// Read `name` and assert the wire block is a canonical JPEG: `image/jpeg`
/// media type, and base64 that decodes to a decodable JPEG of the source
/// dimensions.
fn assert_canonical_jpeg(dir: &std::path::Path, name: &str) {
    let result = read(dir, &json!({"path": name}), None, None);
    let ToolOutput::Image(img) = result.output else {
        panic!("expected an image block, got {result:?}");
    };
    assert_eq!(img.kind, "image");
    assert_eq!(img.media_type, "image/jpeg");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&img.data_base64)
        .expect("wire block is base64");
    let back = image::load_from_memory(&decoded).expect("wire bytes decode");
    assert_eq!((back.width(), back.height()), (64, 64));
}

#[test]
fn read_converts_images_to_a_canonical_jpeg_wire_block() {
    let dir = tempfile::tempdir().unwrap();
    write_test_image(dir.path(), "shot.png");
    assert_canonical_jpeg(dir.path(), "shot.png");
    write_test_image(dir.path(), "anim.gif");
    assert_canonical_jpeg(dir.path(), "anim.gif");
    std::fs::write(
        dir.path().join("pic.webp"),
        base64::engine::general_purpose::STANDARD
            .decode(WEBP_B64)
            .expect("webp fixture is valid base64"),
    )
    .unwrap();
    assert_canonical_jpeg(dir.path(), "pic.webp");
    write_test_image(dir.path(), "scan.tiff");
    assert_canonical_jpeg(dir.path(), "scan.tiff");
    write_test_image(dir.path(), "old.bmp");
    assert_canonical_jpeg(dir.path(), "old.bmp");
    // A JPEG in is re-encoded; a decodable JPEG comes out.
    write_test_image(dir.path(), "shot.jpg");
    assert_canonical_jpeg(dir.path(), "shot.jpg");
}

#[tokio::test]
async fn read_returns_text_for_plain_utf8() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello\nworld\n").unwrap();
    let out = text(read(dir.path(), &json!({"path": "note.txt"}), None, None));
    assert!(out.contains("hello"), "{out}");
}

#[tokio::test]
async fn a_corrupt_image_is_refused_loudly() {
    let dir = tempfile::tempdir().unwrap();
    // PNG magic then garbage: an image we cannot ship, not text.
    std::fs::write(
        dir.path().join("broken.png"),
        b"\x89PNG\r\n\x1a\nnot-really",
    )
    .unwrap();
    let result = read(dir.path(), &json!({"path": "broken.png"}), None, None);
    assert!(!result.ok);
    let ToolOutput::Text(t) = result.output else {
        panic!("expected a text refusal, got {result:?}");
    };
    assert!(t.contains("could not be decoded"), "{t}");
}

#[tokio::test]
async fn an_oversized_image_is_refused_loudly() {
    let dir = tempfile::tempdir().unwrap();
    // A real 64×64 PNG is well over 100 bytes; the cap is on input bytes.
    write_test_image(dir.path(), "big.png");
    // Over the cap: a loud text refusal, not an image block.
    let out = text(read(
        dir.path(),
        &json!({"path": "big.png"}),
        Some(100),
        None,
    ));
    assert!(out.contains("over the 100-byte cap"), "{out}");
    // At the cap: the image block comes through, canonical JPEG.
    assert_canonical_jpeg(dir.path(), "big.png");
}
