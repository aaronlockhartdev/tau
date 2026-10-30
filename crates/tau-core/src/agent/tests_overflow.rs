use super::*;

use crate::agent::testkit::*;

/// A provider scripted to answer `Status` errors (the #255/#256 400 shape).
/// `seen` captures each call's `max_output_tokens` so the bounded retry's
/// halving is observable. The state rides an `Arc` (not `&self`) because
/// the trait hands back a future bound to the sink's lifetime.
pub(crate) struct Scripted400 {
    state: std::sync::Arc<Scripted400State>,
}

struct Scripted400State {
    calls: Vec<(u16, String)>,
    index: std::sync::atomic::AtomicUsize,
    seen: parking_lot::Mutex<Vec<Option<u64>>>,
}

impl Scripted400 {
    pub(crate) fn new(calls: Vec<(u16, String)>) -> Self {
        Self {
            state: std::sync::Arc::new(Scripted400State {
                calls,
                index: std::sync::atomic::AtomicUsize::new(0),
                seen: parking_lot::Mutex::new(Vec::new()),
            }),
        }
    }

    /// The captured per-call output caps (test assertion seam).
    pub(crate) fn seen(&self) -> &parking_lot::Mutex<Vec<Option<u64>>> {
        &self.state.seen
    }
}

/// A provider whose call dies in transport: no status, no body, nothing the
/// provider answered.
struct ScriptedIdle;

impl crate::provider::TurnProvider for ScriptedIdle {
    fn call<'a>(
        &self,
        _request: &ResponseRequest,
        _sink: &'a mut dyn TurnSink,
    ) -> crate::provider::ProviderTurn<'a> {
        Box::pin(async { Err(crate::provider::ProviderError::IdleTimeout) })
    }
}
impl crate::provider::TurnProvider for Scripted400 {
    fn call<'a>(
        &self,
        request: &ResponseRequest,
        _sink: &'a mut dyn TurnSink,
    ) -> crate::provider::ProviderTurn<'a> {
        // Clone the `Arc` by value: the future owns it; `&self` is valid
        // only for the call.
        let state = std::sync::Arc::clone(&self.state);
        let n = state
            .index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let script = state.calls.get(n).map(|(s, b)| (*s, b.clone()));
        let cap = request.max_output_tokens();
        Box::pin(async move {
            state.seen.lock().push(cap);
            match script {
                Some((status, body)) => {
                    Err(crate::provider::ProviderError::Status { status, body })
                }
                None => Ok(TurnResult {
                    outcome: crate::provider::StreamOutcome::Completed,
                    ..TurnResult::default()
                }),
            }
        })
    }
}

/// The #256 shape: an overflow that names no numbers, so the retry halves its
/// own cap.
fn context_400_body_blind() -> String {
    "This model's maximum context length was exceeded.".to_owned()
}

/// The provider's numbers leave nothing: the prompt alone fills the window, so
/// no cap saves the request.
fn context_400_body_no_room() -> String {
    "This model's maximum context length is 262144 tokens. However, your prompt contains at least 262145 input tokens."
        .to_owned()
}

fn context_400_body() -> String {
    // The provider states both sides: 262144 − 250000 − 4096 leaves 8048 for an
    // answer, so a repriced retry has room to carry.
    "This model's maximum context length is 262144 tokens. However, your prompt contains at least 250000 input tokens."
        .to_owned()
}

fn over_budget_turn() -> TurnConfig {
    // budget = 23896 − 4096 margin − 4096 smallest answer = 15704.
    TurnConfig {
        context_window: 23_896,
        max_output_tokens: Some(200),
        ..Default::default()
    }
}

fn omless_agent(
    store: SessionStore,
    provider: std::sync::Arc<dyn crate::provider::TurnProvider>,
    turn: TurnConfig,
    cwd: std::path::PathBuf,
) -> AgentSession {
    AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::surface::bare_specs(),
        cwd,
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn,
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    })
}

/// The provider's rejection of a request, as the session recorded it.
fn recorded_400(store: &SessionStore, want_body: &str) -> bool {
    entries_of(store).iter().any(|e| {
        e.kind == EntryKind::Diagnostic
            && e.payload["status"] == 400
            && e.payload["body"] == want_body
    })
}

