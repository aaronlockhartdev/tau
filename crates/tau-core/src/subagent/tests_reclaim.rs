use super::*;

use crate::subagent::testkit::*;

use serde_json::json;
fn state_entry(state: &str, extra: &serde_json::Value) -> crate::session::Entry {
    let mut payload = serde_json::json!({ "event": "state", "state": state });
    if let (Some(target), Some(obj)) = (extra.as_object(), payload.as_object_mut()) {
        for (k, v) in target {
            obj.insert(k.clone(), v.clone());
        }
    }
    crate::session::Entry {
        id: "e1".into(),
        parent: None,
        timestamp: 0,
        kind: EntryKind::Subagent,
        payload,
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    }
}

fn state_only_child(
    dir: &std::path::Path,
    id: &str,
    parent: &str,
    state: &str,
    extra: &serde_json::Value,
) {
    let mut store = SessionStore::for_workspace(dir, id);
    store.create().unwrap();
    store.set_parent(parent).unwrap();
    store
        .append(EntryKind::Subagent, state_entry(state, extra).payload, None)
        .unwrap();
}

/// A done child on disk: the re-adopt's positive input.
fn done_child(dir: &std::path::Path, id: &str, parent: &str) {
    state_only_child(dir, id, parent, "done", &serde_json::json!({}));
}

/// A parked child on disk: re-adopt must refuse it (terminal-only).
fn idle_child(dir: &std::path::Path, id: &str, parent: &str) {
    state_only_child(
        dir,
        id,
        parent,
        "idle",
        &serde_json::json!({ "waiting_on": "parent" }),
    );
}

#[test]
fn disk_children_lists_only_terminal_children() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    // The parent's file (so the workspace is non-empty) + three children.
    let mut p = SessionStore::for_workspace(dir, "p");
    p.create().unwrap();
    done_child(dir, "c1", "p");
    idle_child(dir, "c2", "p");
    state_only_child(
        dir,
        "c3",
        "p",
        "stopped",
        &serde_json::json!({ "by": "user" }),
    );
    // A sibling (not a child) is not listed.
    let mut sib = SessionStore::for_workspace(dir, "sib");
    sib.create().unwrap();

    let children = restore::disk_children(dir, "p", None);
    let mut ids: Vec<String> = children.iter().map(|c| c.child.clone()).collect();
    ids.sort();
    // Only the terminal children (done + stopped): the parked one is a
    // re-adopt no-op, not a candidate.
    assert_eq!(
        ids,
        vec!["c1".to_owned(), "c3".to_owned()],
        "terminal-only, no sibling"
    );
    let c1 = children.iter().find(|c| c.child == "c1").unwrap();
    assert!(matches!(c1.state, ChildState::Done { .. }));
    // No spawn record in the parent's file → the handle is the session id.
    assert_eq!(c1.handle, "c1");
    // The name is the header title (absent here → the session id).
    assert_eq!(c1.name, "c1");
}

#[test]
fn disk_children_reads_the_minted_handle_from_spawn_records() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut p = SessionStore::for_workspace(dir, "p");
    p.create().unwrap();
    // The parent's spawn record names the child's minted handle.
    p.append(
        EntryKind::Subagent,
        serde_json::json!({
            "event": "spawn",
            "type": "general",
            "brief": "b",
            "context_mode": "fresh",
            "parent": "p",
            "call": "c1",
        }),
        None,
    )
    .unwrap();
    done_child(dir, "child-session", "p");

    let children = restore::disk_children(dir, "p", None);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].child, "child-session");
    // Without a spawn record carrying the child's id, the handle falls back
    // to the session id (the positive case above is the negative here: the
    // minted handle is only recoverable from a spawn record that names the
    // child — absent, so the session id stands in).
    assert_eq!(children[0].handle, "child-session");
}

