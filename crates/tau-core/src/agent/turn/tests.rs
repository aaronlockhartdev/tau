use super::*;

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
    let items = input_items(&entries);
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
    let items = input_items(&entries);
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
