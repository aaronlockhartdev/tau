use super::*;

use crate::agent::testkit::*;
use tau_protocol::Diagnostic;

/// The `[om]` a test reaches cheaply: a 1000-token activation threshold, a
/// 230-token buffer increment, and the sync Buffer arm on.
fn small_om() -> crate::config::Om {
    crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 100_000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    }
}

/// Raw entries past the 1000-token activation: 5 × 900 chars ≈ 1130 tokens.
fn seed_past_activation(store: &mut SessionStore) {
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
}

/// The stall tests' session: the config above on the session's own route.
fn om_agent(
    store: SessionStore,
    cwd: &std::path::Path,
    provider: Arc<ScriptedProvider>,
) -> AgentSession {
    AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: cwd.to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &small_om(),
            crate::om::OmRecord::default(),
        )),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    })
}

fn observer_calls_in(provider: &ScriptedProvider) -> usize {
    let observer = crate::om::observer_system_prompt();
    provider
        .seen
        .lock()
        .iter()
        .filter(|(instructions, _)| instructions.as_deref() == Some(observer.as_str()))
        .count()
}

#[tokio::test]
async fn a_window_compaction_cannot_drain_records_one_stall_at_the_gate_quantity() {
    // #392: the stall record and the plan's gate speak one number. The
    // Observer's output is the degenerate shape, so the settle runs, commits
    // nothing, and leaves the window at activation.
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    seed_past_activation(&mut store);
    // 100 identical 30-char lines: the ported degenerate detector rejects this
    // shape, so no observation lands.
    let degenerate = format!(
        "<observations>{}</observations>",
        (0..100)
            .map(|_| "the same observation line")
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&degenerate),
    ]));
    let agent = om_agent(store, dir.path(), provider.clone());
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    // The pass ran and committed nothing, so the window stayed at activation.
    assert_eq!(
        observer_calls_in(&provider),
        1,
        "the settle ran the Observer"
    );
    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert!(state.record.cursor.is_none());
    let stalls = stalled_in(dir.path());
    assert_eq!(stalls.len(), 1, "one stall per run: {stalls:?}");
    let Diagnostic::OmStalled {
        pending,
        activation,
    } = stalls[0]
    else {
        panic!("expected an om_stalled record, got {stalls:?}");
    };
    assert_eq!(activation, 1000, "the activation an empty log arms");
    // One quantity: the stall's number is the window the settle stamped, and
    // the window a reopen measures from the durable record.
    assert_eq!(u64::from(state.record.pending_tokens), pending);
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let fresh = crate::om_integration::OmState::from_config(
        &small_om(),
        crate::om_integration::OmState::load_record(&mut reopened).unwrap(),
    );
    assert_eq!(
        u64::from(fresh.unobserved_tokens(&mut reopened).unwrap()),
        pending,
        "the stall names a volume the gate did not see"
    );
}

#[tokio::test]
async fn a_settle_that_drains_the_window_acts_on_tokens_and_records_no_stall() {
    // The negative pair, and the activation itself: the two turns run back to
    // back (the idle rung needs IDLE_ACTIVATION_SECS), so it is the 1130-token
    // window that moves the cursor. The settle observes the bounded slice — four
    // entries, 900 tokens, under the 1000-token bound (#394) — so the cursor
    // stops on entry 4 and the tail rides on. The window then sits under
    // activation, and no stall is recorded.
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    seed_past_activation(&mut store);
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json("<observations>did the work</observations>"),
        sse("a2", &[]),
    ]));
    let agent = om_agent(store, dir.path(), provider.clone());
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    assert_eq!(
        observer_calls_in(&provider),
        1,
        "the first settle activated on tokens, the second had no window to activate on"
    );
    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert!(state.record.active_observations.contains("did the work"));
    assert!(state.record.active_observations.contains("did the work"));
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000004");
    assert!(
        stalled_in(dir.path()).is_empty(),
        "a drained window records no stall"
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let fresh = crate::om_integration::OmState::from_config(
        &small_om(),
        crate::om_integration::OmState::load_record(&mut reopened).unwrap(),
    );
    assert!(
        fresh.unobserved_tokens(&mut reopened).unwrap() < 1000,
        "the window came back under activation"
    );
}