/// A fork child's spawn record is appended at the copied leaf — the parent's
/// whole tree is its log — so the fleet read finds it in its tail window (#302).
#[test]
fn disk_children_read_the_provenance_of_a_fork_child_from_its_tail() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut p = SessionStore::for_workspace(dir, "p");
    p.create().unwrap();
    for i in 0..12 {
        p.append(EntryKind::User, json!({ "text": format!("p{i}") }), None)
            .unwrap();
    }
    // fork = the parent's record copied wholesale (spec §5.1), so the child's
    // own spawn record lands at the end of the copied tree, not at the head.
    let mut c = SessionStore::fork_from(&p, "cf").unwrap();
    c.set_parent("p").unwrap();
    let leaf = c.leaf().unwrap().map(|e| e.id);
    c.append(
        EntryKind::Subagent,
        json!({
            "event": "spawn",
            "type": "auditor",
            "brief": "b",
            "context_mode": "fork",
            "parent": "p",
            "call": "t0",
        }),
        leaf.as_deref(),
    )
    .unwrap();
    let leaf = c.leaf().unwrap().map(|e| e.id);
    c.append(
        EntryKind::Subagent,
        state_entry("done", &json!({})).payload,
        leaf.as_deref(),
    )
    .unwrap();

    let children = restore::disk_children(dir, "p", None);
    assert_eq!(children.len(), 1);
    assert_eq!(
        children[0].agent_type, "auditor",
        "the fork child's spawn record sits at its log's tail"
    );
    assert!(
        matches!(children[0].context_mode, ContextMode::Fork),
        "a head-only lookup would have read the general/fresh defaults"
    );
}

/// The fleet read's tail window is the bound its doc names: a state record
/// past it reads as no state record, and the child is a re-adopt no-op.
#[test]
fn a_state_record_past_the_tail_window_is_no_state_record() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut p = SessionStore::for_workspace(dir, "p");
    p.create().unwrap();
    state_only_child(dir, "c1", "p", "done", &json!({}));
    let mut c = SessionStore::for_workspace(dir, "c1");
    c.open().unwrap();
    let mut leaf = c.leaf().unwrap().map(|e| e.id);
    for i in 0..restore::STATE_TAIL - 1 {
        leaf = Some(
            c.append(
                EntryKind::User,
                json!({ "text": format!("after {i}") }),
                leaf.as_deref(),
            )
            .unwrap()
            .id,
        );
    }
    assert_eq!(
        restore::disk_children(dir, "p", None).len(),
        1,
        "a full window of records after the state record still reads it"
    );
    // One entry past the window: the record is out of reach, so the child
    // reads as parked and re-adopt drops it.
    c.append(
        EntryKind::User,
        json!({ "text": "one past the window" }),
        leaf.as_deref(),
    )
    .unwrap();
    assert!(
        restore::disk_children(dir, "p", None).is_empty(),
        "the window is the bound the doc names"
    );
}

#[test]
fn no_state_entry_is_none_not_a_guess() {
    assert!(restore::last_state_from(&[]).is_none());
}

#[test]
fn a_running_record_is_not_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut p = SessionStore::for_workspace(dir, "p");
    p.create().unwrap();
    state_only_child(dir, "c1", "p", "running", &serde_json::json!({}));
    // A `running` record (a restart artifact) is not terminal: re-adopt
    // refuses it, and `disk_children` drops it.
    assert!(restore::disk_children(dir, "p", None).is_empty());
}

/// The re-adopt positive path: a reclaimed (terminal) child on disk is
/// rebuilt into a live `Child` when addressed, with its original handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resolve_miss_re_adopts_a_terminal_child() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![vec![sse("", &[])]],
        SubAgents {
            fleet_soft_bound: Some(4),
            fleet_hard_cap: Some(8),
            ..SubAgents::default()
        },
    );
    done_child(dir.path(), "reclaimed-child", "parent");

    // A resolve for the (unknown) name is a miss → a re-adopt. The child's
    // name is its session id (no title), so address by that.
    let info = sup
        .state_info("reclaimed-child")
        .expect("a terminal child on disk re-adopts on a resolve miss");
    assert_eq!(info.handle, "reclaimed-child");
    assert!(matches!(info.state, ChildState::Done { .. }));
    // The re-adopted child is in the registry again.
    assert!(sup.handles().contains(&"reclaimed-child".to_owned()));
}

