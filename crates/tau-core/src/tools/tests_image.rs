use super::impls::read;
use super::tests::text;
use super::*;
use base64::Engine;

/// A solid `w`×`h` test image saved as `name` in `dir` — no binary fixtures.
fn write_test_image_of_size(
    dir: &std::path::Path,
    name: &str,
    w: u32,
    h: u32,
) -> image::DynamicImage {
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        w,
        h,
        image::Rgba([120, 200, 80, 255]),
    ));
    img.save(dir.join(name)).expect("test image encode");
    img
}

/// A real 64×64 test image saved as `name` in `dir`, for the read
/// conversion tests — no binary fixtures.
fn write_test_image(dir: &std::path::Path, name: &str) -> image::DynamicImage {
    write_test_image_of_size(dir, name, 64, 64)
}

/// A minimal 64×64 lossless WebP fixture (base64). `image` can decode
/// WebP but not encode it, so the fixture is a constant — 44 bytes, a
/// single VP8L frame of solid color.
const WEBP_B64: &str = "UklGRiQAAABXRUJQVlA4TBcAAAAvP8APAAdQ5OIVqv8BICH8f69F9D/1AwA=";

/// Read `name` and assert the wire block is a canonical JPEG: `image/jpeg`
/// media type, and base64 that decodes to a decodable JPEG of the source
/// dimensions.
fn assert_canonical_jpeg(dir: &std::path::Path, name: &str) {
    let result = read(dir, &json!({"path": name}), None, None, None);
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
    // #257: an under-budget read is not downscaled and carries no note.
    assert!(img.note.is_none(), "{:#?}", img.note);
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
    let out = text(read(
        dir.path(),
        &json!({"path": "note.txt"}),
        None,
        None,
        None,
    ));
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
    let result = read(dir.path(), &json!({"path": "broken.png"}), None, None, None);
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
        None,
    ));
    assert!(out.contains("over the 100-byte cap"), "{out}");
    // At the cap: the image block comes through, canonical JPEG.
    assert_canonical_jpeg(dir.path(), "big.png");
}

#[tokio::test]
async fn an_over_budget_image_is_downscaled_to_the_pixel_budget() {
    let dir = tempfile::tempdir().unwrap();
    // 2048×2048 = 4,194,304 px, double the 2,097,152 default budget.
    write_test_image_of_size(dir.path(), "large.png", 2048, 2048);
    let result = read(dir.path(), &json!({"path": "large.png"}), None, None, None);
    let ToolOutput::Image(img) = result.output else {
        panic!("expected an image block, got {result:?}");
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&img.data_base64)
        .expect("wire block is base64");
    let back = image::load_from_memory(&decoded).expect("wire bytes decode");
    // floor(sqrt(2097152 / 4194304)) × 2048 = 1448 on both axes.
    assert_eq!((back.width(), back.height()), (1448, 1448));
    // The note maps wire coordinates back to the original (2048/1448 = 1.41).
    assert_eq!(
        img.note.as_deref(),
        Some(
            "[Image: original 2048x2048, displayed at 1448x1448. Multiply coordinates by 1.41 to map to original image.]"
        ),
    );
}

#[tokio::test]
async fn a_tight_pixel_budget_downscales_an_oversized_image() {
    let dir = tempfile::tempdir().unwrap();
    // 512×512 = 262,144 px; a quarter-area budget scales by ½ to 256×256.
    write_test_image_of_size(dir.path(), "mid.png", 512, 512);
    let result = read(
        dir.path(),
        &json!({"path": "mid.png"}),
        None,
        Some(65_536),
        None,
    );
    let ToolOutput::Image(img) = result.output else {
        panic!("expected an image block, got {result:?}");
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&img.data_base64)
        .expect("wire block is base64");
    let back = image::load_from_memory(&decoded).expect("wire bytes decode");
    assert_eq!((back.width(), back.height()), (256, 256));
    assert_eq!(
        img.note.as_deref(),
        Some(
            "[Image: original 512x512, displayed at 256x256. Multiply coordinates by 2.00 to map to original image.]"
        ),
    );
}
