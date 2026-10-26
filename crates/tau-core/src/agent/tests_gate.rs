//! The completion gate on the turn-end path (spec §5.3): the stop-hook, the
//! repair loop, the `gate_failed` live state, and the `QUESTION:` skip.

use super::*;

use crate::agent::testkit::*;
use crate::config::Gates;
use crate::harness::SessionRole;
use std::sync::Arc;

/// A bare session with a completion gate configured (the acceptance driver's
/// shape). The gate's check runs in the session's cwd.
fn make_agent_with_gate(
    dir: &std::path::Path,
    provider: Arc<ScriptedProvider>,
    gates: Gates,
) -> Arc<AgentSession> {
    AgentSession::launch(
        session_in(dir),
        SessionRole::Bare {
            provider,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.to_path_buf(),
            turn: TurnConfig::default(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            om: None,
            om_model: String::new(),
            gates: Some(gates),
        },
    )
    .unwrap()
}

fn write_call(id: &str, path: &str) -> (String, String, String) {
    (
        "write".into(),
        id.into(),
        format!(r#"{{"path":"{path}","content":"x"}}"#),
    )
}

/// The completion-gate records a run wrote, decoded back to typed facts
/// (ADR-0034).
fn gate_records(agent: &AgentSession) -> Vec<Diagnostic> {
    entries_of(&agent.inner.lock().unwrap().store)
        .into_iter()
        .filter(|e| e.kind == EntryKind::Diagnostic)
        .filter_map(|e| Diagnostic::from_value(&e.payload))
        .filter(|f| {
            matches!(
                f,
                Diagnostic::GateAttempt { .. } | Diagnostic::GateFailed { .. }
            )
        })
        .collect()
}

#[tokio::test]
async fn a_passing_gate_stands_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("", &[write_call("c1", "a.txt")]),
        sse("done", &[]),
    ]));
    let gates = Gates {
        command: "true".into(),
        ..Gates::default()
    };
    let agent = make_agent_with_gate(dir.path(), provider.clone(), gates);
    agent.send("work", Lane::FollowUp);
    agent.process().await.unwrap();

    // Two LLM calls (the tool round + the final), no repair bounce.
    assert_eq!(provider.seen.lock().unwrap().len(), 2);
    assert!(!agent.gate_failed());
    assert!(gate_records(&agent).is_empty(), "a pass records nothing");
    // The check is stated in the system prompt (the design: the model knows
    // what "done" is verified against).
    let seen = provider.seen.lock().unwrap();
    assert!(seen[0].0.as_deref().unwrap().contains("Completion gate"));
}

#[tokio::test]
async fn a_failing_gate_bounces_a_repair_that_passes() {
    let dir = tempfile::tempdir().unwrap();
    // The gate checks for `fixed.txt`; the model's first turn writes only
    // `a.txt` (the check fails), the repair writes `fixed.txt` (it passes).
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("", &[write_call("c1", "a.txt")]),
        sse("done", &[]),
        sse("", &[write_call("c2", "fixed.txt")]),
        sse("done", &[]),
    ]));
    let gates = Gates {
        command: "test -f fixed.txt".into(),
        ..Gates::default()
    };
    let agent = make_agent_with_gate(dir.path(), provider.clone(), gates);
    agent.send("work", Lane::FollowUp);
    agent.process().await.unwrap();

    // Four LLM calls: tool, final (fail), repair tool, final (pass).
    assert_eq!(provider.seen.lock().unwrap().len(), 4);
    assert!(!agent.gate_failed(), "a pass on repair clears the gate");
    // One failure record (the first check); the pass records nothing.
    let records = gate_records(&agent);
    assert_eq!(
        records.len(),
        1,
        "one failed check is recorded: {records:?}"
    );
    assert!(
        matches!(&records[0], Diagnostic::GateAttempt { command, retries: 1, .. }
            if command == "test -f fixed.txt"),
        "the record names the check that failed: {:?}",
        records[0]
    );
}

