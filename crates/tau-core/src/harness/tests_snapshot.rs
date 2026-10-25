use super::*;

use serde_json::json;
use tau_protocol::EntryKind;

use crate::session::SessionStore;

fn entry(kind: EntryKind, payload: serde_json::Value) -> Entry {
    Entry {
        id: "e1".into(),
        parent: None,
        timestamp: 1,
        kind,
        payload,
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    }
}

#[test]
fn preview_reads_the_first_line_and_caps_at_80_chars() {
    let e = entry(EntryKind::User, json!({ "text": "short" }));
    assert_eq!(snapshot::preview(&e), "short");

    let long = "x".repeat(90);
    let e = entry(EntryKind::User, json!({ "text": long }));
    let out = snapshot::preview(&e);
    assert_eq!(out.chars().count(), 81); // 80 chars + the ellipsis
    assert!(out.ends_with('…'));

    // Only the first line: the rest is dropped.
    let e = entry(EntryKind::User, json!({ "text": "first\nsecond" }));
    assert_eq!(snapshot::preview(&e), "first");
}

#[test]
fn preview_falls_back_through_note_state_output_name() {
    assert_eq!(
        snapshot::preview(&entry(EntryKind::System, json!({ "note": "a note" }))),
        "a note"
    );
    assert_eq!(
        snapshot::preview(&entry(EntryKind::Subagent, json!({ "state": "running" }))),
        "running"
    );
    assert_eq!(
        snapshot::preview(&entry(EntryKind::Tool, json!({ "output": "the output" }))),
        "the output"
    );
    assert_eq!(
        snapshot::preview(&entry(EntryKind::Tool, json!({ "name": "bash" }))),
        "bash"
    );
}

#[test]
fn preview_of_an_entry_without_a_string_field_is_empty() {
    let e = entry(EntryKind::Tool, json!({ "args": { "n": 1 } }));
    assert_eq!(snapshot::preview(&e), "");
}

#[test]
fn entry_meta_marks_an_interrupted_assistant() {
    let e = entry(
        EntryKind::Assistant,
        json!({ "text": "partial", "interrupted": true }),
    );
    let meta = snapshot::entry_meta(&e, 42);
    assert_eq!(meta.status, EntryStatus::Interrupted);
    assert_eq!(meta.size, 42);
    assert_eq!(meta.preview, "partial");

    let e = entry(
        EntryKind::Assistant,
        json!({ "text": "whole", "interrupted": false }),
    );
    assert_eq!(snapshot::entry_meta(&e, 7).status, EntryStatus::Ok);
}

#[test]
fn usage_of_reads_the_core_shape_and_cached_tokens() {
    let u = snapshot::usage_of(&json!({
        "input_tokens": 10,
        "output_tokens": 5,
        "total_tokens": 15,
        "prompt_tokens_details": { "cached_tokens": 3 },
    }));
    assert_eq!(
        u,
        Some(Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cached_prompt_tokens: 3,
        })
    );

    // No details: the cached count is zero, not an error.
    let u = snapshot::usage_of(&json!({
        "input_tokens": 1,
        "output_tokens": 2,
        "total_tokens": 3,
    }));
    assert_eq!(u.unwrap().cached_prompt_tokens, 0);

    // `#[serde(default)]`: any object parses (zeros when the fields are
    // absent); a non-object is not a usage.
    assert_eq!(snapshot::usage_of(&json!("not an object")), None);
}

#[test]
fn model_note_is_quiet_on_the_first_change_and_arrow_after() {
    assert_eq!(snapshot::model_note(None, "gpt-5"), "model: gpt-5");
    assert_eq!(
        snapshot::model_note(Some("gpt-5"), "claude-opus-4-8"),
        "model: gpt-5 → claude-opus-4-8"
    );
}

#[test]
fn last_model_note_is_the_newest_model_on_the_active_branch() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    assert_eq!(snapshot::last_model_note(&mut store), None);

    store
        .append(EntryKind::System, json!({ "note": "model: gpt-5" }), None)
        .unwrap();
    assert_eq!(snapshot::last_model_note(&mut store), Some("gpt-5".into()));

    let first_id = store.leaf().unwrap().unwrap().id;
    // A second change: the note carries the arrow, and the newest side wins.
    store
        .append(
            EntryKind::System,
            json!({ "note": "model: gpt-5 → claude-opus-4-8" }),
            Some(first_id.as_str()),
        )
        .unwrap();
    assert_eq!(
        snapshot::last_model_note(&mut store),
        Some("claude-opus-4-8".into())
    );

    // Non-model system entries are skipped.
    let second_id = store.leaf().unwrap().unwrap().id;
    store
        .append(
            EntryKind::System,
            json!({ "note": "something else" }),
            Some(second_id.as_str()),
        )
        .unwrap();
    assert_eq!(
        snapshot::last_model_note(&mut store),
        Some("claude-opus-4-8".into())
    );
}
