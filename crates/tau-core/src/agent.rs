//! The agent loop and the message lanes (spec §2, §7): one turn = system
//! prompt + history → provider call → tool dispatch → repeat until the model
//! ends the turn. Lanes: **force** (kill the in-flight stream, partial kept
//! as an `interrupted` entry, tool batch completed per config, message at
//! the head of the queue), **steering** (delivered at the next LLM call),
//! **follow-up** (delivered after the turn completes).

use crate::config::ToolBatchPolicy;
use crate::provider::{
    FunctionCall, FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage,
    ReasoningEffort, ResponseRequest, ToolSpec, TurnProviderRef, TurnResult, TurnSink,
};
use crate::session::{Entry, SessionStore};
use crate::tools;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A message lane (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Force,
    Steering,
    FollowUp,
}

/// The session entry kinds the loop defines on top of the storage (spec §3).
pub const KIND_USER: &str = "user";
pub const KIND_ASSISTANT: &str = "assistant";
pub const KIND_TOOL: &str = "tool";
pub const KIND_SYSTEM: &str = "system";

/// Runaway guard: a model that never stops calling tools.
const MAX_ROUNDS: usize = 32;

/// A loop-level failure: session storage or the provider.
#[derive(Debug)]
pub enum AgentError {
    Session(crate::session::Error),
    Provider(crate::provider::ProviderError),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(e) => write!(f, "session: {e}"),
            Self::Provider(e) => write!(f, "provider: {e}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<crate::session::Error> for AgentError {
    fn from(e: crate::session::Error) -> Self {
        Self::Session(e)
    }
}

impl From<crate::provider::ProviderError> for AgentError {
    fn from(e: crate::provider::ProviderError) -> Self {
        Self::Provider(e)
    }
}

/// Per-turn provider limits (v0: fixed for the session's life; the hosted
/// test model is a thinking model, so capping reasoning is how live tests
/// stay cheap).
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnConfig {
    pub max_output_tokens: Option<u64>,
    pub reasoning: Option<ReasoningEffort>,
}

#[derive(Clone)]
struct Queued {
    text: String,
    lane: Lane,
}

fn lane_name(lane: Lane) -> &'static str {
    match lane {
        Lane::Force => "force",
        Lane::Steering => "steering",
        Lane::FollowUp => "follow-up",
    }
}

struct Inner {
    store: SessionStore,
    system_prompt: String,
    model: String,
    tools: Vec<ToolSpec>,
    cwd: PathBuf,
    tool_batch_on_force: ToolBatchPolicy,
    turn: TurnConfig,
    queue: VecDeque<Queued>,
}

/// One session's agent loop. Single-writer per session (spec §2): the GUI
/// (or a parent agent) drives it with `send` + `process`; the kill flag is
/// the only cross-thread surface (a force from the GUI's thread).
/// Everything an agent session needs to run, grouped so the constructor
/// stays a single parameter as the ticket series grows the loop (spec §5).
pub struct SessionParams {
    pub store: SessionStore,
    /// The fully assembled system prompt: the caller builds it (agent-type
    /// prompt + context files via `context::assemble`) — the loop takes the
    /// result and performs no discovery of its own.
    pub system_prompt: String,
    pub model: String,
    pub tools: Vec<ToolSpec>,
    pub cwd: PathBuf,
    pub provider: TurnProviderRef,
    pub tool_batch_on_force: ToolBatchPolicy,
    pub turn: TurnConfig,
}

pub struct AgentSession {
    inner: Mutex<Inner>,
    provider: TurnProviderRef,
    kill: Arc<AtomicBool>,
}

