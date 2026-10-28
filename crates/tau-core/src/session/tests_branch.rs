use super::{Entry, branch};

#[test]
fn the_branch_walks_the_leaf_chain_root_to_leaf() {
    let root: Entry = serde_json::from_str(
        r#"{"id":"00000001","timestamp":1,"type":"user","payload":{"text":"a"}}"#,
    )
    .unwrap();
    let side: Entry = serde_json::from_str(
        r#"{"id":"00000002","parentId":"00000001","timestamp":2,"type":"user","payload":{"text":"off-branch"}}"#,
    )
    .unwrap();
    let leaf: Entry = serde_json::from_str(
        r#"{"id":"00000003","parentId":"00000001","timestamp":3,"type":"user","payload":{"text":"b"}}"#,
    )
    .unwrap();
    let branch = branch(&[root, side, leaf], Some("00000003"));
    let texts: Vec<&str> = branch
        .iter()
        .map(|e| e.payload["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(texts, ["a", "b"]);
}

#[test]
fn an_absent_leaf_walks_nothing() {
    let root: Entry = serde_json::from_str(
        r#"{"id":"00000001","timestamp":1,"type":"user","payload":{"text":"a"}}"#,
    )
    .unwrap();
    assert!(branch(&[root], Some("00000009")).is_empty());
}

#[test]
fn a_corrupt_duplicate_id_cycle_terminates() {
    // A duplicate id makes the parent chain cyclic; the branch walk must
    // terminate, not clone entries forever (the 80GB runaway's amplifier).
    let a: Entry = serde_json::from_str(
        r#"{"id":"00000001","parentId":"00000002","timestamp":1,"type":"user","payload":{"text":"a"}}"#,
    )
    .unwrap();
    let b: Entry = serde_json::from_str(
        r#"{"id":"00000002","parentId":"00000001","timestamp":2,"type":"user","payload":{"text":"b"}}"#,
    )
    .unwrap();
    let entries = vec![a, b];
    let branch = branch(&entries, Some("00000001"));
    assert!(
        branch.len() <= entries.len(),
        "the walk escaped the entry count: {} entries",
        branch.len()
    );
}
