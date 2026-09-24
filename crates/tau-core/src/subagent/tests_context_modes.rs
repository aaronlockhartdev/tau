use super::*;

use crate::session::SessionStore;
use crate::subagent::testkit::*;

/// A compacted spawn seeds the parent's observation log verbatim as
/// the child's frozen prefix (a `spawn-snapshot` entry with the
/// parent pointer).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compacted_spawn_seeds_the_frozen_prefix() {
    let dir = tempfile::tempdir().unwrap();
    // The parent carries an OM record with observations.
    let (sup, _) = harness(
        dir.path(),
        vec![sse("obs turn", &[])],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    let parent = sup.parent.lock().unwrap().clone().unwrap();
    parent.set_om(Some(OmState::from_config(
        &Om::default(),
        OmRecord {
            frozen_prefix: String::new(),
            active_observations: "<observations>the parent's log</observations>".into(),
            ..Default::default()
        },
    )));
    let s = sup
        .spawn(
            "general",
            "compact me",
            Some(ContextMode::Compacted),
            None,
            "c0",
        )
        .unwrap();
    // Quiesce the child before reading its file from a second store:
    // its drive ends when the nudge exhausts, after which the file is
    // stable (review B1: a reader racing the drive saw a torn branch).
    s.drive.await.unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
    store.open().unwrap();
    // The spawn-snapshot entry (the record in the child's file).
    let snapshot = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == om_integration::KIND_SPAWN_SNAPSHOT)
        .expect("a spawn-snapshot entry links the frozen prefix to the parent");
    assert_eq!(snapshot.payload["parentSession"], "parent");
    assert!(
        snapshot.payload["log"]
            .as_str()
            .unwrap()
            .contains("the parent's log")
    );
    // The child's own record: the prefix verbatim, empty suffix.
    let record = OmState::load_record(&mut store).unwrap();
    assert_eq!(
        record.frozen_prefix,
        "<observations>the parent's log</observations>"
    );
    assert!(record.active_observations.is_empty());
}

/// Parent-scoped recall (review N4): a compacted child's
/// `scope: "parent"` browses the parent session's raw history — the
/// target of its frozen prefix; a session without a parent link gets a
/// refusal, not a guess.
#[tokio::test]
async fn a_compacted_child_recalls_the_parent_session() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _) = harness(
        dir.path(),
        vec![sse("obs turn", &[])],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    let parent = sup.parent.lock().unwrap().clone().unwrap();
    // The parent's raw history, plus an observation group covering it.
    parent
        .append_entry(
            crate::agent::KIND_USER,
            json!({ "text": "parent raw one", "lane": "follow-up" }),
        )
        .unwrap();
    parent
        .append_entry(
            crate::agent::KIND_USER,
            json!({ "text": "parent raw two", "lane": "follow-up" }),
        )
        .unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.open().unwrap();
    let entries = store.entries_range(0, usize::MAX).unwrap();
    let first = entries[0].id.as_str();
    let last = entries.last().unwrap().id.as_str();
    let record = OmRecord {
        frozen_prefix: String::new(),
        active_observations: crate::om::wrap_in_observation_group(
            "parent obs",
            &format!("{first}:{last}"),
            "pg",
            None,
        ),
        ..Default::default()
    };
    // Saved to the file: the parent-scoped recall reads the record the
    // way it always does (a file read, not the parent's memory).
    OmState::from_config(&Om::default(), record.clone())
        .save(&mut store)
        .unwrap();
    parent.set_om(Some(OmState::from_config(&Om::default(), record)));
    let s = sup
        .spawn(
            "general",
            "compact me",
            Some(ContextMode::Compacted),
            None,
            "c0",
        )
        .unwrap();
    s.drive.await.unwrap();
    let child = sup.child_agent(&s.handle).unwrap();
    // Parent scope: the parent's raw entries come back.
    let out = child.recall_scoped(&json!({ "group": "pg", "scope": "parent" }));
    assert!(out.contains("parent raw one"), "{out}");
    assert!(out.contains("parent raw two"), "{out}");
    // The parent itself has no parent link.
    let out = parent.recall_scoped(&json!({ "group": "pg", "scope": "parent" }));
    assert!(out.contains("needs a parent link"), "{out}");
}

/// A fork copies the parent's entry tree (stable ids) and inherits the
/// parent's record wholesale.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_copies_the_tree_and_inherits_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _) = harness(
        dir.path(),
        vec![sse("working", &[])],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    let parent = sup.parent.lock().unwrap().clone().unwrap();
    // A user entry on the parent (the fork must copy it).
    parent
        .append_entry(
            crate::agent::KIND_USER,
            json!({ "text": "work 1", "lane": "follow-up" }),
        )
        .unwrap();
    // A record on the parent.
    parent.set_om(Some(OmState::from_config(
        &Om::default(),
        OmRecord {
            frozen_prefix: "FROZEN".into(),
            active_observations: "SUFFIX".into(),
            ..Default::default()
        },
    )));
    let s = sup
        .spawn("general", "fork me", Some(ContextMode::Fork), None, "c0")
        .unwrap();
    // Quiesce the child before reading its file from a second store
    // (review B1, same as the compacted test).
    s.drive.await.unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
    store.open().unwrap();
    let child_entries = store.entries_range(0, usize::MAX).unwrap();
    // The parent's user entry is present with its stable id.
    let mut pstore = SessionStore::for_workspace(dir.path(), "parent");
    pstore.open().unwrap();
    let parent_entries = pstore.entries_range(0, usize::MAX).unwrap();
    let parent_user = parent_entries
        .iter()
        .find(|e| e.kind == "user")
        .expect("the parent has a user entry");
    assert!(
        child_entries.iter().any(|e| e.id == parent_user.id),
        "the fork carries the parent's entries with stable ids"
    );
    // The record: the fork owns the whole log (prefix undemoted into
    // the suffix), ADR-0004.
    let record = OmState::load_record(&mut store).unwrap();
    assert_eq!(record.frozen_prefix, "");
    assert_eq!(record.active_observations, "FROZENSUFFIX");
}