impl AgentSession {
    pub fn new(p: SessionParams) -> Self {
        Self {
            inner: Mutex::new(Inner {
                store: p.store,
                system_prompt: p.system_prompt,
                model: p.model,
                tools: p.tools,
                cwd: p.cwd,
                tool_batch_on_force: p.tool_batch_on_force,
                turn: p.turn,
                queue: VecDeque::new(),
            }),
            provider: p.provider,
            kill: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Queue a user message on its lane (spec §7). A force kills the
    /// in-flight stream and jumps to the head of the queue; everything else
    /// keeps its position.
    pub fn send(&self, text: impl Into<String>, lane: Lane) {
        let mut inner = self.inner.lock().unwrap();
        let msg = Queued {
            text: text.into(),
            lane,
        };
        if lane == Lane::Force {
            self.kill.store(true, Ordering::SeqCst);
            inner.queue.push_front(msg);
        } else {
            inner.queue.push_back(msg);
        }
    }

    /// Run turns until the queue is empty.
    pub async fn process(&self) -> Result<(), AgentError> {
        loop {
            let Some(starter) = self.inner.lock().unwrap().queue.pop_front() else {
                return Ok(());
            };
            self.append_user(starter).await?;
            self.run_turn().await?;
        }
    }

    async fn run_turn(&self) -> Result<(), AgentError> {
        let mut rounds = 0usize;
        loop {
            rounds += 1;
            if rounds > MAX_ROUNDS {
                // A runaway turn dies visibly, not silently: the session
                // records why it stopped.
                self.append(
                    KIND_SYSTEM,
                    json!({
                        "note": format!(
                            "stopped after {MAX_ROUNDS} tool rounds without the model ending the turn"
                        )
                    }),
                )?;
                break;
            }
            // Steering and forced messages ride this LLM call (spec §7);
            // follow-ups wait for the turn boundary.
            let delivered = {
                let mut inner = self.inner.lock().unwrap();
                let mut out = Vec::new();
                inner.queue.retain(|m| {
                    if matches!(m.lane, Lane::Steering | Lane::Force) {
                        out.push(m.clone());
                        false
                    } else {
                        true
                    }
                });
                out
            };
            for msg in delivered {
                self.append_user(msg).await?;
            }

            let entries = {
                let inner = self.inner.lock().unwrap();
                inner
                    .store
                    .entries_range(0, usize::MAX)
                    .map_err(AgentError::Session)?
            };

            let input = input_items(&entries);
            let mut request = ResponseRequest::new(
                self.model().to_owned(),
                Some(self.system_prompt().as_str()),
                input,
            )
            .with_tools(self.tools().clone());
            if let Some(n) = self.turn_config().max_output_tokens {
                request = request.with_max_output_tokens(n);
            }
            if let Some(e) = self.turn_config().reasoning {
                request = request.with_reasoning(e);
            }

            // A force has already killed the stream it targeted; every
            // fresh call starts un-killed (spec §7).
            self.kill.store(false, Ordering::SeqCst);
            let mut sink = KillSink(self.kill.clone());
            let result = self.provider.call(&request, &mut sink).await?;
            self.append_assistant(&result)?;

            if !result.completed {
                // A force-kill: per the policy, the in-flight tool batch is
                // let to complete, then one final call so the model sees the
                // tool results alongside the forced message (spec §7).
                if self.tool_batch_on_force() == ToolBatchPolicy::Complete
                    && !result.calls.is_empty()
                {
                    self.run_tools(&result.calls).await?;
                    continue;
                }
                break;
            }
            if result.calls.is_empty() {
                break;
            }
            self.run_tools(&result.calls).await?;
        }
        Ok(())
    }

    async fn run_tools(&self, calls: &[FunctionCall]) -> Result<(), AgentError> {
        for call in calls {
            let args = serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null);
            let tc = tools::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args: args.clone(),
            };
            let output = tools::dispatch(&self.cwd(), &tc).await;
            self.append(
                KIND_TOOL,
                json!({
                    "call_id": call.call_id,
                    "name": call.name,
                    "args": args,
                    "output": output,
                }),
            )?;
        }
        Ok(())
    }

    async fn append_user(&self, msg: Queued) -> Result<(), AgentError> {
        self.append(
            KIND_USER,
            json!({ "text": msg.text, "lane": lane_name(msg.lane) }),
        )
    }

    fn append_assistant(&self, result: &TurnResult) -> Result<(), AgentError> {
        // A partial cut before anything arrived has nothing to record
        // (review N10): an empty, call-less interrupted entry is noise.
        if !result.completed
            && result.text.is_empty()
            && result.reasoning.is_empty()
            && result.calls.is_empty()
        {
            return Ok(());
        }
        self.append(
            KIND_ASSISTANT,
            json!({
                "text": result.text,
                "reasoning": result.reasoning,
                "interrupted": !result.completed,
                "usage": result.usage,
                "calls": result.calls,
            }),
        )
    }

    fn append(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
        // A fresh session has no leaf yet; the first entry starts the
        // branch (None parent).
        // A fresh session has no leaf; the first entry starts the branch.
        // Any other failure is a storage error and propagates.
        let parent = match inner.store.leaf() {
            Ok(leaf) => leaf.map(|e| e.id),
            Err(e) => return Err(AgentError::Session(e)),
        };
        let parent = parent.as_deref();
        inner.store.append(kind, payload, parent)?;
        Ok(())
    }

    fn model(&self) -> String {
        self.inner.lock().unwrap().model.clone()
    }