/// #255 leg 2: the gate's blind spot — the prompt is over the safe budget
/// and there is no unobserved material to compact (OM off). The turn is
/// refused loudly before the provider is called: no doomed 400 on the
/// wire, and the refusal leaves a durable diagnostic record.
#[tokio::test]
async fn over_budget_without_unobserved_material_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(Scripted400::new(vec![]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        over_budget_turn(),
        dir.path().to_path_buf(),
    );
    // A ~16.2k-token user message, over the 15704 budget.
    agent.send("a".repeat(65000), Lane::FollowUp);

    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(&err, AgentError::ContextOverflow { prompt, budget } if *prompt > *budget),
        "expected ContextOverflow, got {err:?}"
    );
    // The provider was never called: the refusal is pre-wire.
    assert!(provider.seen().lock().is_empty());
    // The refusal is durable: a diagnostic record with prompt > budget. The
    // agent moved the store, so re-open the workspace file to inspect it.
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let entries = entries_of(&reopened);
    assert!(
        entries.iter().any(|e| {
            e.kind == EntryKind::Diagnostic
                && e.payload["prompt"].as_u64().unwrap() > e.payload["budget"].as_u64().unwrap()
        }),
        "a diagnostic record names the overflow: {entries:?}"
    );
}

/// #255 leg 2 negative: a prompt under the safe budget goes out normally —
/// the refusal only fires over budget.
#[tokio::test]
async fn under_budget_is_not_refused() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(ScriptedProvider::new(vec![sse("ok", &[])]));
    let agent = AgentSession::new(SessionParams {
        store: session_in(dir.path()),
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::surface::bare_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: over_budget_turn(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    // ~10k tokens: under the 15704 budget.
    agent.send("a".repeat(40000), Lane::FollowUp);
    agent.process().await.unwrap();
    assert_eq!(provider.seen.lock().len(), 1);
}

/// #338: a provider context-length 400 that states its arithmetic is repriced
/// from the provider's own numbers — the retry carries the room the window
/// leaves, not a guessed half. The turn completes.
#[tokio::test]
async fn context_400_reprices_from_the_provider_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(Scripted400::new(vec![(400, context_400_body())]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        TurnConfig {
            max_output_tokens: Some(8000),
            ..Default::default()
        },
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    // 262144 − 250000 − 4096 = 8048: the retry carries the provider's remainder.
    let seen = provider.seen().lock();
    assert_eq!(seen.len(), 2, "one bounded retry: {seen:?}");
    assert_eq!(seen[0], Some(8000), "first call: the wished cap");
    assert_eq!(seen[1], Some(8_048), "retry: the provider's room");
}

/// #256: an overflow that names no numbers falls back to the halved wish — the
/// bounded retry the original ticket asked for, kept for that shape.
#[tokio::test]
async fn context_400_without_numbers_halves_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(Scripted400::new(vec![(400, context_400_body_blind())]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        TurnConfig {
            max_output_tokens: Some(8000),
            ..Default::default()
        },
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    let seen = provider.seen().lock();
    assert_eq!(seen.len(), 2, "one bounded retry: {seen:?}");
    assert_eq!(seen[0], Some(8000), "first call: full cap");
    assert_eq!(seen[1], Some(4000), "retry: halved");
}

/// #338: a 400 whose numbers say the prompt alone fills the window is not
/// retried — no cap saves it. The refusal is the gate's, durable, and the
/// second call never happens.
#[tokio::test]
async fn context_400_with_no_room_is_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(Scripted400::new(vec![(400, context_400_body_no_room())]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        TurnConfig {
            max_output_tokens: Some(8000),
            ..Default::default()
        },
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);

    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(&err, AgentError::ContextOverflow { .. }),
        "no room means a refusal, not a retry: {err:?}"
    );
    assert_eq!(provider.seen().lock().len(), 1, "the retry is not sent");
}

/// #256: the retry is bounded — a second context 400 fails loudly (the
/// error propagates) after a durable system entry records the rejection.
#[tokio::test]
async fn context_400_fails_loudly_after_one_retry() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(Scripted400::new(vec![
        (400, context_400_body_blind()),
        (400, context_400_body_blind()),
    ]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        over_budget_turn(),
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);

    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(
            &err,
            AgentError::Provider(crate::provider::ProviderError::Status { status: 400, .. })
        ),
        "the rejection propagates: {err:?}"
    );
    assert_eq!(provider.seen().lock().len(), 2, "no loop: two calls");
    // The durable trace: a provider_rejected record with the status and body.
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert!(
        recorded_400(&reopened, &context_400_body_blind()),
        "the 400 is recorded"
    );
}

/// #256 negative: a non-context 400 (auth, schema) is not a context
/// rejection — no retry, one call, and the rejection is still recorded.
#[tokio::test]
async fn non_context_400_is_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let auth_body = "Invalid API key provided. Check your credentials.".to_owned();
    let provider = std::sync::Arc::new(Scripted400::new(vec![(400, auth_body.clone())]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        over_budget_turn(),
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);

    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(
            &err,
            AgentError::Provider(crate::provider::ProviderError::Status { status: 400, .. })
        ),
        "the rejection propagates: {err:?}"
    );
    assert_eq!(
        provider.seen().lock().len(),
        1,
        "no retry for a 401-class 400"
    );
    // The 400 was never recorded before #256: the durable trace now exists
    // for every rejection, context or not.
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert!(recorded_400(&reopened, &auth_body), "the 400 is recorded");
}

#[tokio::test]
async fn a_huge_rejection_body_is_recorded_bounded() {
    // The record is a durable sink: a proxy's HTML error page lands in the
    // session file only as its head. The error that propagates keeps all of it.
    let dir = tempfile::tempdir().unwrap();
    let page =
        "<!DOCTYPE html>\n".to_string() + &"<p>gateway rejected the request</p>\n".repeat(2_000);
    let provider = std::sync::Arc::new(Scripted400::new(vec![(400, page.clone())]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        over_budget_turn(),
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);

    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(
            &err,
            AgentError::Provider(crate::provider::ProviderError::Status { status: 400, body })
                if body == &page
        ),
        "the caller still sees the whole body: {err:?}"
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let recorded = entries_of(&reopened)
        .into_iter()
        .find(|e| e.kind == EntryKind::Diagnostic)
        .expect("a provider_rejected record");
    let body = recorded.payload["body"].as_str().unwrap_or_default();
    assert!(
        body.chars().count() <= super::turn::REJECTION_BODY_CAP + 1,
        "the record carries a head, not the page: {} chars",
        body.chars().count()
    );
    assert!(body.ends_with('…'), "the cut is marked");
    assert!(
        page.starts_with(body.strip_suffix('…').unwrap_or_default()),
        "the head is the page's head"
    );
}

/// The gate is either a budget or a named gap, never both and never neither:
/// the two facts by config name, or the room that is missing when both are
/// declared (ADR-0034). One predicate answers for the gate and both record
/// sites, so a record cannot name a gap the gate does not have.
#[test]
fn the_gate_gap_names_what_the_gate_lacks() {
    let declared = |window, max_output| TurnConfig {
        context_window: window,
        max_output_tokens: Some(max_output),
        ..Default::default()
    };
    // A hand-built turn with no config behind it carries the zero window, the
    // only way a gap is reachable now (#338: config requires a window, and a
    // default turn gates).
    let unconfigured = TurnConfig {
        context_window: 0,
        max_output_tokens: None,
        ..Default::default()
    };
    assert_eq!(
        unconfigured.context_gate(),
        ContextGate::Missing {
            reasons: ["context_window".to_owned(), "max_tokens".to_owned()].into()
        },
        "a turn with no config behind it names both facts"
    );
    assert_eq!(
        TurnConfig::default().context_gate(),
        ContextGate::Ready {
            window: crate::config::Provider::DEFAULT_WINDOW,
            cap: u64::from(crate::config::Provider::DEFAULT_MAX_OUTPUT),
        },
        "the default turn gates: #338 removed the unbounded prompt"
    );
    assert_eq!(
        declared(20_000, 200).context_gate(),
        ContextGate::Ready {
            window: 20_000,
            cap: 200,
        },
        "a computable budget is no gap"
    );
    // The config that asks for the whole window arms, it does not die: a
    // `max_tokens` of `context_window` is a ceiling on the answer, so the gate
    // gates on the window and the clamp shrinks the wish. This is the shape that
    // refused every session on a `context_window = max_tokens = 200000` model.
    assert_eq!(
        declared(200_000, 200_000).context_gate(),
        ContextGate::Ready {
            window: 200_000,
            cap: 200_000,
        },
        "a wish at the window is a shrink, not a gap"
    );
    assert_eq!(
        declared(8_192, 200).context_gate(),
        ContextGate::Missing {
            reasons: vec!["context_room".to_owned()]
        },
        "the window alone can leave no room: 8192 cannot carry the margin and an answer"
    );
    assert_eq!(
        declared(8_192, 8_192).context_gate(),
        ContextGate::Missing {
            reasons: vec!["context_room".to_owned()]
        },
        "a wish at the window meets the same window gap"
    );
    assert_eq!(
        TurnConfig {
            context_window: 0,
            max_output_tokens: Some(200),
            ..Default::default()
        }
        .context_gate(),
        ContextGate::Missing {
            reasons: vec!["context_window".to_owned()]
        },
        "one declared fact names only the other"
    );
    let margin = crate::provider::CONTEXT_GATE_MARGIN;
    let answer = crate::provider::MIN_ANSWER_ROOM;
    assert_eq!(
        declared(20_000, 200).context_gate().budget(),
        Some(20_000 - margin - answer)
    );
    assert_eq!(
        declared(200_000, 200_000).context_gate().budget(),
        Some(200_000 - margin - answer),
        "the prompt's ceiling is the window's, not the wish's"
    );
    assert_eq!(
        clamp_max_output(200_000, 200_000, 3_000),
        192_904,
        "a fresh session on that model gets the room the window leaves"
    );
    assert_eq!(
        declared(8_192, 8_192).context_gate().budget(),
        None,
        "no gate means no budget to enforce"
    );
    // The cap is derived from the prompt by the same arithmetic the gate's
    // clamp applies, and a prompt that eats the window leaves no cap: the gate
    // refuses rather than send a doomed request.
    let gate = declared(20_000, 8_000).context_gate();
    let ContextGate::Ready { window, cap } = gate else {
        panic!("a declared window and cap arm the gate");
    };
    assert_eq!(clamp_max_output(cap, window, 10_000), 5_904);
    assert_eq!(
        clamp_max_output(cap, window, 20_000),
        0,
        "a prompt that fills the window leaves no room to answer"
    );
    assert_eq!(unconfigured.context_gate().budget(), None);
}

/// A transport failure is the turn's error to surface, not a fact the provider
/// produced: nothing lands in the record channel (ADR-0034).
#[tokio::test]
async fn a_dead_stream_files_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let agent = omless_agent(
        session_in(dir.path()),
        std::sync::Arc::new(ScriptedIdle),
        over_budget_turn(),
        dir.path().to_path_buf(),
    );
    agent.send("go", Lane::FollowUp);
    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(
            err,
            AgentError::Provider(crate::provider::ProviderError::IdleTimeout)
        ),
        "the failure propagates: {err:?}"
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert!(
        entries_of(&reopened)
            .iter()
            .all(|e| e.kind != EntryKind::Diagnostic),
        "a dead stream is not a record: {:?}",
        entries_of(&reopened)
    );
}

/// Scope 1 (#338) at turn level: the `Missing` gate is a refusal, never an
/// unbounded prompt. An 8192 window cannot carry the framing margin and the
/// smallest answer — `8192 − 4096 − 4096` is nothing — so the turn refuses
/// before the wire and files the durable `gate_unavailable` naming the gap.
/// The predicate's own test proves the gate;
/// this proves the wire stays untouched.
#[tokio::test]
async fn a_gate_with_no_room_refuses_the_turn_and_calls_no_provider() {
    let dir = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(ScriptedProvider::new(vec![sse("ok", &[])]));
    let agent = omless_agent(
        session_in(dir.path()),
        provider.clone(),
        TurnConfig {
            context_window: 8_192,
            max_output_tokens: Some(8_192),
            ..Default::default()
        },
        dir.path().to_path_buf(),
    );
    // Small on purpose: the refusal is the gate's facts, not the prompt's size.
    agent.send("go", Lane::FollowUp);

    let err = agent.process().await.unwrap_err();
    assert!(
        matches!(
            &err,
            AgentError::GateUnavailable { reasons, .. } if reasons == &["context_room".to_owned()]
        ),
        "expected the gate's refusal, got {err:?}"
    );
    assert!(
        provider.seen.lock().is_empty(),
        "a refusal sends nothing: {} calls",
        provider.seen.lock().len()
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let recorded = entries_of(&reopened);
    let records: Vec<&serde_json::Value> = recorded
        .iter()
        .filter(|e| e.kind == EntryKind::Diagnostic && e.payload["code"] == "gate_unavailable")
        .map(|e| &e.payload)
        .collect();
    assert_eq!(
        records.len(),
        1,
        "the refusal is durable, once: {records:?}"
    );
    assert_eq!(
        records[0]["missing"],
        serde_json::json!(["context_room"]),
        "the record names the gap: {}",
        records[0]
    );
}
