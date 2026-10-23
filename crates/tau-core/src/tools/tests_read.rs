use super::impls::{edit, read, write};
use super::tests::{detail, fresh_registry, text};
use super::*;

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
        None,
        Some(&mut *reg),
    ));
    // Resuming from the anchor shows that line and the one after.
    assert!(out.contains("l1") && out.contains("l2"), "{out}");
    assert!(!out.contains("l4"), "{out}");
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

#[tokio::test]
async fn oversized_read_line_gets_an_inline_sed_hint() {
    // Parity with the reference's per-line placeholder: the line keeps its
    // anchor and gets an inspection command in place of the content, so a
    // targeted edit by that anchor still works.
    let dir = tempfile::tempdir().unwrap();
    let big: String = "x".repeat(110 * 1024);
    write(
        dir.path(),
        &json!({"path": "big.txt", "content": format!("head\n{big}\ntail\n")}),
        None,
    )
    .await;
    let mut reg = fresh_registry();
    let out = text(read(
        dir.path(),
        &json!({"path": "big.txt"}),
        None,
        None,
        Some(&mut *reg),
    ));
    // The placeholder names the exact line and the exact command; the summary
    // repeats it. The placeholder row keeps the anchor.
    assert!(
        out.contains("content not shown. Use bash: sed -n '2p'"),
        "{out}"
    );
    assert!(
        out.contains("[1 oversized line(s) over 100 KiB; inspect with bash: sed -n '2p'"),
        "{out}"
    );
    let anchor = out
        .lines()
        .nth(1)
        .and_then(|row| row.split('\u{2502}').next())
        .expect("row 2 keeps its anchor")
        .to_string();
    // The anchor stays served: a targeted edit by it applies.
    let res = text(edit(
        dir.path(),
        &json!({
            "path": "big.txt",
            "edits": [{"from": anchor, "to": anchor, "content": "SMALL"}]
        }),
        Some(&mut *reg),
        false,
    ));
    assert!(res.starts_with("Successfully edited"), "{res}");
}
