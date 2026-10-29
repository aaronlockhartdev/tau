use crate::agent::{
    CallOutput, Entry, FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage,
    TurnResult, Value,
};
use std::collections::HashMap;
use tau_protocol::EntryKind;

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

/// One assembly's parts together with the branch they were read from. The
/// entries travel with the items so the prompt estimate can anchor on the last
/// request's provider count and price only what was appended after it (plan
/// §6.9); `om_items` is how many *leading* items the observation state
/// contributed — assembly always prepends them, so `items[..om_items]` is that
/// contribution, and a compaction is what makes it differ from zero.
pub(super) struct Assembled {
    /// The model this request addresses, so a value resolved for another model
    /// fails beside it (plan §6.8).
    pub model: String,
    pub instructions: String,
    pub items: Vec<InputEntry>,
    pub entries: Vec<Entry>,
    pub om_items: usize,
    /// A child session's parent, whose name decorates its user entries
    /// (ADR-0001 supplement); the tail estimate applies the same labels.
    pub parent_session: Option<String>,
}

impl Assembled {
    /// Whether the assembled input answers a query: at least one `role: user`
    /// item (#334). A chat template is allowed to refuse a request with none —
    /// the string is a template rule, not a wire-protocol rule — so the
    /// harness owns this check, not the provider.
    #[must_use]
    pub fn has_user_query(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, InputEntry::Message(m) if m.role == "user"))
    }

    /// The entry the request this assembly produces ends at: the last entry in
    /// file order, which is the branch's leaf unless a branch adopt reordered it.
    #[must_use]
    pub fn leaf(&self) -> Option<&str> {
        self.entries.last().map(|e| e.id.as_str())
    }

    /// The branch entries appended after `leaf`, or None once the branch no
    /// longer carries it — a compaction consumed it or a fork moved the branch.
    pub fn tail_after(&self, leaf: &str) -> Option<&[Entry]> {
        self.entries
            .iter()
            .rposition(|e| e.id == leaf)
            .map(|i| &self.entries[i + 1..])
    }
}

/// The session's entries (file order) mapped to responses-API input items:
/// user messages, assistant text + calls, and tool results (spec §5.4).
///
/// `parent_session` marks a child session (the child's link names its
/// parent): its user entries are decorated at assembly time with the
/// sender's label — `Parent:` when the entry's source is the parent
/// session (a `subagent_message` or resume), `User:` when it is the
/// human (the GUI's child send), bare when unstamped (the spawn brief's
/// handoff-in) — while a non-child session keeps the #23 `Sub-agent
/// {source}:` prefix for child wake messages. Storage stays clean; only
/// the assembled model context carries the labels (ADR-0001 supplement).
pub(super) fn input_items(entries: &[Entry], parent_session: Option<&str>) -> Vec<InputEntry> {
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
        match entry.kind {
            EntryKind::User => {
                if let Some(text) = entry.payload.get("text").and_then(Value::as_str) {
                    // Sender attribution, applied at assembly time
                    // (ADR-0001 supplement): a child session labels the
                    // entry's source — its parent (Parent:), the user
                    // (User:), or none (bare); a non-child session keeps
                    // the #23 prefix for child wake messages.
                    let is_child = parent_session.is_some();
                    let content = match entry.payload.get("source").and_then(Value::as_str) {
                        Some(source) if is_child && Some(source) == parent_session => {
                            format!("Parent: {text}")
                        }
                        Some("user") if is_child => format!("User: {text}"),
                        Some(source) if !is_child => {
                            format!("Sub-agent {source}: {text}")
                        }
                        _ => text.to_owned(),
                    };
                    out.push(InputEntry::Message(InputMessage {
                        role: "user".into(),
                        content,
                    }));
                }
            }
            EntryKind::Assistant => {
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
                out.extend(call_inputs(entry).map(InputEntry::Call));
            }
            EntryKind::Tool => {
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
            // Record kinds carry no prompt material; listed so a new kind
            // forces a decision here rather than defaulting silently.
            EntryKind::System
            | EntryKind::Om
            | EntryKind::Subagent
            | EntryKind::SpawnSnapshot
            | EntryKind::Diagnostic
            | EntryKind::Other(_) => {}
        }
    }
    out
}

/// The assistant entry's recorded calls as `function_call` inputs. Fields the
/// provider never supplied default to empty (a call with no id still needs a
/// placeholder to pair with its output).
fn call_inputs(entry: &Entry) -> impl Iterator<Item = FunctionCallInput> {
    entry
        .payload
        .get("calls")
        .and_then(Value::as_array)
        .map(|calls| calls.iter())
        .into_iter()
        .flatten()
        .map(|call| FunctionCallInput {
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
        })
}
