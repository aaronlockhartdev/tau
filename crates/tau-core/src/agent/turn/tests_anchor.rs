//! What the context gate acts on once a provider has priced the conversation:
//! the count it reported, not a fresh guess over the branch (§6.9).

use super::*;

use crate::agent::testkit::{ScriptedProvider, session_in};
use crate::harness::SessionRole;
use serde_json::json;
use std::sync::Arc;

fn read_call_frame(call_id: &str, input_tokens: Option<u64>) -> String {
    let item = json!({
        "id": call_id,
        "type": "function_call",
        "name": "read",
        "call_id": call_id,
        "arguments": r#"{"path":"n.txt"}"#,
    });
    let frame = json!({
        "type": "response.output_item.done",
        "item": item,
    });
    format!("data: {frame}\n\n{}\n\n", completion_frame(input_tokens))
}

fn final_frame(text: &str) -> String {
    let delta = json!({ "type": "response.output_text.delta", "delta": text });
    format!("data: {delta}\n\n{}\n\n", completion_frame(None))
}

fn completion_frame(input_tokens: Option<u64>) -> String {
    let response = match input_tokens {
        Some(input_tokens) => json!({
            "usage": { "input_tokens": input_tokens, "output_tokens": 1 }
        }),
        None => json!({}),
    };
    let done = json!({ "type": "response.completed", "response": response });
    format!("data: {done}\n\ndata: [DONE]\n\n")
}

/// A session whose model declares a 12,000-token window and a 500-token output
/// cap: budget = 12,000 − 500 − 4,096 = 7,404.
fn gated_agent(dir: &std::path::Path, provider: Arc<ScriptedProvider>) -> Arc<AgentSession> {
    AgentSession::launch(
        session_in(dir),
        SessionRole::Bare {
            provider,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: crate::tools::surface::bare_specs(),
            cwd: dir.to_path_buf(),
            turn: TurnConfig {
                context_window: Some(12_000),
                max_output_tokens: Some(500),
                ..Default::default()
            },
            tool_batch_on_force: ToolBatchPolicy::Complete,
            om: None,
            om_model: String::new(),
            gates: None,
        },
    )
    .unwrap()
}

fn calls(provider: &ScriptedProvider) -> usize {
    provider.models.lock().unwrap().len()
}

/// The provider charged 9,000 tokens for the first request, so the second call
/// is over budget and the turn refuses before the wire. The whole-prompt
/// estimate over this script — a system line, one user message, one tool call
/// and its output — is a few dozen tokens and would have passed.
#[tokio::test]
async fn the_gate_refuses_on_the_count_the_provider_charged() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        read_call_frame("c1", Some(9_000)),
        final_frame("done"),
    ]));
    let agent = gated_agent(dir.path(), provider.clone());
    agent.send("read n.txt", Lane::Steering);
    let err = agent.process().await.unwrap_err();
    match err {
        AgentError::ContextOverflow { prompt, budget } => {
            // The charged count plus what was appended since — the tool call's
            // arguments and the read output, priced with the framing allowance.
            assert!(
                (9_000..9_200).contains(&prompt),
                "prompt {prompt} is not the charged 9,000 plus a small tail"
            );
            assert_eq!(budget, 7_404);
        }
        other => panic!("expected the context gate, saw {other:?}"),
    }
    assert_eq!(calls(&provider), 1, "the refused call never went out");
}

/// The same turn with a charge inside the budget runs to completion, so the
/// anchor is not a refusal machine.
#[tokio::test]
async fn a_charge_inside_the_budget_keeps_the_turn_running() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        read_call_frame("c1", Some(1_000)),
        final_frame("done"),
    ]));
    let agent = gated_agent(dir.path(), provider.clone());
    agent.send("read n.txt", Lane::Steering);
    agent.process().await.unwrap();
    assert_eq!(calls(&provider), 2);
}

/// A provider that reports no usage anchors nothing: the estimate stays
/// unanchored and the turn runs on the whole-prompt estimate (§6.9).
#[tokio::test]
async fn a_call_that_reports_no_usage_anchors_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        read_call_frame("c1", None),
        final_frame("done"),
    ]));
    let agent = gated_agent(dir.path(), provider.clone());
    agent.send("read n.txt", Lane::Steering);
    agent.process().await.unwrap();
    assert_eq!(calls(&provider), 2);
}
