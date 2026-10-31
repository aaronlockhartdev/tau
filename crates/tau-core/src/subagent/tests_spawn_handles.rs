use super::*;

use crate::agent::Lane;
use crate::session::SessionStore;
use crate::subagent::testkit::*;
use serde_json::json;
use std::time::Duration;

/// Drive the parent's loop: it runs its script, which calls the spawn
/// tools in-loop (the tool dispatch records the spawn entries in the
/// parent's session file).
async fn sup_spawn_from_parent(sup: &Arc<Supervisor>) {
    let parent = sup.parent.lock().as_ref().unwrap().upgrade().unwrap();
    parent.send("go", Lane::FollowUp);
    parent.process().await.unwrap();
}

/// A re-opened parent session must not re-mint a child handle an earlier
/// life already used (ticket #226): the GUI keys its roster rows by
/// handle, so a re-minted handle for a NEW child collides with a row the
/// frontend still carries for the old child, and the old child's card
/// ends up rendering the new sibling's state. The counter is seeded from
/// the `subagent_spawn` tool entries in the parent's own session file,
/// which is the durable spawn log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reopened_parent_does_not_remint_a_child_handle() {
    let dir = tempfile::tempdir().unwrap();
    // Life 1: the parent's loop spawns two children (one tool call each),
    // minting "parent-1" and "parent-2".
    let (sup, bridge, factory, parent) = harness_full(
        dir.path(),
        vec![
            sse(
                "",
                &[(
                    "subagent_spawn".into(),
                    "c1".into(),
                    spawn_args("first", "fresh").to_string(),
                )],
            ),
            sse(
                "",
                &[(
                    "subagent_spawn".into(),
                    "c2".into(),
                    spawn_args("second", "fresh").to_string(),
                )],
            ),
            sse("", &[]),
        ],
        vec![vec![sse("", &[])]],
        vec![Duration::ZERO],
        SubAgents::default(),
    );
    sup_spawn_from_parent(&sup).await;
    let mut handles = sup.handles();
    handles.sort();
    assert_eq!(handles, vec!["parent-1".to_owned(), "parent-2".to_owned()]);

    // Life 2: the session re-opens; a fresh supervisor adopts the same
    // parent session (the harness launch does this: the supervisor
    // adopts the parent's agent).
    let sup2 = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory.clone(),
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: bridge.clone(),
        driver: Arc::new(TestDriver),
        core: None,
        parent_entries: None,
    });
    sup2.attach_parent(&parent);
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

/// The seed scans the parent's session file for spawn records; a fresh
/// parent (one spawn) counts from one, and the spawn is recorded as a
/// `subagent_spawn` tool entry — the seed's source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parent_without_spawn_records_starts_counting_from_one() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _, _parent) = harness(
        dir.path(),
        vec![
            sse(
                "",
                &[(
                    "subagent_spawn".into(),
                    "c1".into(),
                    spawn_args("only", "fresh").to_string(),
                )],
            ),
            sse("", &[]),
        ],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    sup_spawn_from_parent(&sup).await;
    assert_eq!(sup.handles(), vec!["parent-1".to_owned()]);
    // The spawn is recorded as a `subagent_spawn` tool entry in the
    // parent's file (the durable spawn log the counter is seeded from).
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.open().unwrap();
    let spawns = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Tool && e.payload["name"] == json!("subagent_spawn"))
        .count();
    assert_eq!(spawns, 1, "the spawn record is the seed's source");
}
