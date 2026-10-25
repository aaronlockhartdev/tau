use super::*;

use crate::agent::testkit::*;

/// End-to-end OM (the ticket's acceptance bar): synthesized raw entries
/// cross the observe threshold, the canned Observer fills the log past
/// the reflect threshold, the canned Reflector rewrites the suffix —
/// and the continuation hint is injected exactly once across the run
/// (B2 at the agent level).
#[tokio::test]
async fn om_crosses_observe_and_reflect_and_the_hint_is_one_shot() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // Real content accumulation: 5 synthesized entries cross the
    // 1000-token observe threshold (5 x 225 tokens).
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "a".repeat(900), "lane": "follow-up" }),
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
    let obs_text = format!(
        "<observations>obs {}</observations>",
        (0..900)
            .map(|i| format!("L{i:04} data"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ref_text = format!(
        "<observations>condensed {}</observations>",
        (0..200)
            .map(|i| format!("c{i:04}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
        sse_json(&ref_text),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let state = agent
        .inner
        .lock()
        .unwrap()
        .om
        .clone()
        .expect("the om state is written back");
    // Observe: the log is non-empty and the cursor sits on turn 1's
    // last raw entry (the raw window for turn 2 is the new user entry).
    assert!(state.record.active_observations.contains("obs"));
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000007");
    // Reflect: the tagged reflection committed its <observations>
    // content only, as generation 1.
    assert_eq!(state.record.generation, 1);
    assert!(state.record.active_observations.contains("condensed"));
    assert!(!state.record.active_observations.contains("<observations>"));
    // The continuation hint: exactly one assembled context in the whole
    // run carries it — the turn-2 assembly, right after the observe.
    let seen = provider.seen.lock().unwrap();
    let hints = seen
        .iter()
        .filter(|(_, first_input)| {
            first_input
                .as_deref()
                .is_some_and(|i| i.contains(crate::om::OBSERVATION_CONTINUATION_HINT))
        })
        .count();
    assert_eq!(hints, 1, "the hint is one-shot: {seen:?}");
}

/// The OM-run status hook (the app's `om_status` emitter): each run
/// reports its kind at start and `idle` at the end, in order.
#[tokio::test]
async fn om_runs_notify_the_status_hook_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "a".repeat(900), "lane": "follow-up" }),
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
    let obs_text = format!(
        "<observations>obs {}</observations>",
        (0..900)
            .map(|i| format!("L{i:04} data"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ref_text = format!(
        "<observations>condensed {}</observations>",
        (0..200)
            .map(|i| format!("c{i:04}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
        sse_json(&ref_text),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    let kinds = Arc::new(Mutex::new(Vec::<String>::new()));
    let k = kinds.clone();
    agent.set_om_status_hook(Some(Arc::new(move |kind: &str| {
        k.lock().unwrap().push(kind.to_owned());
    })));
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();
    assert_eq!(
        *kinds.lock().unwrap(),
        vec!["observing", "idle", "reflecting", "idle"]
    );
}

/// A compacted-spawn-seeded record: the frozen prefix survives the
/// child's reflect byte-identical (ADR-0004), and the reflector prompt
/// carries the frozen-prefix marker.
#[tokio::test]
async fn a_compacted_seed_keeps_the_frozen_prefix_byte_identical_across_reflect() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "a".repeat(900), "lane": "follow-up" }),
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
    let obs_text = format!(
        "<observations>obs {}</observations>",
        (0..900)
            .map(|i| format!("L{i:04} data"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ref_text = format!(
        "<observations>condensed {}</observations>",
        (0..200)
            .map(|i| format!("c{i:04}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
        sse_json(&ref_text),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord {
                frozen_prefix: "FROZEN PARENT LOG".into(),
                ..Default::default()
            },
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let state = agent
        .inner
        .lock()
        .unwrap()
        .om
        .clone()
        .expect("the om state is written back");
    assert_eq!(state.record.frozen_prefix, "FROZEN PARENT LOG");
    assert_eq!(state.record.generation, 1);
    // The reflector call used the frozen variant.
    let seen = provider.seen.lock().unwrap();
    let reflect = seen
        .iter()
        .find(|(_, content)| {
            content
                .as_deref()
                .is_some_and(|c| c.contains("<frozen-prefix>"))
        })
        .expect("the reflector prompt carries the frozen-prefix marker");
    assert!(
        reflect.1.as_deref().unwrap().contains("FROZEN PARENT LOG"),
        "{reflect:?}"
    );
}

/// Context gate (ticket #210): when the assembled prompt exceeds the safe
/// budget and there is unobserved material, the gate fires the OM pass
/// before the call, so the re-assembled context fits. A second turn is
/// idempotent — no unobserved material means no gate.
#[tokio::test]
async fn context_gate_fires_when_over_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // 10 entries of 3200 chars ≈ 8000 tokens, over the 7904 budget
    // (20000 − 8000 − 4096).
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
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse_json(obs_text), // observer call (from the gate)
        sse("a1", &[]),     // main call, turn 1
        sse("a2", &[]),     // main call, turn 2 (no gate)
    ]));
    let turn = TurnConfig {
        context_window: Some(20000),
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
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let seen = provider.seen.lock().unwrap();
    // Turn 1: gate fires (observer + main). Turn 2: no gate (main only).
    assert_eq!(seen.len(), 3, "obs + main + main: {seen:?}");
    // The first call is the observer, not the main prompt.
    assert_ne!(
        seen[0].0.as_deref(),
        Some("be terse"),
        "first call is the observer"
    );
    // Turn 1's main call and turn 2's main call both use the session prompt
    // (with the observation log appended by assemble_context).
    assert!(
        seen[1]
            .0
            .as_deref()
            .is_some_and(|s| s.starts_with("be terse")),
        "turn 1 main call: {:?}",
        seen[1].0
    );
    assert!(
        seen[2]
            .0
            .as_deref()
            .is_some_and(|s| s.starts_with("be terse")),
        "turn 2 main call: {:?}",
        seen[2].0
    );
    // The re-assembled input has no raw entries (all observed after the gate).
    assert!(
        seen[1]
            .1
            .as_deref()
            .is_none_or(|c| !c.contains(&"a".repeat(100))),
        "raw window is empty after the gate: {:?}",
        seen[1].1
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
        context_window: Some(20000),
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
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    let seen = provider.seen.lock().unwrap();
    // No observer call: the gate did not fire.
    assert_eq!(seen.len(), 1, "no gate: {seen:?}");
    assert_eq!(seen[0].0.as_deref(), Some("be terse"));
}

/// Context gate (ticket #210) degraded mode: the observer's output is so
/// large that the re-assembled prompt is still over budget. The gate warns
/// and proceeds — the turn completes, it just runs hot.
#[tokio::test]
async fn context_gate_degraded_when_still_over_budget() {
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
        sse_json(&huge_obs), // observer call (from the gate)
        sse("a1", &[]),      // main call (degraded: still over budget)
        sse_json(small_obs), // turn-end observer call (the "go" entry)
    ]));
    let turn = TurnConfig {
        context_window: Some(20000),
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
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    let seen = provider.seen.lock().unwrap();
    // Gate observer + main + turn-end observer.
    assert_eq!(seen.len(), 3, "gate obs + main + turn-end obs: {seen:?}");
    // The main call (index 1) proceeded with the session prompt.
    assert!(
        seen[1]
            .0
            .as_deref()
            .is_some_and(|s| s.starts_with("be terse")),
        "main call: {:?}",
        seen[1].0
    );
}