    fn system_prompt(&self) -> String {
        self.inner.lock().unwrap().system_prompt.clone()
    }

    fn tools(&self) -> Vec<ToolSpec> {
        self.inner.lock().unwrap().tools.clone()
    }

    fn cwd(&self) -> PathBuf {
        self.inner.lock().unwrap().cwd.clone()
    }

    fn tool_batch_on_force(&self) -> ToolBatchPolicy {
        self.inner.lock().unwrap().tool_batch_on_force
    }

    fn turn_config(&self) -> TurnConfig {
        self.inner.lock().unwrap().turn
    }
}

/// The sink the loop gives the provider: a set kill flag stops the stream
/// (spec §7 force).
struct KillSink(Arc<AtomicBool>);

impl TurnSink for KillSink {
    fn event(&mut self, _: crate::provider::TurnEvent) -> bool {
        !self.0.load(Ordering::SeqCst)
    }
}

/// The session's entries (file order) mapped to responses-API input items:
/// user messages, assistant text + calls, and tool results (spec §5.4).
fn input_items(entries: &[Entry]) -> Vec<InputEntry> {
    let mut out = Vec::new();
    for entry in entries {
        match entry.kind.as_str() {
            KIND_USER => {
                if let Some(text) = entry.payload.get("text").and_then(Value::as_str) {
                    out.push(InputEntry::Message(InputMessage {
                        role: "user".into(),
                        content: text.to_owned(),
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
                    entry.payload.get("output").and_then(Value::as_str),
                ) else {
                    continue;
                };
                out.push(InputEntry::CallOutput(FunctionCallOutputInput {
                    kind: "function_call_output",
                    call_id: call_id.to_owned(),
                    output: output.to_owned(),
                }));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{canned, canned_cut};

    fn session_in(dir: &std::path::Path) -> SessionStore {
        let mut store = SessionStore::for_workspace(dir, "s1");
        store.create().unwrap();
        store
    }

    fn make_agent(dir: &std::path::Path, provider: TurnProviderRef) -> AgentSession {
        AgentSession::new(SessionParams {
            store: session_in(dir),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.to_path_buf(),
            provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
        })
    }

    fn sse(text: &str, calls: &[(String, String, String)]) -> String {
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

    /// A canned provider scripted per call: each entry is (sse body, calls).
    struct ScriptedProvider {
        calls: Vec<String>,
        index: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedProvider {
        fn new(calls: Vec<String>) -> Self {
            Self {
                calls,
                index: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl crate::provider::TurnProvider for ScriptedProvider {
        fn call<'a>(
            &self,
            _request: &ResponseRequest,
            sink: &'a mut dyn TurnSink,
        ) -> crate::provider::ProviderTurn<'a> {
            let body = self
                .calls
                .get(self.index.fetch_add(1, Ordering::SeqCst) % self.calls.len())
                .cloned()
                .unwrap_or_else(|| sse("", &[]));
            let provider = canned(&body);
            provider.call(_request, sink)
        }
    }

    fn entries_of(store: &SessionStore) -> Vec<Entry> {
        store.entries_range(0, usize::MAX).unwrap()
    }

    #[tokio::test]
    async fn a_plain_turn_appends_user_then_assistant() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
        let agent = make_agent(dir.path(), provider);
        agent.send("hello", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        assert_eq!(
            entries.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            vec![KIND_USER, KIND_ASSISTANT]
        );
        assert_eq!(entries[0].payload["text"], "hello");
        assert_eq!(entries[1].payload["text"], "done");
        assert!(!entries[1].payload["interrupted"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn tool_calls_roundtrip_through_the_session() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
        let body1 = sse(
            "",
            &[("read".into(), "c1".into(), r#"{"path":"n.txt"}"#.into())],
        );
        let body2 = sse("read it", &[]);
        let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
        let agent = make_agent(dir.path(), provider);
        agent.send("read n.txt", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![KIND_USER, KIND_ASSISTANT, KIND_TOOL, KIND_ASSISTANT]
        );
        assert_eq!(entries[2].payload["call_id"], "c1");
        assert_eq!(entries[2].payload["name"], "read");
        assert!(entries[2].payload["output"].as_str().unwrap().contains("a"));
    }

    #[tokio::test]
    async fn steering_lands_on_the_next_llm_call() {
        let dir = tempfile::tempdir().unwrap();
        let body1 = sse(
            "",
            &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
        );
        let body2 = sse("after steering", &[]);
        let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
        let agent = make_agent(dir.path(), provider);
        agent.send("start", Lane::FollowUp);
        agent.send("steer me", Lane::Steering);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        // The steering message rides the next LLM call: it is a user entry
        // delivered at the head of the same turn as its starter.
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                KIND_USER,
                KIND_USER,
                KIND_ASSISTANT,
                KIND_TOOL,
                KIND_ASSISTANT
            ]
        );
        assert_eq!(entries[1].payload["text"], "steer me");
        assert_eq!(entries[1].payload["lane"], "steering");
    }

    #[tokio::test]
    async fn follow_up_lands_after_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("first done", &[]),
            sse("second done", &[]),
        ]));
        let agent = make_agent(dir.path(), provider);
        agent.send("first", Lane::FollowUp);
        agent.send("later", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![KIND_USER, KIND_ASSISTANT, KIND_USER, KIND_ASSISTANT]
        );
        assert_eq!(entries[1].payload["text"], "first done");
        // The follow-up starts the *next* turn: its user entry sits between
        // the two assistant entries, not inside the first turn.
        assert_eq!(entries[2].payload["text"], "later");
        assert_eq!(entries[3].payload["text"], "second done");
    }

    #[tokio::test]
    async fn force_kills_the_stream_and_keeps_the_partial() {
        let dir = tempfile::tempdir().unwrap();
        // A long stream the force cuts after two text events; no calls, so
        // the turn ends at the kill.
        let body = sse("a", &[]) + sse("b", &[]).as_str();
        let provider = canned_cut(&body, 1);
        let agent = make_agent(dir.path(), provider);
        agent.send("go", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let assistant = entries.iter().find(|e| e.kind == KIND_ASSISTANT).unwrap();
        assert!(assistant.payload["interrupted"].as_bool().unwrap());
        assert_eq!(assistant.payload["text"], "a");
    }

    #[tokio::test]
    async fn force_preempts_a_queued_steering_at_the_head_of_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("killed", &[]),
            sse("next", &[]),
        ]));
        let agent = make_agent(dir.path(), provider);
        agent.send("go", Lane::FollowUp);
        agent.send("queued steering", Lane::Steering);
        // The force arrives mid-turn: kill + head of queue.
        agent.send("FORCE", Lane::Force);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let users: Vec<&str> = entries
            .iter()
            .filter(|e| e.kind == KIND_USER)
            .map(|e| e.payload["text"].as_str().unwrap())
            .collect();
        // The forced message is delivered before the queued steering, on
        // the turn that follows the killed one.
        assert_eq!(users, vec!["FORCE", "queued steering", "go"]);
    }

    /// A force sent while the stream is IN FLIGHT (not a pre-cut): the kill
    /// flag stops the slow stream mid-way, the partial stands as interrupted,
    /// and the forced message preempts a steering queued at the same instant
    /// on the next call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn force_mid_stream_kills_the_stream_and_preempts_contemporaneous_steering() {
        let dir = tempfile::tempdir().unwrap();
        // One stream, four 40 ms-apart text deltas, ending in a single
        // completed event: left alone it runs ~200 ms to completion, the
        // force at ~100 ms cuts it mid-way. (Built by hand — sse() ends each
        // chunk in [DONE], and the decoder stops at the first one.)
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"c\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"d\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
            "data: [DONE]\n\n"
        );
        let provider = crate::provider::canned_slow(body, 40);
        let agent = Arc::new(make_agent(dir.path(), provider));
        agent.send("go", Lane::FollowUp);
        let killer = {
            let agent = agent.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                agent.send("FORCE", Lane::Force);
                agent.send("steer", Lane::Steering);
            })
        };
        agent.process().await.unwrap();
        killer.await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                KIND_USER,
                KIND_ASSISTANT,
                KIND_USER,
                KIND_USER,
                KIND_ASSISTANT
            ]
        );
        let killed = &entries[1];
        assert!(
            killed.payload["interrupted"].as_bool().unwrap(),
            "{killed:?}"
        );
        // a, b, c land before the 100 ms force (d is at 120 ms); under a
        // loaded runner the 80 ms c may lose the race, so accept ab or abc.
        let partial = killed.payload["text"].as_str().unwrap();
        assert!(
            partial == "ab" || partial == "abc",
            "partial was {partial:?}"
        );
        assert_eq!(entries[2].payload["text"], "FORCE");
        assert_eq!(entries[3].payload["text"], "steer");
        // The following call starts un-killed and runs the stream to completion.
        assert_eq!(entries[4].payload["text"], "abcd");
        assert!(!entries[4].payload["interrupted"].as_bool().unwrap());
    }

    /// Live acceptance (ticket #19): a four-tool session against the hosted
    /// vLLM endpoint; skipped unless TAU_TEST_ENDPOINT is set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_tool_calling_session() {
        let base = match std::env::var("TAU_TEST_ENDPOINT") {
            Ok(base) => base,
            Err(_) => return,
        };
        let model = std::env::var("TAU_TEST_MODEL").unwrap_or_else(|_| "qwen3.8-27b".into());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "line1\nline2\nline3\n").unwrap();
        let provider = crate::provider::production(
            &reqwest::Client::new(),
            &crate::config::Provider {
                base_url: base,
                key_env: String::new(),
                models: vec![model.clone()],
            },
            &crate::config::Requests::default(),
        );
        let agent = AgentSession::new(SessionParams {
            store: session_in(dir.path()),
            system_prompt:
                "You have the tools read, write, edit, and bash. Use them as                  instructed; edit takes the 3-char anchors from read output."
                    .into(),
            model,
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig {
                max_output_tokens: Some(200),
                reasoning: Some(crate::provider::ReasoningEffort::Low),
            },
        });
        agent.send(
            "Do exactly this: 1) read notes.txt, 2) edit the line containing              'line2' so it becomes 'LINE2', 3) bash: cat notes.txt, 4) write              the file out.txt with the single line 'done'. Then reply              'finished'.",
            Lane::FollowUp,
        );
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let called: Vec<&str> = entries
            .iter()
            .filter(|e| e.kind == KIND_TOOL)
            .map(|e| e.payload["name"].as_str().unwrap())
            .collect();
        for want in ["read", "edit", "bash", "write"] {
            assert!(called.contains(&want), "missing {want}: {called:?}");
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "line1\nLINE2\nline3\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "done"
        );
    }

    #[tokio::test]
    async fn append_propagates_storage_errors_not_a_new_root() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![sse("ok", &[])]));
        let agent = make_agent(dir.path(), provider);
        agent.send("go", Lane::FollowUp);
        agent.process().await.unwrap();
        // Lose the file under the session: the next append must fail with a
        // storage error, not silently start a new root branch.
        std::fs::remove_file(agent.inner.lock().unwrap().store.path()).unwrap();
        agent.send("again", Lane::FollowUp);
        assert!(agent.process().await.is_err());
    }

    #[tokio::test]
    async fn runaway_turn_stops_with_a_visible_note() {
        let dir = tempfile::tempdir().unwrap();
        // The same tool call, forever: the round cap is the only exit.
        let body = sse(
            "",
            &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
        );
        let provider = Arc::new(ScriptedProvider::new(vec![body]));
        let agent = make_agent(dir.path(), provider);
        agent.send("loop", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let note = entries
            .iter()
            .find(|e| e.kind == KIND_SYSTEM)
            .expect("a system note records why the turn stopped");
        assert!(
            note.payload["note"].as_str().unwrap().contains("32"),
            "{note:?}"
        );
        assert_eq!(
            entries.iter().filter(|e| e.kind == KIND_TOOL).count(),
            MAX_ROUNDS
        );
    }

    #[tokio::test]
    async fn kill_policy_completes_the_inflight_tool_batch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "x\n").unwrap();
        // The killed stream had a completed function_call before the cut:
        // with the Complete policy the tool runs and the turn continues to
        // a final call where the forced message lands alongside the result.
        let mut body = String::new();
        body.push_str(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"c1\",\"type\":\"function_call\",\"name\":\"read\",\"call_id\":\"c1\",\"arguments\":\"{\\\"path\\\":\\\"x.txt\\\"}\"}}\n\n",
        );
        body.push_str("data: {\"type\":\"response.output_text.delta\",\"delta\":\"cut\"}\n\n");
        let body = body;
        let provider = canned_cut(&body, 1);
        let agent = AgentSession::new(SessionParams {
            store: session_in(dir.path()),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
        });
        agent.send("go", Lane::FollowUp);
        agent.send("FORCE", Lane::Force);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        // killed assistant + tool result + (the next scripted call = the
        // canned cut again, since the provider is single-shot) + the forced
        // user entry is delivered at the head of that final call.
        assert!(kinds.contains(&KIND_TOOL), "{kinds:?}");
        let interrupted = entries
            .iter()
            .find(|e| e.kind == KIND_ASSISTANT && e.payload["interrupted"].as_bool() == Some(true))
            .unwrap();
        assert_eq!(interrupted.payload["calls"].as_array().unwrap().len(), 1);
    }
}
