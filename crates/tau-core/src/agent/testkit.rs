use std::fmt::Write as _;

use super::*;
use crate::harness::SessionRole;
use crate::provider::canned;

pub(crate) fn session_in(dir: &std::path::Path) -> SessionStore {
    let mut store = SessionStore::for_workspace(dir, "s1");
    store.create().unwrap();
    store
}

pub(crate) fn make_agent(dir: &std::path::Path, provider: TurnProviderRef) -> Arc<AgentSession> {
    AgentSession::launch(
        session_in(dir),
        SessionRole::Bare {
            provider,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::surface::bare_specs(),
            cwd: dir.to_path_buf(),
            turn: TurnConfig::default(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            om: None,
            om_model: String::new(),
            gates: None,
        },
    )
    .unwrap()
}

pub(crate) fn sse(text: &str, calls: &[(String, String, String)]) -> String {
    sse_charged(text, calls, Some(1))
}

/// Like `sse()`, with the provider's charge set: `input_tokens` is the count a
/// turn anchors its next estimate on (plan §6.9); None omits the usage object,
/// the stream that reports no counts at all.
pub(crate) fn sse_charged(
    text: &str,
    calls: &[(String, String, String)],
    input_tokens: Option<u64>,
) -> String {
    // (name, call_id, arguments-json)
    let mut body = String::new();
    for (name, call_id, args) in calls {
        let item = serde_json::json!({
            "id": call_id,
            "type": "function_call",
            "name": name,
            "call_id": call_id,
            "arguments": args,
        });
        let _ = write!(
            body,
            "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\n"
        );
    }
    if !text.is_empty() {
        let _ = write!(
            body,
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n"
        );
    }
    let response = match input_tokens {
        Some(n) => format!(
            "{{\"usage\":{{\"input_tokens\":{n},\"output_tokens\":1,\"total_tokens\":{}}}}}",
            n.saturating_add(1)
        ),
        None => "{}".to_owned(),
    };
    let _ = write!(
        body,
        "data: {{\"type\":\"response.completed\",\"response\":{response}}}\n\n",
    );
    body.push_str("data: [DONE]\n\n");
    body
}

/// Like `sse()`, but the text is JSON-escaped (multi-line deltas).
pub(crate) fn sse_json(text: &str) -> String {
    let delta = serde_json::json!({ "type": "response.output_text.delta", "delta": text });
    let done = serde_json::json!({
        "type": "response.completed",
        "response": { "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } }
    });
    format!("data: {delta}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

/// A canned provider scripted per call: each entry is (sse body, calls).
/// `seen` captures (instructions, all input message contents joined) per call.
/// `user_contents` captures the `role: user` message contents per call — the
/// seam the "every request carries a user query" assertions read (#334).
pub(crate) struct ScriptedProvider {
    calls: Vec<String>,
    index: std::sync::atomic::AtomicUsize,
    pub(crate) seen: parking_lot::Mutex<Vec<(Option<String>, Option<String>)>>,
    pub(crate) user_contents: parking_lot::Mutex<Vec<Vec<String>>>,
    /// The `model` field of each request body as it went on the wire (§6.7).
    pub(crate) models: parking_lot::Mutex<Vec<String>>,
}

impl ScriptedProvider {
    pub(crate) fn new(calls: Vec<String>) -> Self {
        Self {
            calls,
            index: std::sync::atomic::AtomicUsize::new(0),
            seen: parking_lot::Mutex::new(Vec::new()),
            user_contents: parking_lot::Mutex::new(Vec::new()),
            models: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl crate::provider::TurnProvider for ScriptedProvider {
    fn call<'a>(
        &self,
        request: &ResponseRequest,
        sink: &'a mut dyn TurnSink,
    ) -> crate::provider::ProviderTurn<'a> {
        let body = self
            .calls
            .get(self.index.fetch_add(1, Ordering::SeqCst))
            .cloned()
            .unwrap_or_else(|| sse("", &[]));
        let captured = serde_json::to_value(request).unwrap();
        self.models.lock().push(
            captured
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
        self.seen.lock().push((
            captured
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned),
            captured
                .get("input")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| i.get("content").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
        ));
        self.user_contents.lock().push(
            captured
                .get("input")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter(|i| i.get("role").and_then(Value::as_str) == Some("user"))
                        .filter_map(|i| i.get("content").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        );
        let provider = canned(&body);
        provider.call(request, sink)
    }
}

// The whole log by design (#302): a test asks what the session recorded, and
// the fork child's records include the parent's copied tree (spec §3).
pub(crate) fn entries_of(store: &SessionStore) -> Vec<Entry> {
    store.entries_range(0, usize::MAX).unwrap()
}
