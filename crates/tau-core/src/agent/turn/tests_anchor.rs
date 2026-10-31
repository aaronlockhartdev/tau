//! What the context gate acts on once a provider has priced the conversation:
//! the count it reported, not a fresh guess over the branch (§6.9).

use super::*;

use crate::agent::testkit::{ScriptedProvider, session_in, sse_charged};
use crate::harness::SessionRole;
use std::sync::Arc;

/// A read call the provider charged `input_tokens` for, then a final answer.
fn read_then_answer(input_tokens: Option<u64>) -> Vec<String> {
    let call = [(
        "read".to_owned(),
        "c1".to_owned(),
        r#"{"path":"n.txt"}"#.to_owned(),
    )];
    vec![
        sse_charged("", &call, input_tokens),
        sse_charged("done", &[], None),
    ]
}

/// A session whose model declares a 12,000-token window and a 500-token output
/// cap. The window is what sets the prompt's ceiling; the cap rides along as the
/// request's hard limit.
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
                // budget = 12000 − 4096 margin − 4096 smallest answer = 3808.
                context_window: 12_000,
                max_output_tokens: Some(500),
                ..Default::default()
            },
            tool_batch_on_force: ToolBatchPolicy::Complete,
            om: None,
            om_route: crate::agent::OmRoute::Session,
            gates: None,
        },
    )
    .unwrap()
}

fn calls(provider: &ScriptedProvider) -> usize {
    provider.models.lock().len()
}

/// The provider charged 9,000 tokens for the first request, so the second call
/// is over budget and the turn refuses before the wire. The whole-prompt
/// estimate over this script — a system line, one user message, one tool call
/// and its output — is a few dozen tokens and would have passed.
#[tokio::test]
async fn the_gate_refuses_on_the_count_the_provider_charged() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let provider = Arc::new(ScriptedProvider::new(read_then_answer(Some(9_000))));
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
            assert_eq!(budget, 3_808);
        }
        other => panic!("expected the context gate, saw {other:?}"),
    }
    assert_eq!(calls(&provider), 1, "the refused call never went out");
}

/// The largest charge a provider can report is not a number the gate shrugs at:
/// the refusal names the ceiling, because a fresh estimate would read as room
/// where the provider just said there is none.
#[tokio::test]
async fn a_charge_past_the_ceiling_refuses_at_the_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let provider = Arc::new(ScriptedProvider::new(read_then_answer(Some(
        u64::from(u32::MAX) + 1_000,
    ))));
    let agent = gated_agent(dir.path(), provider.clone());
    agent.send("read n.txt", Lane::Steering);
    let err = agent.process().await.unwrap_err();
    match err {
        AgentError::ContextOverflow { prompt, budget } => {
            assert_eq!(prompt, u32::MAX);
            assert_eq!(budget, 3_808);
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
    let provider = Arc::new(ScriptedProvider::new(read_then_answer(Some(1_000))));
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
    let provider = Arc::new(ScriptedProvider::new(read_then_answer(None)));
    let agent = gated_agent(dir.path(), provider.clone());
    agent.send("read n.txt", Lane::Steering);
    agent.process().await.unwrap();
    assert_eq!(calls(&provider), 2);
}
