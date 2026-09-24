use super::*;
use serde_json::json;

#[test]
fn recall_browses_group_ranges_and_reports_missing_things() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 5, 10);
    // A long-text entry: the preview is capped at 200 chars.
    store
        .append(
            "user",
            json!({ "text": "l".repeat(300), "lane": "follow-up" }),
            Some("00000004"),
        )
        .unwrap();
    let g1 = om::wrap_in_observation_group("first obs", "00000000:00000002", "a", None);
    let g2 = om::wrap_in_observation_group(
        "merged obs",
        "00000003:00000004,00000005:00000006",
        "b",
        None,
    );
    let record = OmRecord {
        active_observations: format!("{g1}\n\n{g2}"),
        ..Default::default()
    };
    // Simple range: exactly its two entries.
    let out = recall(&mut store, &record, &json!({ "group": "a" }));
    assert!(out.contains("00000000"), "{out}");
    assert!(out.contains("00000002"), "{out}");
    assert!(!out.contains("00000003"), "{out}");
    // Merged span: first segment's start to the last segment's end.
    let out = recall(&mut store, &record, &json!({ "group": "b" }));
    for id in ["00000003", "00000004", "00000005", "00000006"] {
        assert!(out.contains(id), "{out}");
    }
    assert!(!out.contains("00000002"), "{out}");
    // The 200-char preview.
    assert!(out.contains(&"l".repeat(200)), "{out}");
    assert!(!out.contains(&"l".repeat(201)), "{out}");
    // Not-found paths: no arg, unknown group, rangeless group, and a
    // range pointing outside this session (a parent-scoped group).
    assert!(recall(&mut store, &record, &json!({})).contains("missing"));
    assert!(
        recall(&mut store, &record, &json!({ "group": "nope" })).contains("no observation group")
    );
    let no_range = OmRecord {
        active_observations: "<observation-group id=\"c\" range=\",\">orphan</observation-group>"
            .into(),
        ..Default::default()
    };
    assert!(recall(&mut store, &no_range, &json!({ "group": "c" })).contains("no range"));
    let foreign = OmRecord {
        active_observations: om::wrap_in_observation_group(
            "foreign",
            "99999999:99999999",
            "d",
            None,
        ),
        ..Default::default()
    };
    assert!(recall(&mut store, &foreign, &json!({ "group": "d" })).contains("not in this session"));
}