/// The re-adopt follows the inherit rule (#336): the pair the parent moved to
/// is the pair the rebuilt child is addressed at — the provider its model id
/// names, not the model the supervisor launched with.
#[test]
fn a_re_adopted_child_is_addressed_at_the_parents_current_model() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, factory, _parent) = harness_full(
        dir.path(),
        vec![sse("x", &[])],
        vec![vec![sse("", &[])]],
        vec![],
        SubAgents::default(),
    );
    done_child(dir.path(), "reclaimed-child", "parent");
    sup.set_inherited(
        "zzz/gpt".into(),
        TurnConfig {
            max_output_tokens: Some(4096),
            ..TurnConfig::default()
        },
    );

    sup.state_info("reclaimed-child")
        .expect("a terminal child on disk re-adopts on a resolve miss");

    assert_eq!(
        factory.routed.lock().as_slice(),
        ["zzz/gpt".to_string()],
        "the re-adopt routes the parent's current model"
    );
    assert_eq!(
        sup.child_agent("reclaimed-child").map(|a| a.model()),
        Some("zzz/gpt".into()),
        "the rebuilt child carries the parent's current model"
    );
    assert_eq!(
        sup.child_agent("reclaimed-child")
            .map(|a| a.current_turn().max_output_tokens),
        Some(Some(4096)),
        "the rebuilt child is gated with that model's facts, not the launch model's"
    );
}

/// The re-adopt negative path: a parked (idle) child on disk is NOT
/// re-adopted on a resolve miss — it is a genuine unknown, not a zombie.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resolve_miss_does_not_re_adopt_a_parked_child() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![vec![sse("", &[])]],
        SubAgents {
            fleet_soft_bound: Some(4),
            fleet_hard_cap: Some(8),
            ..SubAgents::default()
        },
    );
    idle_child(dir.path(), "parked-child", "parent");

    // A parked child is not a re-adopt candidate: the resolve stays a miss.
    assert!(sup.state_info("parked-child").is_none());
    assert!(sup.handles().is_empty(), "no child was fabricated");
}

/// The re-adopt negative path: an unknown name with no disk record is a
/// miss, not a fabricated child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resolve_miss_with_no_disk_record_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![vec![sse("", &[])]],
        SubAgents::default(),
    );
    assert!(sup.state_info("never-existed").is_none());
    assert!(sup.handles().is_empty());
}

/// The fleet's hard cap is a spawn-time backstop: a spawn that would push
/// the total handle count past the cap is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hard_cap_refuses_a_spawn_over_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![
            vec![sse("", &[])],
            vec![sse("", &[])],
            vec![sse("", &[])],
            vec![sse("", &[])],
        ],
        SubAgents {
            fleet_soft_bound: None, // no reclamation: the cap is the limit
            fleet_hard_cap: Some(2),
            ..SubAgents::default()
        },
    );
    // Two spawns fit (total 2); the third is refused (it would make 3 > 2).
    sup.spawn("general", "a", None, "c1").expect("first fits");
    sup.spawn("general", "b", None, "c2").expect("second fits");
    let err = sup
        .spawn("general", "c", None, "c3")
        .expect_err("the third is over the hard cap");
    assert!(
        err.contains("fleet cap"),
        "the refusal names the fleet cap, got: {err}"
    );
}

