use super::*;

use crate::agent::testkit::*;

/// Context gate (ticket #210): when the assembled prompt exceeds the safe
/// budget and there is unobserved material, the gate fires the OM pass
/// before the call, so the re-assembled context fits. A second turn is
/// idempotent — no unobserved material means no gate.
#[tokio::test]
async fn context_gate_fires_when_over_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // 10 entries of 3200 chars ≈ 8000 tokens, over the 7904 budget the 16096
    // window arms (16096 − 4096 margin − 4096 smallest answer).
    let mut parent: Option<String> = None;
    for _ in 0..10 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "a".repeat(3200), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    // A small observation so the re-assembled prompt drops under budget.
    let obs_text = "<observations>did the work</observations>";
    // The gate's observe rung drains the window a bounded slice per pass
    // (#394): 12 entries of 800 tokens, and the 1000-token bound admits one per
    // pass, so the drain is 12 Observer calls. The ladder stops the moment the
    // prompt fits, so this is the rung's cost, not a fixed tariff.
    let mut script: Vec<String> = (0..12).map(|_| sse_json(obs_text)).collect();
    script.push(sse("a1", &[])); // main call, turn 1
    script.push(sse("a2", &[])); // main call, turn 2 (no gate)
    let provider = Arc::new(ScriptedProvider::new(script));
    let turn = TurnConfig {
        context_window: 16_096,
        max_output_tokens: Some(8000),
        ..Default::default()
    };
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn,
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();
    let seen = provider.seen.lock();
    // The gate's compaction is the drain: a settle observes a bounded slice of
    // the window, so the ladder runs one pass per slice (#394) and the count is
    // the rung's cost. The assertions read calls by role, not by position.
    let main_calls: Vec<usize> = (0..seen.len())
        .filter(|i| {
            seen[*i]
                .0
                .as_deref()
                .is_some_and(|s| s.starts_with("be terse"))
        })
        .collect();
    let observer_calls = seen.len() - main_calls.len();
    assert!(observer_calls >= 1, "the gate ran no observer: {seen:?}");
    // Turn 1's main call and turn 2's main call both use the session prompt
    // (with the observation log appended by assemble_context).
    assert_eq!(
        main_calls.len(),
        2,
        "one main call per turn, the drain is the observer's: {seen:?}"
    );
    for i in &main_calls {
        assert!(
            seen[*i]
                .0
                .as_deref()
                .is_some_and(|s| s.starts_with("be terse")),
            "main call {i}: {:?}",
            seen[*i].0
        );
    }
    // The drain shrank the window before the first main call. The ladder stops
    // the moment the prompt fits, so the tail rides on unobserved and later
    // settles observe it (#394) — the gate compacts the prompt, not the whole
    // window.
    let first_main = main_calls[0];
    let raw_in_first_main = seen[first_main]
        .1
        .as_deref()
        .map_or(0, |c| c.matches(&"a".repeat(3200)).count());
    assert!(
        raw_in_first_main > 0 && raw_in_first_main < 10,
        "the gate's drain left the {raw_in_first_main} raw entries as they were: {:?}",
        seen[first_main].1
    );
    // The window came back inside activation, so the stall stays unrecorded:
    // the record is for what compaction could not fix (ADR-0034).
    assert!(
        stalled_in(dir.path()).is_empty(),
        "a repaired window records no stall"
    );
}

/// Context gate (ticket #210): a prompt under the safe budget does not
/// fire the gate — no observer call, the main call goes straight through.
#[tokio::test]
async fn context_gate_noop_when_under_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // One small entry: well under the 7904 budget.
    store
        .append(
            EntryKind::User,
            serde_json::json!({ "text": "hello", "lane": "follow-up" }),
            None,
        )
        .unwrap();
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let provider = Arc::new(ScriptedProvider::new(vec![sse("a1", &[])]));
    let turn = TurnConfig {
        context_window: 16_096,
        max_output_tokens: Some(8000),
        ..Default::default()
    };
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn,
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    let seen = provider.seen.lock();
    // No observer call: the gate did not fire.
    assert_eq!(seen.len(), 1, "no gate: {seen:?}");
    assert_eq!(seen[0].0.as_deref(), Some("be terse"));
}

/// The ladder ran out of rungs: the observer's output is so large that the
/// re-priced prompt stays over budget. #338 closes the warn-and-send arm — the
/// gate refuses pre-wire and the refusal is durable, so the doomed request
/// never reaches the endpoint (the #338 incident's failure mode).
#[tokio::test]
async fn context_gate_refuses_when_the_ladder_cannot_fit() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // 10 entries of 3200 chars ≈ 8000 tokens, over the 7904 budget.
    let mut parent: Option<String> = None;
    for _ in 0..10 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "a".repeat(3200), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        // High enough that the huge observation doesn't trigger a reflect
        // call — the test is about the gate, not reflect.
        reflect_threshold: 100_000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    // A huge observation: 40000 chars ≈ 10000 tokens, still over budget.
    let huge_obs = format!("<observations>{}</observations>", "x".repeat(40000));
    let small_obs = "<observations>ok</observations>";
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse_json(&huge_obs), // observer call (the ladder's observe rung)
        sse_json(small_obs), // reflector calls follow, then the scripted list runs dry
    ]));
    let turn = TurnConfig {
        context_window: 16_096,
        max_output_tokens: Some(8000),
        ..Default::default()
    };
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn,
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    let err = agent
        .process()
        .await
        .expect_err("a prompt the ladder cannot fit is refused");
    assert!(
        matches!(&err, AgentError::ContextOverflow { prompt, budget } if *prompt > *budget),
        "expected ContextOverflow, got {err:?}"
    );

    let seen = provider.seen.lock();
    // Compaction ran and is visible in the call list; the main prompt never
    // went on the wire — the warn-and-send arm this ticket closes.
    assert!(
        !seen.is_empty(),
        "the ladder's compaction calls happen: {seen:?}"
    );
    assert!(
        seen.iter()
            .all(|c| c.0.as_deref().is_none_or(|s| !s.starts_with("be terse"))),
        "no main call: the refusal is pre-wire: {seen:?}"
    );
    // The refusal is durable: a context_overflow record names prompt > budget.
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let entries = entries_of(&reopened);
    assert!(
        entries.iter().any(|e| {
            e.kind == EntryKind::Diagnostic
                && e.payload["prompt"].as_u64().unwrap_or_default()
                    > e.payload["budget"].as_u64().unwrap_or_default()
        }),
        "a diagnostic record names the overflow: {entries:?}"
    );
}