#[test]
fn raw_window_prunes_to_the_floor_and_keeps_tool_results_attached() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    // user (225 tokens), tool call, tool result, user (225): 470 total
    // over the 230-token floor, with the prune boundary landing on the
    // tool run.
    let entries = vec![
        ("user", "a".repeat(900)),
        ("tool", "call".to_owned()),
        ("tool", "result".to_owned()),
        ("user", "c".repeat(900)),
    ];
    let mut parent: Option<String> = None;
    for (kind, text) in entries {
        let e = store
            .append(
                kind,
                json!({ "text": text, "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let all = store.entries_range(0, usize::MAX).unwrap();
    let leaf = store.leaf().unwrap().map(|e| e.id);
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230, // = the retention floor at this threshold
    };
    let state = OmState::from_config(&cfg, OmRecord::default());
    let window = state.raw_window_from(&all, leaf.as_deref());
    // The window is bounded at the floor and, after the prune left a
    // tool entry at its head, the leading tool run is cut so no result
    // is orphaned from its call.
    assert_eq!(window.len(), 1, "{:?}", window);
    assert_eq!(window[0].kind, "user");
    assert_eq!(window[0].payload["text"], "c".repeat(900));
    assert!(state.pending_tokens(&window) <= 230);
    // Below the floor: nothing is pruned.
    let small = store.entries_range(0, usize::MAX).unwrap();
    let state2 = OmState::from_config(
        &crate::config::Om {
            om_model: String::new(),
            observe_threshold: 30_000,
            reflect_threshold: 40_000,
            buffer_increment: 6_000,
        },
        OmRecord::default(),
    );
    assert_eq!(
        state2.raw_window_from(&small, leaf.as_deref()).len(),
        small.len()
    );
}

#[test]
fn the_assembly_is_bounded_and_the_continuation_hint_is_one_shot() {
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
    };
    let mut state = OmState::from_config(
        &cfg,
        OmRecord {
            active_observations: "the log".into(),
            ..Default::default()
        },
    );
    state.changed = true;
    let first = state.assemble_context("base prompt", None);
    assert!(first.starts_with("base prompt"));
    assert!(first.contains("the log"));
    assert!(first.contains(om::OBSERVATION_CONTINUATION_HINT));
    // The one-shot flip: a second assembly carries no hint.
    let second = state.assemble_context("base prompt", None);
    assert!(!second.contains(om::OBSERVATION_CONTINUATION_HINT));
    // The task-resume-contract slot (ticket #24 fills it).
    let with_task = state.assemble_context("base prompt", Some("do the thing"));
    assert!(with_task.contains("# Task (resume contract)"));
    assert!(with_task.contains("do the thing"));
    // A demoted prefix drops out of the live context.
    let mut state3 = OmState::from_config(
        &cfg,
        OmRecord {
            frozen_prefix: "demoted prefix".into(),
            active_observations: "the log".into(),
            prefix_demoted: true,
            ..Default::default()
        },
    );
    assert!(
        !state3
            .assemble_context("base", None)
            .contains("demoted prefix")
    );
}

#[test]
fn the_idle_gap_measures_the_pause_before_the_current_turn() {
    let mk = |id: &str, parent: Option<&str>, ts: u64, kind: &str| Entry {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        timestamp: ts,
        kind: kind.to_owned(),
        payload: json!({ "text": "x" }),
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    };
    // Turn 1 (user + assistant), a 90-second pause, then the current
    // turn (user + assistant).
    let entries = vec![
        mk("00000001", None, 1_000, "user"),
        mk("00000002", Some("00000001"), 2_000, "assistant"),
        mk("00000003", Some("00000002"), 92_000, "user"),
        mk("00000004", Some("00000003"), 93_000, "assistant"),
    ];
    assert_eq!(idle_gap_secs(&entries, Some("00000004")), 90);
    // The gap uses the turn's FIRST user entry: a steering sent inside
    // the turn doesn't shrink the measured pause.
    let entries = vec![
        mk("00000001", None, 1_000, "user"),
        mk("00000002", Some("00000001"), 2_000, "assistant"),
        mk("00000003", Some("00000002"), 92_000, "user"),
        mk("00000004", Some("00000003"), 92_500, "user"),
        mk("00000005", Some("00000004"), 93_000, "assistant"),
    ];
    assert_eq!(idle_gap_secs(&entries, Some("00000005")), 90);
    // A fresh session (branch starts with the turn's user entry): zero.
    let fresh = vec![
        mk("00000001", None, 1_000, "user"),
        mk("00000002", Some("00000001"), 2_000, "assistant"),
    ];
    assert_eq!(idle_gap_secs(&fresh, Some("00000002")), 0);
}

#[test]
fn seed_compacted_child_stores_the_snapshot_and_returns_the_frozen_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut parent_store = SessionStore::for_workspace(dir.path(), "parent");
    parent_store.create().unwrap();
    let parent_record = OmRecord {
        frozen_prefix: "grandparent log".into(),
        active_observations: om::wrap_in_observation_group(
            "parent log",
            "00000000:00000001",
            "p",
            None,
        ),
        ..Default::default()
    };
    let mut child_store = SessionStore::for_workspace(dir.path(), "child");
    child_store.create().unwrap();
    let child = seed_compacted_child(&parent_record, "parent", &mut child_store).unwrap();
    // The child's record carries the parent's full log verbatim.
    assert_eq!(
        child.frozen_prefix,
        format!("grandparent log{}", parent_record.active_observations)
    );
    assert!(child.active_observations.is_empty());
    // The child's session file carries the spawn-snapshot entry.
    let entries = child_store.entries_range(0, usize::MAX).unwrap();
    let snap = entries
        .iter()
        .find(|e| e.kind == KIND_SPAWN_SNAPSHOT)
        .expect("spawn-snapshot entry");
    assert_eq!(snap.payload["parentSession"], "parent");
    assert_eq!(snap.payload["range"], "00000000:00000001");
    assert_eq!(snap.payload["log"], child.frozen_prefix);
    // An empty parent log: no snapshot entry, empty prefix.
    let mut child2 = SessionStore::for_workspace(dir.path(), "child2");
    child2.create().unwrap();
    let rec2 = seed_compacted_child(&OmRecord::default(), "parent", &mut child2).unwrap();
    assert!(rec2.frozen_prefix.is_empty());
    assert!(child2.entries_range(0, usize::MAX).unwrap().is_empty());
}
