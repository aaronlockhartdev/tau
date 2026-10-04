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

    /// Read until the response for a previously written request arrives.
    async fn response(&mut self, id: i64) -> Value {
        loop {
            let msg = self.next().await;
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
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
    // No config layer declares a context window for the env provider's
    // model, so no `usage_update` is owed (N5: `size` must not be guessed).
    assert!(
        updates
            .iter()
            .all(|u| u["params"]["update"]["sessionUpdate"] != "usage_update"),
        "no usage_update without a declared window: {updates:?}"
    );
    assert_eq!(resp["result"]["stopReason"], "end_turn");
}

/// A declared context window reaches the client as a per-call
/// `usage_update` (N5): the system config declares one for the session's
/// model, so the turn's `StreamEnd` usage rides alongside the finalize.
#[tokio::test]
async fn prompt_usage_update_when_the_window_is_known() {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(tau_mock_llm::server::serve(
        listener,
        Arc::new(tau_mock_llm::scenario::ScenarioSet::default()),
    ));
    let base_url = format!("http://{addr}/v1");

    // The system config declares the session's model with a window; the
    // provider name sorts first, so its model is the effective default.
    let config_dir = home.path().join(".config").join("tau");
    std::fs::create_dir_all(&config_dir).expect("makes the config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "[providers.aa-mock]\nbase_url = \"{base_url}\"\nkey_env = \"\"\n\n[providers.aa-mock.models.windowed-model]\ncontext_window = 4096\n"
        ),
    )
    .expect("writes the system config");

    let mut client = Client::spawn(home.path(), &base_url, "mock-model");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");
    assert_eq!(
        new["result"]["configOptions"][0]["currentValue"], "windowed-model",
        "the windowed model is the effective default"
    );

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

    let usage: Vec<&Value> = updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "usage_update")
        .collect();
    assert_eq!(
        usage.len(),
        1,
        "one usage_update per provider call: {updates:?}"
    );
    // The mock fallback's usage: 120 input + 48 output.
    assert_eq!(usage[0]["params"]["update"]["used"], 168);
    assert_eq!(usage[0]["params"]["update"]["size"], 4096);
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

/// The project layer (the workspace's `.tau/config.toml`) reaches the model
/// selector: its providers' models are offered in `session/new` alongside
/// the env-override provider's (M4).
#[tokio::test]
async fn project_layer_models_appear_in_the_selector() {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(tau_mock_llm::server::serve(
        listener,
        Arc::new(tau_mock_llm::scenario::ScenarioSet::default()),
    ));
    let base_url = format!("http://{addr}/v1");
    let cwd = tempfile::tempdir().expect("temp cwd");
    std::fs::create_dir_all(cwd.path().join(".tau")).expect("makes .tau");
    std::fs::write(
        cwd.path().join(".tau").join("config.toml"),
        format!(
            "[providers.proj]\nbase_url = \"{base_url}\"\nkey_env = \"\"\n\n[providers.proj.models.\"proj-a\"]\n[providers.proj.models.\"proj-b\"]\n"
        ),
    )
    .expect("writes the project config");

    let mut client = Client::spawn(home.path(), &base_url, "mock-model");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let values: Vec<&str> = new["result"]["configOptions"][0]["options"]
        .as_array()
        .expect("model options")
        .iter()
        .map(|o| o["value"].as_str().expect("a model id"))
        .collect();
    assert!(
        values.contains(&"proj-a"),
        "project-layer model offered: {values:?}"
    );
    assert!(
        values.contains(&"proj-b"),
        "project-layer model offered: {values:?}"
    );
    assert!(
        values.contains(&"mock-model"),
        "the env-override model is still offered: {values:?}"
    );
}

/// `session/set_config_option` answers with the updated model select: the v1
/// schema marks `configOptions` required on this response, and
/// `currentValue` sits on the model just set (B1; Harbor's `--model` path).
#[tokio::test]
async fn set_config_option_responds_with_the_updated_select() {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    tokio::spawn(tau_mock_llm::server::serve(
        listener,
        Arc::new(tau_mock_llm::scenario::ScenarioSet::default()),
    ));
    let base_url = format!("http://{addr}/v1");
    let cwd = tempfile::tempdir().expect("temp cwd");
    std::fs::create_dir_all(cwd.path().join(".tau")).expect("makes .tau");
    std::fs::write(
        cwd.path().join(".tau").join("config.toml"),
        format!(
            "[providers.proj]\nbase_url = \"{base_url}\"\nkey_env = \"\"\n\n[providers.proj.models.\"proj-a\"]\n[providers.proj.models.\"proj-b\"]\n"
        ),
    )
    .expect("writes the project config");

    let mut client = Client::spawn(home.path(), &base_url, "mock-model");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let resp = client
        .request(
            2,
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": "model", "value": "proj-b" }),
        )
        .await;
    let option = &resp["result"]["configOptions"][0];
    assert_eq!(option["id"], "model");
    assert_eq!(option["currentValue"], "proj-b");
    let values: Vec<&str> = option["options"]
        .as_array()
        .expect("the full options list")
        .iter()
        .map(|o| o["value"].as_str().expect("a model id"))
        .collect();
    assert!(
        values.contains(&"proj-b"),
        "the new model is offered: {values:?}"
    );
}

