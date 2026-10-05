//! Headless (episode-loop) protocol tests: drive the real `tau-acp` binary
//! over stdio against `tau-mock-llm` with multi-turn scenarios. The episode
//! loop (ticket #73) is invisible to the ACP protocol — one `session/prompt`
//! in, one response out — so these assert on what the client CAN see: the
//! per-episode agent chunks and the continuation prompts'
//! `user_message_chunk` surface.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

// Generous on purpose: under a full-suite nextest run the child's first
// answer can lag well past 30 s on a loaded machine.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

struct Client {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: BufReader<tokio::process::ChildStdout>,
}

impl Drop for Client {
    fn drop(&mut self) {
        // No orphaned agent processes on test failure: the normal path is
        // the stdin EOF, the kill covers the early-return ones.
        let _ = self.child.start_kill();
    }
}

impl Client {
    fn spawn(home: &Path, base_url: &str, model: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tau-acp"))
            .env("HOME", home)
            .env("TAU_BASE_URL", base_url)
            .env("TAU_MODEL", model)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawns the tau-acp binary");
        let stdin = child.stdin.take().expect("piped stdin");
        let lines = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            lines,
        }
    }

    async fn write(&mut self, msg: &Value) {
        let line = serde_json::to_string(msg).expect("test messages serialize");
        self.stdin
            .write_all(line.as_bytes())
            .await
            .expect("writes to the child's stdin");
        self.stdin
            .write_all(b"\n")
            .await
            .expect("writes to the child's stdin");
        self.stdin.flush().await.expect("flushes stdin");
    }

    /// The next ACP message of any kind (response or notification).
    async fn next(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(READ_TIMEOUT, self.lines.read_line(&mut line))
            .await
            .expect("the child answered in time")
            .expect("reads a line from the child's stdout");
        serde_json::from_str(line.trim()).expect("child stdout is ACP JSON")
    }

    /// Send a request and return its response, skipping notifications.
    async fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let msg = self.next().await;
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
            }
        }
    }

    /// Send a request, returning its response and the `session/update`
    /// notifications that arrived before it.
    async fn request_with_updates(
        &mut self,
        id: i64,
        method: &str,
        params: Value,
    ) -> (Value, Vec<Value>) {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        let mut updates = Vec::new();
        loop {
            let msg = self.next().await;
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return (msg, updates);
            }
            if msg.get("method").and_then(Value::as_str) == Some("session/update") {
                updates.push(msg);
            }
        }
    }
}

/// The `user_message_chunk` texts among the updates (the continuation
/// prompts' client-visible surface).
fn user_chunks(updates: &[Value]) -> Vec<String> {
    updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "user_message_chunk")
        .map(|u| {
            u["params"]["update"]["content"]["text"]
                .as_str()
                .expect("text chunk")
                .to_owned()
        })
        .collect()
}

/// The concatenated `agent_message_chunk` text among the updates.
fn agent_text(updates: &[Value]) -> String {
    updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .map(|u| {
            u["params"]["update"]["content"]["text"]
                .as_str()
                .expect("text chunk")
        })
        .collect()
}

fn mock_with_turns(pattern: &str, texts: &[&str]) -> Arc<tau_mock_llm::scenario::ScenarioSet> {
    let mut set = tau_mock_llm::scenario::ScenarioSet::default();
    set.scenarios.push(tau_mock_llm::scenario::Scenario::new(
        pattern,
        texts
            .iter()
            .map(|t| tau_mock_llm::scenario::Turn {
                text: Some(t.to_string()),
                calls: vec![],
            })
            .collect(),
        tau_mock_llm::scenario::Usage::default(),
    ));
    Arc::new(set)
}

const MARKER: &str = "All requirements verified.\n```json\n{\"task_complete\": true}\n```";

async fn spawn_client(
    scenarios: Arc<tau_mock_llm::scenario::ScenarioSet>,
) -> (tempfile::TempDir, Client) {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(tau_mock_llm::server::serve(listener, scenarios));
    let client = Client::spawn(home.path(), &format!("http://{addr}/v1"), "mock-model");
    (home, client)
}

/// An early stop (no marker) is answered with a task-anchored continuation;
/// the declaration is met with the confirmation round; the second
/// declaration ends the run. One `session/prompt` in, one response out.
#[tokio::test]
async fn early_stop_continues_until_completion() {
    let (home, mut client) = spawn_client(mock_with_turns(
        "fix-build",
        &[
            "Inspecting the repository.",
            MARKER,
            "Yes, everything is verified.",
        ],
    ))
    .await;
    let _ = &home;

    let new = client
        .request(1, "session/new", json!({ "cwd": home.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let (resp, updates) = client
        .request_with_updates(
            2,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "fix-build: make the tests pass" }],
            }),
        )
        .await;
    assert_eq!(resp["result"]["stopReason"], "end_turn");

    // Every episode's assistant text reached the client.
    let text = agent_text(&updates);
    for expected in [
        "Inspecting the repository.",
        "All requirements verified.",
        "Yes, everything is verified.",
    ] {
        assert!(text.contains(expected), "missing {expected} in {text:?}");
    }

    // The continuation prompts surfaced as user_message_chunk: the generic
    // nudge (episode 1 → 2) and the confirmation (episode 2 → 3).
    let nudges = user_chunks(&updates);
    assert!(
        nudges
            .iter()
            .any(|n| n.starts_with("The task is not complete. Task: fix-build")),
        "generic nudge missing: {nudges:?}"
    );
    assert!(
        nudges
            .iter()
            .any(|n| n.starts_with("Are you sure you want to mark the task as complete?")),
        "confirmation missing: {nudges:?}"
    );
}

