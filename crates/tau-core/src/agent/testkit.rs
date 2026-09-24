use super::*;
use crate::provider::canned;

pub(crate) fn session_in(dir: &std::path::Path) -> SessionStore {
    let mut store = SessionStore::for_workspace(dir, "s1");
    store.create().unwrap();
    store
}

pub(crate) fn make_agent(dir: &std::path::Path, provider: TurnProviderRef) -> AgentSession {
    AgentSession::new(SessionParams {
        store: session_in(dir),
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    })
}

pub(crate) fn sse(text: &str, calls: &[(String, String, String)]) -> String {
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
        body.push_str(&format!(
            "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\n"
        ));
    }
    if !text.is_empty() {
        body.push_str(&format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n"
        ));
    }
    body.push_str(
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
    );
    body.push_str("data: [DONE]\n\n");
    body
}

/// Like sse(), but the text is JSON-escaped (multi-line deltas).
pub(crate) fn sse_json(text: &str) -> String {
    let delta = json!({ "type": "response.output_text.delta", "delta": text });
    let done = json!({
        "type": "response.completed",
        "response": { "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } }
    });
    format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", delta, done)
}

/// A canned provider scripted per call: each entry is (sse body, calls).
/// `seen` captures (instructions, first input message content) per call.
pub(crate) struct ScriptedProvider {
    calls: Vec<String>,
    index: std::sync::atomic::AtomicUsize,
    pub(crate) seen: std::sync::Mutex<Vec<(Option<String>, Option<String>)>>,
}

impl ScriptedProvider {
    pub(crate) fn new(calls: Vec<String>) -> Self {
        Self {
            calls,
            index: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
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
            .get(self.index.fetch_add(1, Ordering::SeqCst) % self.calls.len())
            .cloned()
            .unwrap_or_else(|| sse("", &[]));
        let captured = serde_json::to_value(request).unwrap();
        self.seen.lock().unwrap().push((
            captured
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned),
            captured
                .get("input")
                .and_then(|i| i.get(0))
                .and_then(|i| i.get("content"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        ));
        let provider = canned(&body);
        provider.call(request, sink)
    }
}

pub(crate) fn entries_of(store: &SessionStore) -> Vec<Entry> {
    store.entries_range(0, usize::MAX).unwrap()
}