/// `session/cancel` mid-turn: the in-flight `session/prompt` is answered
/// `stopReason: "cancelled"` — a response, never a JSON-RPC error (the spec
/// is explicit; M5a).
#[tokio::test]
async fn cancel_mid_turn_answers_cancelled() {
    let home = tempfile::tempdir().expect("temp home");
    let endpoint = hanging_endpoint().await;
    let mut client = Client::spawn(home.path(), &endpoint, "mock-model");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    // The turn hangs in the provider call; the cancel lands while it is in
    // flight (200 ms is well past the dispatch on localhost).
    client
        .write(&json!({
            "jsonrpc": "2.0", "id": 10, "method": "session/prompt",
            "params": { "sessionId": session_id, "prompt": [{ "type": "text", "text": "a" }] },
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    client
        .write(&json!({
            "jsonrpc": "2.0", "method": "session/cancel",
            "params": { "sessionId": session_id },
        }))
        .await;
    let resp = client.response(10).await;
    assert_eq!(
        resp["result"]["stopReason"], "cancelled",
        "the cancel answers the prompt, not an error: {resp}"
    );
}

/// A non-text prompt block is an `Invalid params` error: image/audio/
/// embeddedContext are not advertised, so rejection is spec-compliant (M5b).
#[tokio::test]
async fn non_text_prompt_blocks_are_rejected() {
    let home = tempfile::tempdir().expect("temp home");
    let endpoint = hanging_endpoint().await;
    let mut client = Client::spawn(home.path(), &endpoint, "mock-model");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let new = client
        .request(1, "session/new", json!({ "cwd": cwd.path() }))
        .await;
    let session_id = new["result"]["sessionId"].as_str().expect("sessionId");

    let resp = client
        .request(
            2,
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "image", "data": "aGk=", "mimeType": "image/png" }],
            }),
        )
        .await;
    assert_eq!(resp["error"]["code"], -32602);
    assert!(
        resp["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("unsupported prompt block"),
        "{resp}"
    );
}

/// A full turn with a tool call, over the wire: the `tool_call` create and
/// the `tool_call_update` result both arrive as `session/update`
/// notifications before the prompt response (M5d).
#[tokio::test]
async fn prompt_tool_calls_stream_as_updates() {
    let home = tempfile::tempdir().expect("temp home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the mock LLM");
    let addr = listener.local_addr().expect("bound addr");
    // One scripted scenario: a bash call, then the closing text. The second
    // provider request re-matches the scenario (the user message persists in
    // the input) and advances to the closing turn.
    let mut set = tau_mock_llm::scenario::ScenarioSet::default();
    set.scenarios.push(tau_mock_llm::scenario::Scenario::new(
        "tool-task",
        vec![
            tau_mock_llm::scenario::Turn {
                text: None,
                calls: vec![tau_mock_llm::scenario::Call {
                    name: "bash".into(),
                    arguments: json!({ "command": "echo tool-output" }),
                }],
            },
            tau_mock_llm::scenario::Turn {
                text: Some("the tool ran".into()),
                calls: vec![],
            },
        ],
        tau_mock_llm::scenario::Usage::default(),
    ));
    tokio::spawn(tau_mock_llm::server::serve(listener, Arc::new(set)));

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
                "prompt": [{ "type": "text", "text": "please run the tool-task" }],
            }),
        )
        .await;

    let creates: Vec<&Value> = updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "tool_call")
        .collect();
    assert_eq!(creates.len(), 1, "one tool_call create, got: {updates:?}");
    assert_eq!(creates[0]["params"]["update"]["name"], "bash");
    assert_eq!(creates[0]["params"]["update"]["status"], "in_progress");
    assert_eq!(
        creates[0]["params"]["update"]["rawInput"]["command"],
        "echo tool-output"
    );

    let updates_for_call: Vec<&Value> = updates
        .iter()
        .filter(|u| {
            u["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && u["params"]["update"]["toolCallId"]
                    == creates[0]["params"]["update"]["toolCallId"]
        })
        .collect();
    assert_eq!(
        updates_for_call.len(),
        1,
        "one tool_call_update for the call, got: {updates:?}"
    );
    assert_eq!(
        updates_for_call[0]["params"]["update"]["status"],
        "completed"
    );
    assert!(
        updates_for_call[0]["params"]["update"]["rawOutput"]
            .to_string()
            .contains("tool-output"),
        "the tool's output reaches the client: {}",
        updates_for_call[0]["rawOutput"]
    );

    assert_eq!(resp["result"]["stopReason"], "end_turn");
}
