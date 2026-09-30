//! The mock is correct iff the real client decodes it: every test below
//! drives `tau_core::provider` (the production SSE decode pipeline) at a
//! live mock server — no hand-rolled SSE assertions.

use std::sync::Arc;

use serde_json::json;
use tau_core::config::{Provider, Requests};
use tau_core::provider::{self, InputEntry, InputMessage, ResponseRequest, TurnEvent, TurnSink};
use tau_mock_llm::scenario::{Call, Scenario, ScenarioSet, Turn, Usage};
use tau_mock_llm::server;

fn provider_for(addr: &str) -> Provider {
    Provider {
        base_url: format!("http://{addr}/v1"),
        key_env: String::new(),
        models: std::collections::BTreeMap::new(),
    }
}

fn fast_requests() -> Requests {
    Requests {
        timeout_secs: 10,
        retries: 1,
        idle_timeout_secs: 10,
        tool_batch_on_force: tau_core::config::ToolBatchPolicy::default(),
    }
}

async fn start(set: ScenarioSet) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, Arc::new(set)));
    format!("{addr}")
}

fn user_request(text: &str) -> ResponseRequest {
    ResponseRequest::new(
        "mock-model",
        None,
        vec![InputEntry::Message(InputMessage {
            role: "user".into(),
            content: text.into(),
        })],
    )
}

#[tokio::test]
async fn a_text_turn_streams_and_decodes_through_the_real_client() {
    let mut set = ScenarioSet::default();
    set.scenarios.push(Scenario::new(
        "hello-mock",
        vec![Turn {
            text: Some("the mock answered the hello.".into()),
            calls: vec![],
        }],
        Usage {
            input_tokens: 11,
            output_tokens: 7,
            total_tokens: 18,
        },
    ));
    let addr = start(set).await;

    // A recording sink: the stream-delta path the GUI's coalescing rides.
    struct Record(Vec<TurnEvent>);
    impl TurnSink for Record {
        fn event(&mut self, event: TurnEvent) -> bool {
            self.0.push(event);
            true
        }
    }
    let mut sink = Record(Vec::new());
    let result = provider::stream_turn(
        &reqwest::Client::new(),
        &provider_for(&addr),
        &fast_requests(),
        &user_request("hello-mock"),
        &mut sink,
    )
    .await
    .expect("the real client decodes the mock's stream");
    assert_eq!(result.text, "the mock answered the hello.");
    assert!(result.completed, "the [DONE]-terminated stream is complete");
    assert_eq!(result.usage.unwrap().total_tokens, 18);
    // The text arrived as deltas (streaming fidelity), not one lump.
    let deltas = sink
        .0
        .iter()
        .filter(|e| matches!(e, TurnEvent::Text(_)))
        .count();
    assert!(deltas >= 1, "at least one text delta was streamed");
}

#[tokio::test]
async fn a_tool_call_turn_decodes_the_complete_function_call() {
    let mut set = ScenarioSet::default();
    set.scenarios.push(Scenario::new(
        "write-a-file",
        vec![Turn {
            text: Some("writing now.".into()),
            calls: vec![Call {
                name: "write".into(),
                arguments: json!({ "path": "a.txt", "content": "hi\n" }),
            }],
        }],
        Usage::default(),
    ));
    let addr = start(set).await;

    let result = provider::stream_response(
        &reqwest::Client::new(),
        &provider_for(&addr),
        &fast_requests(),
        &user_request("write-a-file"),
    )
    .await
    .expect("decoded");
    assert_eq!(result.text, "writing now.");
    assert_eq!(result.calls.len(), 1);
    let call = &result.calls[0];
    assert_eq!(call.name, "write");
    // Decoded `arguments` is the wire JSON string, whole (the core parses
    // it at tool execution).
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap(),
        json!({ "path": "a.txt", "content": "hi\n" })
    );
}

#[tokio::test]
async fn the_models_endpoint_round_trips() {
    let addr = start(ScenarioSet::default()).await;
    let models = provider::list_models(&reqwest::Client::new(), &provider_for(&addr))
        .await
        .expect("models round-trip");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "mock-model");
}

#[tokio::test]
async fn an_unscripted_request_gets_the_fallback_and_terminates() {
    let mut set = ScenarioSet::default();
    set.scenarios.push(Scenario::new(
        "never-matches-this",
        vec![Turn {
            text: Some("scripted".into()),
            calls: vec![],
        }],
        Usage::default(),
    ));
    let addr = start(set).await;
    let result = provider::stream_response(
        &reqwest::Client::new(),
        &provider_for(&addr),
        &fast_requests(),
        &user_request("something no scenario claims"),
    )
    .await
    .expect("the fallback stream decodes and terminates");
    assert!(result.text.starts_with("mock fallback:"));
    assert!(result.completed, "the fallback never hangs the stream");
}

#[tokio::test]
async fn sequential_requests_advance_the_script_then_clamp() {
    let mut set = ScenarioSet::default();
    set.scenarios.push(Scenario::new(
        "multi",
        vec![
            Turn {
                text: Some("turn one".into()),
                calls: vec![],
            },
            Turn {
                text: Some("turn two".into()),
                calls: vec![],
            },
        ],
        Usage::default(),
    ));
    let addr = start(set).await;
    let client = reqwest::Client::new();
    let p = provider_for(&addr);
    let r = fast_requests();
    assert_eq!(
        provider::stream_response(&client, &p, &r, &user_request("multi"))
            .await
            .unwrap()
            .text,
        "turn one"
    );
    assert_eq!(
        provider::stream_response(&client, &p, &r, &user_request("multi"))
            .await
            .unwrap()
            .text,
        "turn two"
    );
    // Past the end of the script: the final turn, not a hang.
    assert_eq!(
        provider::stream_response(&client, &p, &r, &user_request("multi"))
            .await
            .unwrap()
            .text,
        "turn two"
    );
}

#[tokio::test]
async fn an_unknown_path_answers_404() {
    let addr = start(ScenarioSet::default()).await;
    let res = reqwest::Client::new()
        .get(format!("http://{addr}/v1/nope"))
        .send()
        .await
        .expect("the request reaches the mock");
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn a_malformed_request_body_is_answered_not_fatal() {
    let set = ScenarioSet::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, Arc::new(set)));
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(b"not json at all".to_vec())
        .send()
        .await
        .expect("the request reaches the mock");
    // The selector falls back (empty parse) and the fallback turn streams:
    // a malformed body is answered, never a dropped connection.
    assert_eq!(res.status(), 200);
    let text = res.text().await.expect("body");
    assert!(text.contains("mock fallback:"), "got: {text}");
}
