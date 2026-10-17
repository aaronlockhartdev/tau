use crate::agent::{
    CallOutput, Entry, FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage,
    KIND_ASSISTANT, KIND_TOOL, KIND_USER, TurnResult, Value,
};
use std::collections::HashMap;

/// ticket #70: repair malformed tool-call arguments at receipt. A call whose
/// arguments fail a strict parse is run through `jsonrepair`; if that yields
/// valid JSON, the repaired string replaces the raw one, so the stored entry
/// and the echoed history are valid (and vLLM stops coercing the argument to
/// `{}`). Valid arguments are never altered. Unrepairable arguments are left
/// as-is — `run_tools` turns those into a parse-error result, never a silent
/// null.
pub(super) fn repair_tool_call_arguments(result: &mut TurnResult) {
    for call in &mut result.calls {
        if serde_json::from_str::<Value>(&call.arguments).is_ok() {
            continue;
        }
        match jsonrepair_rs::jsonrepair(&call.arguments) {
            Ok(repaired) if serde_json::from_str::<Value>(&repaired).is_ok() => {
                tracing::info!(
                    tool = %call.name,
                    raw_len = call.arguments.len(),
                    repaired_len = repaired.len(),
                    "repaired malformed tool-call arguments"
                );
                call.arguments = repaired;
            }
            _ => {}
        }
    }
}

/// The watcher-parsed plan state appended to the assembled instructions
/// (the design's re-injection); `None` = the session has no observed plan
/// state yet, and the assembly is untouched.
pub(super) fn with_plan_state(mut instructions: String, plan: Option<&str>) -> String {
    if let Some(plan) = plan {
        instructions.push_str("\n\n");
        instructions.push_str(plan);
    }
    instructions
}

/// The session's entries (file order) mapped to responses-API input items:
/// user messages, assistant text + calls, and tool results (spec §5.4).
pub(super) fn input_items(entries: &[Entry]) -> Vec<InputEntry> {
    // Two-phase tool recording writes a call-phase entry (empty output)
    // before its result-phase entry, so the last tool entry per call_id in
    // file order is the result. Emit exactly that one — a per-call_id
    // last-wins dedupe (ticket #37) — so a tool that returns "" still pairs
    // its function_call with an output and the Responses API never 400s on
    // a missing output.
    let last_tool = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let cid = e.payload.get("call_id").and_then(Value::as_str)?;
            e.payload
                .get("output")
                .and_then(CallOutput::from_payload)
                .is_some()
                .then(|| (cid.to_owned(), i))
        })
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        match entry.kind.as_str() {
            KIND_USER => {
                if let Some(text) = entry.payload.get("text").and_then(Value::as_str) {
                    // A wake message from a child (ticket #23) is stored as a
                    // user entry with its source: the prefix keeps the parent
                    // from reading the child's words as the user's.
                    let content = match entry.payload.get("source").and_then(Value::as_str) {
                        Some(source) => format!("Sub-agent {source}: {text}"),
                        None => text.to_owned(),
                    };
                    out.push(InputEntry::Message(InputMessage {
                        role: "user".into(),
                        content,
                    }));
                }
            }
            KIND_ASSISTANT => {
                if let Some(text) = entry
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    out.push(InputEntry::Message(InputMessage {
                        role: "assistant".into(),
                        content: text.to_owned(),
                    }));
                }
                if let Some(calls) = entry.payload.get("calls").and_then(Value::as_array) {
                    for call in calls {
                        out.push(InputEntry::Call(FunctionCallInput {
                            kind: "function_call",
                            id: call
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            call_id: call
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            name: call
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            arguments: call
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        }));
                    }
                }
            }
            KIND_TOOL => {
                let (Some(call_id), Some(output)) = (
                    entry.payload.get("call_id").and_then(Value::as_str),
                    entry
                        .payload
                        .get("output")
                        .and_then(CallOutput::from_payload),
                ) else {
                    continue;
                };
                // Only the last recorded phase per call_id is an input item
                // (the result); an earlier phase for the same call_id is the
                // call, not a second result.
                if last_tool.get(call_id) != Some(&i) {
                    continue;
                }
                out.push(InputEntry::CallOutput(FunctionCallOutputInput {
                    kind: "function_call_output",
                    call_id: call_id.to_owned(),
                    output,
                }));
            }
            _ => {}
        }
    }
    out
}
