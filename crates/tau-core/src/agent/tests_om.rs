use super::*;

use crate::agent::testkit::*;

/// End-to-end OM (the ticket's acceptance bar): synthesized raw entries
/// cross the observe threshold, the canned Observer fills the log past
/// the reflect threshold, the canned Reflector rewrites the suffix —
/// and every request the run sends answers a user query (#334), the hint
/// riding on each assembly once observations exist (B2 at the agent level).
#[tokio::test]
async fn om_crosses_observe_and_reflect_and_every_request_carries_a_user_query() {
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
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    // Observe: the log is non-empty, and the cursor sits on the bounded slice
    // the settle observed — 4 × 225 tokens is under the 1000-token bound, a fifth
    // entry passes it, so the window's tail rides into turn 2 (#394).
    assert!(state.record.active_observations.contains("obs"));
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000004");
    // Reflect: the tagged reflection committed its <observations>
    // content only, as generation 1.
    assert_eq!(state.record.generation, 1);
    assert!(state.record.active_observations.contains("condensed"));
    assert!(!state.record.active_observations.contains("<observations>"));
    // The user-query invariant (#334): every request the run sent — main turns
    // and OM calls alike — carries at least one `role: user` item. A run where
    // the last user entry is observed and the tail is tool calls used to send a
    // request with none, and a chat template answered 400 "No user query found
    // in messages".
    let userless: Vec<usize> = provider
        .user_contents
        .lock()
        .iter()
        .enumerate()
        .filter(|(_, users)| users.is_empty())
        .map(|(i, _)| i)
        .collect();
    assert!(
        userless.is_empty(),
        "requests without a user query: {userless:?}"
    );
    // The hint rides the assemblies made after the observe, so a request has a
    // user item beyond the raw window's; the per-assembly gate itself is pinned
    // at the assembly seam (window.rs).
    let seen = provider.seen.lock();
    let with_hint = seen
        .iter()
        .filter(|(_, first_input)| {
            first_input
                .as_deref()
                .is_some_and(|i| i.contains(crate::om::OBSERVATION_CONTINUATION_HINT))
        })
        .count();
    assert!(with_hint >= 1, "the hint never reached a request: {seen:?}");
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
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    let kinds = Arc::new(Mutex::new(Vec::<String>::new()));
    let k = kinds.clone();
    agent.set_om_status_hook(Some(Arc::new(move |kind: &str| {
        k.lock().push(kind.to_owned());
    })));
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();
    assert_eq!(
        *kinds.lock(),
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
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert_eq!(state.record.frozen_prefix, "FROZEN PARENT LOG");
    assert_eq!(state.record.generation, 1);
    // The reflector call used the frozen variant.
    let seen = provider.seen.lock();
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

#[tokio::test]
async fn a_turn_end_spawns_the_buffer_cycle_on_a_crossed_interval() {
    // #399: a turn with no tool round never reaches the mid-loop trigger, so
    // a crossed interval at the settle used to wait for the next turn to
    // spawn its slice. Seeded raw past one interval (under activation, with
    // async on): the settle plans Done, the turn end spawns the cycle, and
    // the cycle's commit lands as an observation chunk counting the raw of
    // the slice it covered — the cycle's call is bounded, not a call over
    // the whole unbuffered window (#399).
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // 3 × 900 chars ≈ 675 tokens: past the 600 interval, under the 1000
    // activation.
    let mut parent: Option<String> = None;
    for _ in 0..3 {
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
        observe_threshold: 1_000,
        reflect_threshold: 2_000,
        buffer_increment: 230,
        buffer_tokens: 600,
        retries: crate::config::OmRetries::default(),
    };
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("done", &[]),
        sse_json("<observations>the slice, observed</observations>"),
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
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    // The cycle is spawned, not awaited: the turn ends with it in flight,
    // and it lands its commit when the runtime next runs it.
    // The guard must drop before the join: the cycle re-acquires the
    // session's lock to commit, and a guard held across the await deadlocks it.
    let handle = agent.inner.lock().om_inflight.take();
    if let Some(handle) = handle {
        handle.await.unwrap();
    }
    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert_eq!(
        state.record.buffered_chunks.len(),
        1,
        "the cycle committed no chunk"
    );
    let chunk = &state.record.buffered_chunks[0];
    assert!(
        chunk.text.contains("the slice, observed"),
        "the chunk is not the cycle's observation"
    );
    // The cycle's call is the next increment slice, not the whole
    // unbuffered window: the 230-increment floor is one seeded entry (900
    // chars ≈ 225 tokens); the window is 675 plus the turn's records.
    assert!(
        (200..675).contains(&chunk.tokens),
        "the chunk counts {}, not one entry-bounded increment slice",
        chunk.tokens
    );
    assert!(state.record.last_buffered_at_tokens >= 600);
}

#[tokio::test]
async fn the_kill_switch_leaves_the_turn_end_spawning_nothing() {
    // The negative pair (#399): with `buffer_tokens = 0` the sync Buffer arm
    // owns the sub-threshold range — the settle observes the bounded slice
    // itself, and the turn end spawns no cycle.
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    let mut parent: Option<String> = None;
    for _ in 0..3 {
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
        observe_threshold: 1_000,
        reflect_threshold: 2_000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("done", &[]),
        sse_json("<observations>the slice, observed</observations>"),
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
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    assert!(
        agent.inner.lock().om_inflight.is_none(),
        "the kill switch must spawn no background cycle"
    );
    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert_eq!(state.record.buffered_chunks.len(), 1);
    assert!(
        state.record.buffered_chunks[0]
            .text
            .contains("the slice, observed")
    );
}