#[tokio::test]
async fn an_always_failing_gate_exhausts_retries_and_marks_gate_failed() {
    let dir = tempfile::tempdir().unwrap();
    // max_retries = 1: the initial check + one repair, then gate_failed.
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("", &[write_call("c1", "a.txt")]),
        sse("done", &[]),
        sse("", &[write_call("c2", "a.txt")]),
        sse("done", &[]),
    ]));
    let gates = Gates {
        command: "false".into(),
        max_retries: 1,
        ..Gates::default()
    };
    let agent = make_agent_with_gate(dir.path(), provider.clone(), gates);
    agent.send("work", Lane::FollowUp);
    agent.process().await.unwrap();

    // Four LLM calls: the initial + one repair, then the turn ends.
    assert_eq!(provider.seen.lock().unwrap().len(), 4);
    assert!(agent.gate_failed(), "retries exhausted → gate_failed");
    // Two records: the failed check, then the exhaustion.
    let records = gate_records(&agent);
    assert_eq!(
        records.len(),
        2,
        "the check and the exhaustion: {records:?}"
    );
    assert!(
        matches!(&records[1], Diagnostic::GateFailed { command, retries: 1 }
            if command == "false"),
        "the exhaustion record names the check and the repairs spent: {:?}",
        records[1]
    );
}

#[tokio::test]
async fn a_question_ending_skips_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    // A check that would fail if run — but the `QUESTION:` marker skips it.
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("", &[write_call("c1", "a.txt")]),
        sse("QUESTION: should I continue?", &[]),
    ]));
    let gates = Gates {
        command: "false".into(),
        ..Gates::default()
    };
    let agent = make_agent_with_gate(dir.path(), provider.clone(), gates);
    agent.send("work", Lane::FollowUp);
    agent.process().await.unwrap();

    // Two LLM calls: the gate is skipped, so no repair bounce.
    assert_eq!(provider.seen.lock().unwrap().len(), 2);
    assert!(!agent.gate_failed());
    assert!(gate_records(&agent).is_empty(), "a skip records nothing");
}

#[tokio::test]
async fn a_no_tool_turn_is_not_gated() {
    let dir = tempfile::tempdir().unwrap();
    // No tool call this turn: not a qualifying turn-end, the gate is not run.
    let provider = Arc::new(ScriptedProvider::new(vec![sse("all done, no tools", &[])]));
    let gates = Gates {
        command: "false".into(),
        ..Gates::default()
    };
    let agent = make_agent_with_gate(dir.path(), provider.clone(), gates);
    agent.send("work", Lane::FollowUp);
    agent.process().await.unwrap();

    // One LLM call: the gate is not run, so no repair.
    assert_eq!(provider.seen.lock().unwrap().len(), 1);
    assert!(!agent.gate_failed());
    assert!(
        gate_records(&agent).is_empty(),
        "an ungated turn records nothing"
    );
}

#[tokio::test]
async fn a_later_passing_turn_clears_the_prior_gate_failed() {
    let dir = tempfile::tempdir().unwrap();
    // Turn 1: the check fails through its one repair (max_retries = 1) →
    // gate_failed. Turn 2: the check passes → the live state clears across
    // the turn boundary.
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("", &[write_call("c1", "a.txt")]),
        sse("done", &[]),
        sse("", &[write_call("c2", "a.txt")]),
        sse("done", &[]),
        sse("", &[write_call("c3", "fixed.txt")]),
        sse("done", &[]),
    ]));
    let gates = Gates {
        command: "test -f fixed.txt".into(),
        max_retries: 1,
        ..Gates::default()
    };
    let agent = make_agent_with_gate(dir.path(), provider.clone(), gates);
    agent.send("work", Lane::FollowUp);
    agent.process().await.unwrap();
    assert!(agent.gate_failed(), "turn 1 exhausted the retries");

    agent.send("work again", Lane::FollowUp);
    agent.process().await.unwrap();
    // Six LLM calls: four in turn 1 (initial + repair + two finals), two
    // in turn 2.
    assert_eq!(provider.seen.lock().unwrap().len(), 6);
    assert!(
        !agent.gate_failed(),
        "turn 2's pass clears the prior gate_failed"
    );
}
