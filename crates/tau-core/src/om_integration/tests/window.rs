use super::*;
use serde_json::json;

/// The joined content of the OM-derived `role: user` input items — the seam
/// the continuation-hint assertions read (the hint rides as a synthetic user
/// message, #97; it is per-assembly, #334).
fn om_user_content(items: &[crate::provider::InputEntry]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            crate::provider::InputEntry::Message(m) if m.role == "user" => Some(m.content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The joined content of the OM-derived `role: system` input items — the
/// per-chunk observation messages (#96).
fn om_system_content(items: &[crate::provider::InputEntry]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            crate::provider::InputEntry::Message(m) if m.role == "system" => {
                Some(m.content.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn recall_browses_group_ranges_and_reports_missing_things() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 5, 10);
    // A long-text entry: the preview is capped at 200 chars.
    store
        .append(
            EntryKind::User,
            json!({ "text": (0..50).map(|i| format!("l: {i:04}")).collect::<Vec<_>>().join("\n"), "lane": "follow-up" }),
            Some("00000005"),
        )
        .unwrap();
    let g1 = om::wrap_in_observation_group("first obs", "00000001:00000002", "a", None);
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
    let out = recall(&mut store, &record, &json!({ "group": "a" })).unwrap();
    assert!(out.contains("00000001"), "{out}");
    assert!(out.contains("00000002"), "{out}");
    assert!(!out.contains("00000003"), "{out}");
    // Merged span: first segment's start to the last segment's end.
    let out = recall(&mut store, &record, &json!({ "group": "b" })).unwrap();
    for id in ["00000003", "00000004", "00000005", "00000006"] {
        assert!(out.contains(id), "{out}");
    }
    assert!(!out.contains("00000002"), "{out}");
    // The 200-char preview, cut on a line boundary: the partial last line is
    // dropped and an ellipsis marks the cut.
    assert!(out.contains("l: 0024"), "{out}");
    assert!(!out.contains("l: 0025"), "no partial last line: {out}");
    assert!(out.contains('…'), "marker: {out}");
    // range pointing outside this session (a parent-scoped group).
    assert!(
        recall(&mut store, &record, &json!({}))
            .unwrap_err()
            .contains("missing")
    );
    assert!(
        recall(&mut store, &record, &json!({ "group": "nope" }))
            .unwrap_err()
            .contains("no observation group")
    );
    let no_range = OmRecord {
        active_observations: "<observation-group id=\"c\" range=\",\">orphan</observation-group>"
            .into(),
        ..Default::default()
    };
    assert!(
        recall(&mut store, &no_range, &json!({ "group": "c" }))
            .unwrap_err()
            .contains("no range")
    );
    let foreign = OmRecord {
        active_observations: om::wrap_in_observation_group(
            "foreign",
            "99999999:99999999",
            "d",
            None,
        ),
        ..Default::default()
    };
    assert!(
        recall(&mut store, &foreign, &json!({ "group": "d" }))
            .unwrap_err()
            .contains("not in this session")
    );
    // range pointing outside this session (a parent-scoped group).
}

#[test]
fn recall_previews_stop_at_a_line_boundary() {
    // #219: a preview that must be cut stops at a line boundary, with an
    // ellipsis marker — never a partial line.
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s-recall");
    store.create().unwrap();
    let first = store
        .append(EntryKind::User, serde_json::json!({ "text": "one" }), None)
        .unwrap();
    let big: String = (0..100)
        .map(|i| format!("line {i:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    let long = store
        .append(
            EntryKind::User,
            serde_json::json!({ "text": big }),
            Some(&first.id),
        )
        .unwrap();
    let g = om::wrap_in_observation_group("obs", &format!("{}:{}", first.id, long.id), "e", None);
    let record = OmRecord {
        active_observations: g,
        ..Default::default()
    };
    let out = recall(&mut store, &record, &json!({ "group": "e" })).unwrap();
    let entry_line = out
        .lines()
        .find(|l| l.starts_with(&long.id))
        .expect("the long entry's line");
    assert!(entry_line.contains('…'), "marker missing: {entry_line}");
    // The preview is space-joined; the last `line NNN` in it is a complete
    // line, and the next one is absent (cut at the boundary).
    let body = entry_line.strip_suffix('…').expect("the ellipsis");
    // All-digit tokens in the entry line: the 10-digit id first, then the
    // `NNN` numbers of the `line NNN` preview tokens — so the last one is
    // the last complete line in the preview.
    let nums: Vec<String> = body
        .split(' ')
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned)
        .collect();
    let last = nums.last().expect("at least one line number");
    let n: usize = last.parse().expect("a numeric line token");
    assert!(
        body.contains(&format!("line {n:03}")),
        "last complete line missing: {body}"
    );
    assert!(
        !nums.contains(&(n + 1).to_string()),
        "no partial next line: {body}"
    );
}
#[test]
fn raw_window_is_the_full_unobserved_tail() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    // user (225 tokens), tool call, tool result, user (225): 452 total,
    // over the 230-token floor.
    let entries = vec![
        (EntryKind::User, "a".repeat(900)),
        (EntryKind::Tool, "call".to_owned()),
        (EntryKind::Tool, "result".to_owned()),
        (EntryKind::User, "c".repeat(900)),
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
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let state = OmState::from_config(&cfg, OmRecord::default());
    let window = state.raw_window_from(&all, leaf.as_deref());
    // No floor prune: the window is the full unobserved tail, the head
    // (the "task") included. The newest user entry is the tail's last item,
    // so no separate anchor is emitted (#334).
    assert!(window.anchor.is_none(), "{window:?}");
    assert_eq!(window.tail.len(), 4, "{window:?}");
    assert_eq!(window.tail[0].kind, EntryKind::User);
    assert_eq!(window.tail[0].payload["text"], "a".repeat(900));
    assert_eq!(window.tail[3].payload["text"], "c".repeat(900));
}
#[test]
fn raw_window_retains_unsummarized_head_past_the_floor() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    // Two 225-token user entries: 450 total, over the 230-token floor.
    let entries = vec![
        (EntryKind::User, "a".repeat(900)),
        (EntryKind::User, "b".repeat(900)),
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
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let state = OmState::from_config(&cfg, OmRecord::default());
    let window = state.raw_window_from(&all, leaf.as_deref());
    // No floor prune: the unsummarized head (the "task") survives past
    // the retention floor, and it is in the tail, so no anchor.
    assert!(window.anchor.is_none(), "{window:?}");
    assert_eq!(window.tail.len(), 2, "{window:?}");
    assert_eq!(window.tail[0].payload["text"], "a".repeat(900));
}

#[test]
fn raw_window_cuts_a_leading_orphaned_tool_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    // user, assistant, tool call, tool result, user: the cursor lands on
    // the tool call, so its result would be orphaned at the window head.
    let entries = [
        (EntryKind::User, "a".repeat(900)),
        (EntryKind::Assistant, "b".repeat(900)),
        (EntryKind::Tool, "call X".to_owned()),
        (EntryKind::Tool, "r".repeat(900)),
        (EntryKind::User, "c".repeat(900)),
    ];
    let mut parent: Option<String> = None;
    let mut call_id = String::new();
    for (i, (kind, text)) in entries.iter().enumerate() {
        let e = store
            .append(
                kind.clone(),
                json!({ "text": text, "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        if i == 2 {
            call_id = e.id.clone();
        }
        parent = Some(e.id);
    }
    let all = store.entries_range(0, usize::MAX).unwrap();
    let leaf = store.leaf().unwrap().map(|e| e.id);
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230, // = the retention floor at this threshold
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let state = OmState::from_config(
        &cfg,
        OmRecord {
            cursor: Some(Cursor {
                entry_id: call_id,
                timestamp: 1,
            }),
            ..Default::default()
        },
    );
    let window = state.raw_window_from(&all, leaf.as_deref());
    // The leading tool run is cut (the result would be orphaned from its
    // call); the surviving user entry is the newest, so it anchors nothing
    // and appears exactly once (#334's dedupe rule).
    assert!(window.anchor.is_none(), "{window:?}");
    assert_eq!(window.tail.len(), 1, "{window:?}");
    assert_eq!(window.tail[0].kind, EntryKind::User);
    assert_eq!(window.tail[0].payload["text"], "c".repeat(900));
}

#[test]
fn raw_window_anchors_the_newest_user_entry_when_the_tail_is_all_tool_calls() {
    // #334: the cursor advances past the newest user entry into a tool run,
    // the §4 cut drops that run, and the window would carry no user entry at
    // all — the shape that makes the provider answer "No user query found in
    // messages". The newest user entry rides as the anchor instead.
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    let entries = [
        (EntryKind::User, "a".repeat(900)),
        (EntryKind::Assistant, "b".repeat(900)),
        (EntryKind::Tool, "call X".to_owned()),
        (EntryKind::Tool, "r".repeat(900)),
    ];
    let ids = append_chain(&mut store, &entries);
    let all = store.entries_range(0, usize::MAX).unwrap();
    let leaf = store.leaf().unwrap().map(|e| e.id);
    let state = state_with_cursor_on(&ids[1]);
    let window = state.raw_window_from(&all, leaf.as_deref());
    assert!(window.tail.is_empty(), "{window:?}");
    let anchor = window.anchor.expect("the newest user entry anchors");
    assert_eq!(anchor.kind, EntryKind::User);
    assert_eq!(anchor.payload["text"], "a".repeat(900));
    assert_eq!(anchor.id, ids[0], "the ENTRY is pinned, not the range");
}

#[test]
fn raw_window_anchors_the_newest_user_entry_not_an_older_one() {
    // Sessions carry many user entries (steering, follow-ups, parent
    // messages). The anchor is the newest, so the model is pinned to the
    // instruction it is working on, and no older entry is re-anchored.
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    let entries = [
        (EntryKind::User, "brief".repeat(300)),
        (EntryKind::Assistant, "b".repeat(900)),
        (EntryKind::User, "steer".repeat(300)),
        (EntryKind::Assistant, "d".repeat(900)),
        (EntryKind::Tool, "call X".to_owned()),
        (EntryKind::Tool, "r".repeat(900)),
    ];
    let ids = append_chain(&mut store, &entries);
    let all = store.entries_range(0, usize::MAX).unwrap();
    let leaf = store.leaf().unwrap().map(|e| e.id);
    let state = state_with_cursor_on(&ids[3]);
    let window = state.raw_window_from(&all, leaf.as_deref());
    assert!(window.tail.is_empty(), "{window:?}");
    let anchor = window.anchor.expect("the newest user entry anchors");
    assert_eq!(anchor.id, ids[2], "the newest, not the brief");
    assert_eq!(anchor.payload["text"], "steer".repeat(300));
}

#[test]
fn raw_window_anchor_never_orphans_a_tool_result() {
    // The §4 cut runs on the tail before the anchor is chosen, so an anchored
    // window opens on a user message: the tool run the cursor advanced into is
    // dropped, not carried behind the anchor separated from its call.
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    let entries = [
        (EntryKind::User, "a".repeat(900)),
        (EntryKind::User, "c".repeat(900)),
        (EntryKind::Assistant, "d".repeat(900)),
        (EntryKind::Tool, "call X".to_owned()),
        (EntryKind::Tool, "e".repeat(900)),
    ];
    let ids = append_chain(&mut store, &entries);
    let all = store.entries_range(0, usize::MAX).unwrap();
    let leaf = store.leaf().unwrap().map(|e| e.id);
    // The cursor sits on the assistant entry, so the whole tail is a tool run.
    let state = state_with_cursor_on(&ids[2]);
    let window = state.raw_window_from(&all, leaf.as_deref());
    assert!(window.tail.is_empty(), "the tool run is cut: {window:?}");
    let anchor = window.anchor.expect("the newest user entry anchors");
    assert_eq!(anchor.id, ids[1]);
}

fn append_chain(store: &mut SessionStore, entries: &[(EntryKind, String)]) -> Vec<String> {
    let mut parent: Option<String> = None;
    let mut ids = Vec::new();
    for (kind, text) in entries {
        let e = store
            .append(
                kind.clone(),
                json!({ "text": text, "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id.clone());
        ids.push(e.id);
    }
    ids
}

fn state_with_cursor_on(entry_id: &str) -> OmState {
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    OmState::from_config(
        &cfg,
        OmRecord {
            cursor: Some(Cursor {
                entry_id: entry_id.to_owned(),
                timestamp: 1,
            }),
            ..Default::default()
        },
    )
}

#[test]
fn the_assembly_is_bounded_and_the_continuation_hint_is_per_assembly() {
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let mut state = OmState::from_config(
        &cfg,
        OmRecord {
            active_observations: om::wrap_in_observation_group(
                "the log",
                "00000001:00000002",
                "a1",
                None,
            ),
            ..Default::default()
        },
    );
    let first = state.assemble_context("base prompt");
    assert!(first.instructions.starts_with("base prompt"));
    // The observation content rides as `role: system` items, not in
    // `instructions` (#96): the framing stays in the prompt, the volatile
    // content moves to the input.
    assert!(first.instructions.contains(om::OBSERVATION_CONTEXT_PROMPT));
    assert!(!first.instructions.contains("the log"));
    assert!(om_system_content(&first.om_input_items).contains("the log"));
    assert!(
        !first
            .instructions
            .contains(om::OBSERVATION_CONTINUATION_HINT)
    );
    assert!(om_user_content(&first.om_input_items).contains(om::OBSERVATION_CONTINUATION_HINT));
    // Mastra's shape (#334): the hint rides on every assembly while
    // observations exist, so the request always carries a `role: user` item
    // even when the raw window is all tool calls.
    let second = state.assemble_context("base prompt");
    assert!(om_user_content(&second.om_input_items).contains(om::OBSERVATION_CONTINUATION_HINT));
    // Negative: with no observations there is no hint to carry.
    let mut bare = OmState::from_config(&cfg, OmRecord::default());
    assert!(
        !om_user_content(&bare.assemble_context("base prompt").om_input_items)
            .contains(om::OBSERVATION_CONTINUATION_HINT)
    );
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
            .assemble_context("base")
            .instructions
            .contains("demoted prefix")
    );
}

#[test]
fn the_main_agent_view_has_no_boundary_markers() {
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    // A log with a boundary delimiter between two observation groups.
    let log = format!(
        "\n{}\n\n{}\n\n{}",
        om::message_boundary("2025-12-05T09:15:00Z"),
        om::wrap_in_observation_group("first obs", "00000001:00000002", "a", None),
        om::wrap_in_observation_group("second obs", "00000003:00000004", "b", None),
    );
    let record = OmRecord {
        active_observations: log,
        ..Default::default()
    };
    // The stored log keeps the delimiter (reflector/compaction need it);
    assert!(
        record
            .live_observations()
            .contains("--- message boundary (")
    );
    // the main-agent view strips it, keeping content + group wrappers.
    let clean = record.agent_observations();
    assert!(!clean.contains("--- message boundary ("), "{clean}");
    assert!(clean.contains("first obs"), "{clean}");
    assert!(clean.contains("second obs"), "{clean}");
    assert!(clean.contains("<observation-group"), "{clean}");
    // And the assembled main-agent context is clean end-to-end.
    let mut state = OmState::from_config(&cfg, record);
    let ctx = state.assemble_context("base prompt");
    assert!(
        !ctx.instructions.contains("--- message boundary ("),
        "{ctx:?}"
    );
    assert!(
        om_system_content(&ctx.om_input_items).contains("first obs"),
        "{ctx:?}"
    );
    assert!(!ctx.instructions.contains("first obs"), "{ctx:?}");
}

#[test]
fn the_suggested_response_is_injected_once_then_cleared() {
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let mut state = OmState::from_config(
        &cfg,
        OmRecord {
            active_observations: "the log".into(),
            om_suggested_response: "pause and answer the user".into(),
            ..Default::default()
        },
    );
    let first = state.assemble_context("base prompt");
    // The steering signal is handed to the model alongside the hint...
    let first_om = om_user_content(&first.om_input_items);
    // The steering signal is handed to the model alongside the hint...
    assert!(first_om.contains(om::OBSERVATION_CONTINUATION_HINT));
    assert!(first_om.contains("pause and answer the user"), "{first_om}");
    // ...and consumed: the next assembly carries the hint again (it is
    // per-assembly) but must not re-inject the now-stale signal.
    let second = state.assemble_context("base prompt");
    let second_om = om_user_content(&second.om_input_items);
    assert!(second_om.contains(om::OBSERVATION_CONTINUATION_HINT));
    assert!(
        !second_om.contains("pause and answer the user"),
        "{second_om}"
    );
    assert!(state.record.om_suggested_response.is_empty());
}
#[test]
fn the_idle_gap_measures_the_pause_before_the_current_turn() {
    let mk = |id: &str, parent: Option<&str>, ts: u64, kind: EntryKind| Entry {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        timestamp: ts,
        kind,
        payload: json!({ "text": "x" }),
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    };
    // Turn 1 (user + assistant), a 90-second pause, then the current
    // turn (user + assistant).
    let entries = vec![
        mk("00000001", None, 1_000, EntryKind::User),
        mk("00000002", Some("00000001"), 2_000, EntryKind::Assistant),
        mk("00000003", Some("00000002"), 92_000, EntryKind::User),
        mk("00000004", Some("00000003"), 93_000, EntryKind::Assistant),
    ];
    assert_eq!(idle_gap_secs(&entries, Some("00000004")), 90);
    // The gap uses the turn's FIRST user entry: a steering sent inside
    // the turn doesn't shrink the measured pause.
    let entries = vec![
        mk("00000001", None, 1_000, EntryKind::User),
        mk("00000002", Some("00000001"), 2_000, EntryKind::Assistant),
        mk("00000003", Some("00000002"), 92_000, EntryKind::User),
        mk("00000004", Some("00000003"), 92_500, EntryKind::User),
        mk("00000005", Some("00000004"), 93_000, EntryKind::Assistant),
    ];
    assert_eq!(idle_gap_secs(&entries, Some("00000005")), 90);
    // A fresh session (branch starts with the turn's user entry): zero.
    let fresh = vec![
        mk("00000001", None, 1_000, EntryKind::User),
        mk("00000002", Some("00000001"), 2_000, EntryKind::Assistant),
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
        .find(|e| e.kind == EntryKind::SpawnSnapshot)
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