/// A stop with no questions and no marker at all gets the question
/// auto-answer: the run continues, then completes.
#[tokio::test]
async fn question_stop_gets_the_auto_answer() {
    let (home, mut client) = spawn_client(mock_with_turns(
        "db-choice",
        &["Which database should I use?", MARKER],
    ))
    .await;

    let new = client
        .request(1, "session/new", json!({ "cwd": home.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let (resp, updates) = client
        .request_with_updates(
            2,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "db-choice: build the service" }],
            }),
        )
        .await;
    assert_eq!(resp["result"]["stopReason"], "end_turn");

    let nudges = user_chunks(&updates);
    assert!(
        nudges
            .iter()
            .any(|n| n.starts_with("There is no user to answer questions in this session.")),
        "auto-answer missing: {nudges:?}"
    );
}

/// Repeated empty stops are a degenerate loop: two continuations, then the
/// run settles as a failure instead of nudging forever.
#[tokio::test]
async fn empty_stops_settle_at_the_budget() {
    let (home, mut client) = spawn_client(mock_with_turns("empty-task", &[""])).await;

    let new = client
        .request(1, "session/new", json!({ "cwd": home.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let (resp, updates) = client
        .request_with_updates(
            2,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "empty-task: do the thing" }],
            }),
        )
        .await;
    assert_eq!(resp["result"]["stopReason"], "end_turn");

    let nudges = user_chunks(&updates);
    let empty_nudges = nudges
        .iter()
        .filter(|n| n.starts_with("Your previous response was empty"))
        .count();
    assert_eq!(empty_nudges, 2, "two empty nudges, then settle: {nudges:?}");
}

/// A provider stream cut mid-flight (no `[DONE]`) used to settle the prompt
/// as `cancelled` and end the run (ticket #78). A provider interruption is
/// not a user cancel: the episode loop answers it like an early stop, and
/// the prompt ends `end_turn`.
///
/// The in-test provider drops the first call's connection after a partial
/// text and completes every later call with the marker.
#[tokio::test]
async fn provider_cut_stream_continues_the_run() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the cut provider");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(cut_provider(listener));

    let home = tempfile::tempdir().expect("temp home");
    let mut client = Client::spawn(home.path(), &format!("http://{addr}/v1"), "mock-model");

    let new = client
        .request(1, "session/new", json!({ "cwd": home.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let (resp, updates) = client
        .request_with_updates(
            2,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "cut-task: finish the work" }],
            }),
        )
        .await;

    assert!(
        !user_chunks(&updates).is_empty(),
        "the cut turn must be answered with a continuation prompt"
    );
    assert_eq!(resp["result"]["stopReason"], "end_turn");
}

/// First request: a partial text, then the connection drops (no `[DONE]`,
/// no `response.completed`). Later requests: a full completion with the
/// marker, so the run finishes in the confirmation round.
async fn cut_provider(listener: tokio::net::TcpListener) {
    let mut calls = 0u32;
    loop {
        let (mut stream, _) = listener.accept().await.expect("accepts");
        calls += 1;
        let mut head = [0u8; 8192];
        let _ = stream.read(&mut head).await;
        let frames: Vec<String> = if calls == 1 {
            vec![
                json!({ "type": "response.created", "response": { "id": "cut-1" } }).to_string(),
                json!({ "type": "response.output_text.delta", "delta": "Working on it." })
                    .to_string(),
            ]
        } else {
            vec![
                json!({ "type": "response.created", "response": { "id": "ok" } }).to_string(),
                json!({ "type": "response.output_text.delta", "delta": MARKER }).to_string(),
                json!({
                    "type": "response.completed",
                    "response": { "id": "ok", "usage": {
                        "input_tokens": 120,
                        "output_tokens": 48,
                        "total_tokens": 168,
                    } },
                })
                .to_string(),
                "[DONE]".to_string(),
            ]
        };
        let mut body = String::new();
        for f in &frames {
            use std::fmt::Write as _;
            let _ = write!(body, "data: {f}\n\n");
        }
        let _ = stream
            .write_all(
                format!("HTTP/1.1 200 OK\ncontent-type: text/event-stream\n\n{body}")
                    .as_bytes(),
            )
            .await;
        drop(stream); // the cut: the connection ends without `[DONE]`
    }
}