/// The soft bound evicts the oldest terminal child on a spawn that pushes
/// the fleet over it; a running child is never evicted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_over_the_soft_bound_evicts_the_oldest_terminal() {
    let dir = tempfile::tempdir().unwrap();
    // Soft bound 2: the third spawn evicts one terminal child.
    let (sup, bridge, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![
            // child 0: finishes (done) → terminal.
            vec![sse(
                "",
                &[crate::subagent::testkit::notify_call(
                    "n1",
                    "a",
                    true,
                    Some(json!({})),
                    None,
                )],
            )],
            // child 1: finishes (done) → terminal.
            vec![sse(
                "",
                &[crate::subagent::testkit::notify_call(
                    "n2",
                    "b",
                    true,
                    Some(json!({})),
                    None,
                )],
            )],
            // child 2: keeps running.
            vec![sse("", &[])],
        ],
        SubAgents {
            fleet_soft_bound: Some(2),
            fleet_hard_cap: Some(64),
            ..SubAgents::default()
        },
    );
    // Spawn three children; the first two finish, the third runs.
    let h0 = sup
        .spawn("general", "a", None, "c1")
        .expect("spawn 0")
        .handle;
    let h1 = sup
        .spawn("general", "b", None, "c2")
        .expect("spawn 1")
        .handle;
    // Wait for both to be done (terminal) before the third spawn.
    wait_for(|| {
        matches!(sup.state_info(&h0).unwrap().state, ChildState::Done { .. })
            && matches!(sup.state_info(&h1).unwrap().state, ChildState::Done { .. })
    });
    assert_eq!(
        sup.handles().len(),
        2,
        "two terminal children before the third spawn"
    );
    // The third spawn pushes the fleet to 3 > soft bound 2 → it evicts the
    // oldest terminal child (h0, the lower mint counter).
    let h2 = sup
        .spawn("general", "c", None, "c3")
        .expect("spawn 2")
        .handle;
    // The reclamation notice names the evicted (oldest terminal) child.
    let reclaims = bridge.reclaims.lock().clone();
    assert_eq!(reclaims.len(), 1, "one child was reclaimed");
    assert_eq!(
        reclaims[0].handle, h0,
        "the oldest terminal child is evicted"
    );
    // The registry now holds the two youngest: h1 (terminal) + h2 (running).
    let binding = sup.handles();
    let handles: Vec<&str> = binding.iter().map(String::as_str).collect();
    assert!(
        handles.contains(&h1.as_str()),
        "h1 (younger terminal) stays"
    );
    assert!(handles.contains(&h2.as_str()), "h2 (running) stays");
    assert!(
        !handles.contains(&h0.as_str()),
        "h0 (oldest terminal) is gone"
    );
    assert_eq!(sup.handles().len(), 2, "back under the soft bound");
}

/// A running / parked child is never evicted, even when the fleet is over
/// the soft bound and only non-terminal children are present.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_child_is_never_evicted() {
    let dir = tempfile::tempdir().unwrap();
    // Soft bound 1: with two children over the bound, only a terminal one
    // can be evicted. Both here are parked (idle) → nothing is evicted.
    let (sup, bridge, _parent) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![
            // child 0: rests idle (parked, not terminal).
            vec![sse("", &[notify_call("n1", "resting", false, None, None)])],
            vec![sse("", &[])],
            // child 1: rests idle too.
            vec![sse("", &[notify_call("n2", "resting", false, None, None)])],
            vec![sse("", &[])],
        ],
        SubAgents {
            fleet_soft_bound: Some(1),
            fleet_hard_cap: Some(64),
            ..SubAgents::default()
        },
    );
    let h0 = sup
        .spawn("general", "a", None, "c1")
        .expect("spawn 0")
        .handle;
    wait_for(|| matches!(sup.state_info(&h0).unwrap().state, ChildState::Idle { .. }));
    let h1 = sup
        .spawn("general", "b", None, "c2")
        .expect("spawn 1")
        .handle;
    wait_for(|| matches!(sup.state_info(&h1).unwrap().state, ChildState::Idle { .. }));
    // Two parked children, soft bound 1: over the bound, but neither is
    // terminal, so the reclamation evicts nothing.
    assert!(
        bridge.reclaims.lock().is_empty(),
        "no parked child is ever reclaimed"
    );
    assert_eq!(sup.handles().len(), 2, "both parked children stay");
}
