use super::*;
use crate::subagent::testkit::*;
use crate::session::SessionStore;
use serde_json::json;
use std::time::Duration;

/// A re-opened parent session must not re-mint a child handle an earlier
/// life already used (ticket #226): the GUI keys its roster rows by
/// handle, so a re-minted handle for a NEW child collides with a row the
/// frontend still carries for the old child, and the old child's card ends
/// up rendering the new sibling's state. The counter is seeded from the
/// spawn entries in the parent's own session file, which is the durable
/// spawn record (ADR-0001).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reopened_parent_does_not_remint_a_child_handle() {
    let dir = tempfile::tempdir().unwrap();
    // Life 1: two children, minting "parent-1" and "parent-2".
    let (sup, _bridge, _factory, _parent) = harness_full(
        dir.path(),
        vec![sse("", &[])],
        vec![vec![sse("", &[])]],
        vec![Duration::ZERO],
        SubAgents::default(),
    );
    let s1 = sup
        .spawn("general", "first", Some(ContextMode::Fresh), "c1")
        .unwrap();
    let s2 = sup
        .spawn("general", "second", Some(ContextMode::Fresh), "c2")
        .unwrap();
    assert_eq!(s1.handle, "parent-1");
    assert_eq!(s2.handle, "parent-2");
    drop(s1);
    drop(s2);

    // Life 2: the session re-opens; a fresh supervisor adopts the same
    // parent session id.
    let sup2 = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: _factory.clone(),
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: _bridge.clone(),
        driver: Arc::new(TestDriver),
        core: None,
    });
    // The re-opened life re-attaches to the same parent session (the
    // harness launch does this: the supervisor adopts the parent's agent).
    sup2.attach_parent(&_parent);
    let s3 = sup2
        .spawn("general", "third", Some(ContextMode::Fresh), "c3")
        .unwrap();
    // The new child's handle must not re-mint a handle an earlier life
    // already handed out (it would collide with a stale roster row).
    assert_ne!(
        s3.handle, "parent-1",
        "re-opened parent re-minted a previous life's handle"
    );
    assert_ne!(s3.handle, "parent-2");
    // Monotonic: a re-opened counter continues, it does not restart.
    assert_eq!(s3.handle, "parent-3");
}

/// The seed scans the parent's session file for spawn records; a file
/// with no spawn entries seeds the counter at zero (a fresh parent).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parent_without_spawn_records_starts_counting_from_one() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = harness(
        dir.path(),
        vec![sse("", &[])],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    let s1 = sup
        .spawn("general", "only", Some(ContextMode::Fresh), "c1")
        .unwrap();
    assert_eq!(s1.handle, "parent-1");
    // The spawn record is in the parent's file (the durable spawn log).
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.open().unwrap();
    let spawns = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == json!("spawn"))
        .count();
    assert_eq!(spawns, 1, "the spawn record is the seed's source");
}
