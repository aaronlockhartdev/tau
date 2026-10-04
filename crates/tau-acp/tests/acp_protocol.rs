//! Protocol-level tests: drive the real `tau-acp` binary over stdio the
//! way an ACP client (Harbor, an IDE) would. The positive prompt flow runs
//! against `tau-mock-llm` (deterministic); the in-flight rejection runs
//! against a listener that accepts and never answers, so the first turn is
//! in flight for as long as the test needs it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
            .write_all(format!("{line}\n").as_bytes())
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

/// A listener that accepts every connection and holds it open without
/// answering: a provider call against it is in flight until the test ends.
async fn hanging_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the hanging endpoint");
    let addr = listener
        .local_addr()
        .expect("the bound listener has a local addr");
    tokio::spawn(async move {
        // Every accepted socket stays open for the task's lifetime: that
        // is what keeps the provider call in flight.
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
        let _ = held;
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn initialize_and_session_new_advertise_the_contract() {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(tau_mock_llm::server::serve(
        listener,
        Arc::new(tau_mock_llm::scenario::ScenarioSet::default()),
    ));

    let mut client = Client::spawn(home.path(), &format!("http://{addr}/v1"), "mock-model");
    let cwd = tempfile::tempdir().expect("temp cwd");

    let init = client
        .request(
            1,
            "initialize",
            json!({ "protocolVersion": 1, "clientCapabilities": {} }),
        )
        .await;
    assert_eq!(init["result"]["protocolVersion"], 1);
    assert_eq!(init["result"]["authMethods"][0]["id"], "openai-compatible");
    assert_eq!(init["result"]["agentInfo"]["name"], "tau");

    let new = client
        .request(2, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");
    assert!(!session_id.is_empty());
    let options = &new["result"]["configOptions"][0];
    assert_eq!(options["id"], "model");
    assert_eq!(options["category"], "model");
    assert_eq!(options["currentValue"], "mock-model");
    assert!(
        options["options"]
            .as_array()
            .expect("model options")
            .iter()
            .any(|o| o["value"] == "mock-model"),
        "the env provider's model is offered"
    );
}

#[tokio::test]
async fn prompt_streams_chunks_and_ends_the_turn() {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(tau_mock_llm::server::serve(
        listener,
        Arc::new(tau_mock_llm::scenario::ScenarioSet::default()),
    ));

    let mut client = Client::spawn(home.path(), &format!("http://{addr}/v1"), "mock-model");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let (resp, updates) = client
        .request_with_updates(
            2,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "hello" }],
            }),
        )
        .await;
    let chunks: Vec<&Value> = updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .collect();
    assert!(
        !chunks.is_empty(),
        "the turn streamed at least one agent_message_chunk, got: {updates:?}"
    );
    let text: String = chunks
        .iter()
        .map(|u| {
            u["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        })
        .collect();
    assert!(!text.is_empty(), "the chunks carry the assistant text");
    assert_eq!(resp["result"]["stopReason"], "end_turn");
}

#[tokio::test]
async fn second_concurrent_prompt_is_rejected() {
    let home = tempfile::tempdir().expect("temp home");
    let endpoint = hanging_endpoint().await;
    let mut client = Client::spawn(home.path(), &endpoint, "mock-model");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    // Prompt 1 hangs in the provider call (the endpoint never answers).
    client
        .write(&json!({
            "jsonrpc": "2.0", "id": 10, "method": "session/prompt",
            "params": { "sessionId": session_id, "prompt": [{ "type": "text", "text": "a" }] },
        }))
        .await;
    // Prompt 2 must be refused while the first turn is in flight.
    let second = client
        .request(
            11,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "b" }],
            }),
        )
        .await;
    assert_eq!(second["error"]["code"], -32600);
    assert!(
        second["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("in flight"),
        "the rejection says a turn is in flight: {second}"
    );
}

#[tokio::test]
async fn clear_errors_for_unknowns_and_bad_frames() {
    let home = tempfile::tempdir().expect("temp home");
    let endpoint = hanging_endpoint().await;
    let mut client = Client::spawn(home.path(), &endpoint, "mock-model");

    // Unknown method.
    let resp = client.request(1, "session/load", json!({})).await;
    assert_eq!(resp["error"]["code"], -32601);

    // A malformed frame: parse error with a null id.
    client
        .stdin
        .write_all(b"this is not json\n")
        .await
        .expect("writes to the child's stdin");
    let parse = client.next().await;
    assert!(
        parse["id"].is_null(),
        "a parse error carries a null id: {parse}"
    );
    assert_eq!(parse["error"]["code"], -32700);

    // Prompting a session the agent never created.
    let resp = client
        .request(
            2,
            "session/prompt",
            json!({
                "sessionId": "no-such-session",
                "prompt": [{ "type": "text", "text": "a" }],
            }),
        )
        .await;
    assert_eq!(resp["error"]["code"], -32602);
    assert!(
        resp["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("unknown session")
    );

    // A non-model config option.
    let cwd = tempfile::tempdir().expect("temp cwd");
    let new = client
        .request(3, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");
    let resp = client
        .request(
            4,
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": "temperature", "value": 0.5 }),
        )
        .await;
    assert_eq!(resp["error"]["code"], -32602);
}
