use super::*;
use crate::agent::{CallOutput, Entry, FunctionCall, InputEntry, KIND_TOOL, KIND_USER};

fn tool_entry(id: &str, call_id: &str, output: &str) -> Entry {
    Entry {
        id: id.to_owned(),
        parent: None,
        timestamp: 0,
        kind: KIND_TOOL.to_owned(),
        payload: tau_protocol::payload::ToolPayload {
            call_id: call_id.to_owned(),
            name: "bash".to_owned(),
            args: Value::Null,
            output: tau_protocol::payload::ToolOutput::Text(output.to_owned()),
            status: None,
        }
        .to_value(),
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    }
}

/// A stored user entry with an optional sender stamp (the #187 `source`).
fn user_entry(id: &str, text: &str, source: Option<&str>) -> Entry {
    Entry {
        id: id.to_owned(),
        parent: None,
        timestamp: 0,
        kind: KIND_USER.to_owned(),
        payload: tau_protocol::payload::UserPayload {
            text: text.to_owned(),
            lane: "steering".to_owned(),
            source: source.map(str::to_owned),
            skill: None,
        }
        .to_value(),
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    }
}

fn user_content(items: &[InputEntry]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            InputEntry::Message(m) if m.role == "user" => Some(m.content.clone()),
            _ => None,
        })
        .collect()
}

fn turn_result_with_call(args: &str) -> TurnResult {
    let mut r = TurnResult::default();
    r.calls.push(FunctionCall {
        id: "id-1".into(),
        call_id: "call-1".into(),
        name: "edit".into(),
        arguments: args.into(),
    });
    r
}

#[test]
fn valid_tool_args_pass_through_unchanged() {
    let mut r = turn_result_with_call(r#"{"file": "a.txt"}"#);
    repair_tool_call_arguments(&mut r);
    assert_eq!(r.calls[0].arguments, r#"{"file": "a.txt"}"#);
}

#[test]
fn single_quote_tool_args_repair_to_intended() {
    let mut r = turn_result_with_call("{'file': 'a.txt'}");
    repair_tool_call_arguments(&mut r);
    let v: Value =
        serde_json::from_str(&r.calls[0].arguments).expect("repaired args must be valid JSON");
    assert_eq!(v, serde_json::json!({"file": "a.txt"}));
}

#[test]
fn trailing_comma_tool_args_repair_to_intended() {
    let mut r = turn_result_with_call(r#"{"a": 1,}"#);
    repair_tool_call_arguments(&mut r);
    let v: Value =
        serde_json::from_str(&r.calls[0].arguments).expect("repaired args must be valid JSON");
    assert_eq!(v, serde_json::json!({"a": 1}));
}

#[test]
fn raw_newline_in_value_is_repaired() {
    // The observed production breakage class: a raw control character
    // (newline) inside a string value is invalid JSON.
    let mut r = turn_result_with_call("{\"content\": \"line1\nline2\"}");
    repair_tool_call_arguments(&mut r);
    let v: Value =
        serde_json::from_str(&r.calls[0].arguments).expect("repaired args must be valid JSON");
    assert!(
        v["content"].is_string(),
        "content should stay a string: {v:?}"
    );
}

#[test]
fn unrepairable_args_are_left_untouched() {
    // If the repair cannot produce valid JSON, the raw string is kept
    // verbatim (run_tools then emits the parse error) — never corrupted.
    let raw = "definitely not json {[";
    let mut r = turn_result_with_call(raw);
    repair_tool_call_arguments(&mut r);
    if serde_json::from_str::<Value>(&r.calls[0].arguments).is_err() {
        assert_eq!(r.calls[0].arguments, raw);
    }
}

#[test]
fn empty_tool_result_is_emitted_as_call_output() {
    // A tool that returns "" still pairs its call: the result phase is
    // emitted even though its text is empty (ticket #37). The old
    // skip-empties rule dropped it and 400'd the next request.
    let entries = vec![tool_entry("e1", "call-1", "")];
    let items = input_items(&entries, None);
    assert!(
        matches!(&items[..], [InputEntry::CallOutput(o)] if o.call_id == "call-1"),
        "expected one call output for the empty result, got {items:?}"
    );
}

#[test]
fn dedupe_keeps_only_the_last_phase_per_call_id() {
    // Two entries share a call_id (the call phase and its result): the
    // dedupe keeps only the last (the result), not both (ticket #37).
    let entries = vec![
        tool_entry("e1", "call-1", ""),
        tool_entry("e2", "call-1", "done"),
    ];
    let items = input_items(&entries, None);
    assert!(
        matches!(
            &items[..],
            [InputEntry::CallOutput(o)]
                if o.call_id == "call-1"
                    && matches!(&o.output, CallOutput::Text(t) if t == "done")
        ),
        "expected exactly the result phase, got {items:?}"
    );
}

#[test]
fn child_entry_from_the_parent_is_labeled_parent() {
    // A `subagent_message` stamps its source with the parent session id:
    // the child's assembly reads it as its parent speaking.
    let entries = vec![user_entry("e1", "tighten the scope", Some("parent-sess"))];
    let items = input_items(&entries, Some("parent-sess"));
    assert_eq!(
        user_content(&items),
        vec!["Parent: tighten the scope".to_owned()]
    );
}

#[test]
fn child_entry_from_the_user_is_labeled_user() {
    // The GUI's child send stamps "user": the child's assembly keeps the
    // human's words distinct from the parent model's.
    let entries = vec![user_entry("e1", "please also check the logs", Some("user"))];
    let items = input_items(&entries, Some("parent-sess"));
    assert_eq!(
        user_content(&items),
        vec!["User: please also check the logs".to_owned()]
    );
}

#[test]
fn child_entry_without_a_source_stays_bare() {
    // The spawn brief (the handoff-in) carries no source: it is the child's
    // own task, not a message from either sender, so it is unlabeled.
    let entries = vec![user_entry("e1", "work your plan file", None)];
    let items = input_items(&entries, Some("parent-sess"));
    assert_eq!(user_content(&items), vec!["work your plan file".to_owned()]);
}

#[test]
fn child_labels_do_not_leak_onto_other_sources() {
    // In a child session a source that is neither the parent nor "user" is
    // not a real sender (children have no sub-agents); it stays bare.
    let entries = vec![user_entry("e1", "hello", Some("someone-else"))];
    let items = input_items(&entries, Some("parent-sess"));
    assert_eq!(user_content(&items), vec!["hello".to_owned()]);
}

#[test]
fn non_child_session_keeps_the_sub_agent_prefix() {
    // A plain (non-child) session keeps the #23 prefix: a child wake's
    // source is stamped into its user entry and prefixed with the source.
    let entries = vec![user_entry(
        "e1",
        "done, output attached",
        Some("parent-sess"),
    )];
    let items = input_items(&entries, None);
    assert_eq!(
        user_content(&items),
        vec!["Sub-agent parent-sess: done, output attached".to_owned()]
    );
}

#[test]
fn non_child_session_user_and_bare_are_untouched() {
    // "user" is a real sender only in a child: a plain session stamps it
    // like any other source, and an unstamped entry stays bare.
    let entries = vec![
        user_entry("e1", "from the gui", Some("user")),
        user_entry("e2", "plain", None),
    ];
    let items = input_items(&entries, None);
    assert_eq!(
        user_content(&items),
        vec![
            "Sub-agent user: from the gui".to_owned(),
            "plain".to_owned()
        ]
    );
}
