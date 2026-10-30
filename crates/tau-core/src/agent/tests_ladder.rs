//! The context gate's ladder and the `om_failed` record (#338): each rung
//! earns a re-pricing only by compacting something, and a compaction whose
//! round-trip dies is recorded once, at the stage it died at.

use super::*;

use crate::agent::testkit::*;
use tau_protocol::Diagnostic;

/// The `om_failed` records a session file holds, in append order, by stage.
fn om_failed_stages(store: &SessionStore) -> Vec<String> {
    store
        .entries_range(0, usize::MAX)
        .expect("the session opens")
        .iter()
        .filter(|e| e.kind == EntryKind::Diagnostic)
        .filter_map(|e| Diagnostic::from_value(&e.payload))
        .filter_map(|d| match d {
            Diagnostic::OmFailed { stage, .. } => Some(stage),
            _ => None,
        })
        .collect()
}

/// A provider that answers main turns and drives the OM round-trips: each OM
/// call takes the next scripted body, and past the end of the script — or at
/// every call, with `dead` — it fails with the permanent 400 an `om_model`
/// route naming no model returns. The calls are counted apart, so a test can
/// say which rung ran.
struct OmScript {
    om_calls: std::sync::atomic::AtomicUsize,
    main_calls: std::sync::atomic::AtomicUsize,
    /// Each OM call's answer, already in its `<observations>` tags; past the
    /// end of this list the route is dead — the permanent 400 an `om_model`
    /// naming no model returns.
    answers: Vec<String>,
    /// Each OM call's `role: user` content — the Reflector prompt rides the
    /// body, not the instructions.
    om_bodies: parking_lot::Mutex<Vec<String>>,
    /// Each main call's request as built: the output cap and the prompt's parts
    /// (#338's window fence — the inequality is measured off this object, not
    /// off the gate's own number).
    main_requests: parking_lot::Mutex<Vec<MainRequest>>,
}

/// A built main request, as the window fence reads it.
#[derive(Debug)]
pub(crate) struct MainRequest {
    pub(crate) cap: Option<u64>,
    pub(crate) instructions: String,
    pub(crate) input: Vec<InputEntry>,
}

impl OmScript {
    /// A provider whose OM calls answer with each observation text in turn.
    fn scripted(observations: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            om_calls: std::sync::atomic::AtomicUsize::new(0),
            main_calls: std::sync::atomic::AtomicUsize::new(0),
            answers: observations
                .into_iter()
                .map(|o| format!("<observations>{o}</observations>"))
                .collect(),
            om_bodies: parking_lot::Mutex::new(Vec::new()),
            main_requests: parking_lot::Mutex::new(Vec::new()),
        })
    }

    /// A provider whose every OM call dies: the dead-route shape (#338).
    fn dead() -> Arc<Self> {
        Self::scripted(Vec::new())
    }
}

impl crate::provider::TurnProvider for OmScript {
    fn call<'a>(
        &self,
        request: &ResponseRequest,
        sink: &'a mut dyn TurnSink,
    ) -> crate::provider::ProviderTurn<'a> {
        // A main turn carries the session's system prompt; an OM round-trip
        // carries the Observer's or the Reflector's.
        if request
            .instructions()
            .is_some_and(|i| i.starts_with("be terse"))
        {
            self.main_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.main_requests.lock().push(MainRequest {
                cap: request.max_output_tokens(),
                instructions: request.instructions().unwrap_or_default().to_owned(),
                input: request.input().to_vec(),
            });
            let canned = crate::provider::canned(&sse("ok", &[]));
            return canned.call(request, sink);
        }
        let n = self
            .om_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.om_bodies.lock().push(
            request
                .input()
                .iter()
                .filter_map(|i| match i {
                    InputEntry::Message(m) if m.role == "user" => Some(m.content.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        match self.answers.get(n) {
            Some(answer) => {
                let canned = crate::provider::canned(&sse_json(answer));
                canned.call(request, sink)
            }
            None => Box::pin(async {
                Err(crate::provider::ProviderError::Status {
                    status: 400,
                    body: "no model named dev/om-model".to_owned(),
                })
            }),
        }
    }
}

/// The ladder fixture: a 16096 window, a wish the window leaves room for.
fn agent_on(
    dir: &std::path::Path,
    store: SessionStore,
    provider: Arc<OmScript>,
    om: crate::om_integration::OmState,
) -> AgentSession {
    agent_wish_at(dir, store, provider, om, 16_096, 8_000)
}

/// A session whose gate is armed at `window`: the prompt's budget is
/// `window − margin − answer` (7904 at 16096) — the quantity the ladder prices
/// against. `wish` is a ceiling on the answer and sets no part of that budget.
fn agent_wish_at(
    dir: &std::path::Path,
    store: SessionStore,
    provider: Arc<OmScript>,
    om: crate::om_integration::OmState,
    window: u32,
    wish: u64,
) -> AgentSession {
    AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::surface::bare_specs(),
        cwd: dir.to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig {
            context_window: window,
            max_output_tokens: Some(wish),
            ..Default::default()
        },
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(om),
        om_model: String::new(),
        subagents: None,
        child: None,
    })
}

/// The OM config these runs share: the reflect threshold out of reach (so a
/// turn-end pass plans no reflect of its own), the sync buffer (the kill
/// switch) on, and an observe threshold set by the caller in tokens.
fn om_config(observe_threshold: u64) -> crate::config::Om {
    crate::config::Om {
        om_model: String::new(),
        observe_threshold,
        reflect_threshold: 100_000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    }
}

fn state(observe_threshold: u64, record: crate::om::OmRecord) -> crate::om_integration::OmState {
    crate::om_integration::OmState::from_config(&om_config(observe_threshold), record)
}

/// Text of `tokens` tokens, every 200-character window distinct: the log's
/// size is the prompt's size (the estimator's quarter-of-bytes rule), and
/// `detect_degenerate_repetition` reads repeated filler as a broken model.
fn bulk(tokens: usize) -> String {
    (0..tokens)
        .map(|i| format!("item {i} covers file src/part{i:03}.rs with hash {i:08x}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The F4 pin (#338): a promote whose record `save` fails names the `session`
/// stage — no LLM pass ran — and leaves the stages that DO run an LLM pass free
/// to record. Before the fix this arm hardcoded `buffer`, so the GUI card read
/// "compaction buffer pass failed" and the latch suppressed the later, real
/// record of that stage.
///
/// The save is broken by the sidecar-blob write: the OM record's payload is over
/// the 100 KB blob threshold, and `blobs` is a regular file, so the blob cannot
/// be created. Diagnostic payloads stay under the threshold, so every record
/// this test asserts on appends normally.
#[tokio::test]
async fn a_promote_whose_save_fails_records_the_session_stage() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 10, 3_200);
    // The oversized log: the record's payload clears the blob threshold, the
    // prompt clears the 7904 budget by a mile (so the ladder runs to the end).
    let mut om = state(
        1_000,
        crate::om::OmRecord {
            active_observations: log(2, 15_000),
            observation_tokens: 30_000,
            ..Default::default()
        },
    );
    om.record.pending_tokens = 8_000;
    let chunk = |first: &str, last: &str| crate::om_integration::BufferedChunk {
        range: (first.into(), last.into()),
        last_ts: 100,
        text: "promoted chunk".into(),
        tokens: 5_000,
    };
    om.buffered = vec![chunk("00000001", "00000010"), chunk("00000011", "00000011")];
    om.record.buffered_chunks = om.buffered.clone();
    // `blobs` as a file: the oversized record's blob write is the failure.
    std::fs::write(dir.path().join(".tau/blobs"), "not a directory").unwrap();
    let provider = OmScript::dead();
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    let err = agent
        .process()
        .await
        .expect_err("a prompt no compaction shrinks is refused");
    assert!(
        matches!(&err, AgentError::ContextOverflow { prompt, budget } if *prompt > *budget),
        "expected ContextOverflow, got {err:?}"
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    let stages = om_failed_stages(&reopened);
    assert_eq!(
        stages.first().map(String::as_str),
        Some("session"),
        "the promote's storage failure names the store, and is the first rung: {stages:?}"
    );
    assert!(
        !stages.contains(&"buffer".to_owned()),
        "a promote runs no buffer round-trip, so it files no buffer failure: {stages:?}"
    );
    let latched = agent.inner.lock().om_failed_recorded.clone();
    assert!(
        latched.contains(&"session".to_owned()) && !latched.contains(&"buffer".to_owned()),
        "the latch holds the stage that died; the buffer stage stays free to record: {latched:?}"
    );
}

/// The window fence (#338 acceptance): a declared window yields a request where
/// `prompt + cap + margin <= window`, asserted on the built `ResponseRequest` —
/// measured off the request the provider received (the same estimate the gate
/// prices a bare prompt with), never off the gate's own number.
#[tokio::test]
async fn the_built_request_respects_the_window_fence() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 10, 3_200);
    let mut om = state(1_000, crate::om::OmRecord::default());
    om.record.pending_tokens = 8_000;
    let chunk = |first: &str, last: &str| crate::om_integration::BufferedChunk {
        range: (first.into(), last.into()),
        last_ts: 100,
        text: "promoted chunk".into(),
        tokens: 5_000,
    };
    om.buffered = vec![chunk("00000001", "00000010"), chunk("00000011", "00000011")];
    om.record.buffered_chunks = om.buffered.clone();
    let provider = OmScript::scripted(Vec::new());
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    let built = provider.main_requests.lock();
    assert_eq!(built.len(), 1, "the ladder's rung 1 fitted the prompt");
    let request = built.first().expect("one main request");
    let tools_json = serde_json::to_string(&tools::surface::bare_specs()).unwrap();
    let prompt =
        crate::provider::prompt_token_estimate(&request.instructions, &request.input, &tools_json);
    let cap = request.cap.expect("the armed gate's request carries a cap");
    assert!(
        u64::from(prompt) + cap + u64::from(crate::provider::CONTEXT_GATE_MARGIN) <= 16_096,
        "the request breaches its window: prompt {prompt} + cap {cap} + margin \
         {} > 16096",
        crate::provider::CONTEXT_GATE_MARGIN
    );
    // The wish survives while the window leaves room: the cap is the smaller of
    // the wish and the room, so the fence is doing arithmetic, not clipping to 0.
    assert!(
        cap > 0 && cap <= 8_000,
        "the cap is the wish, clamped: {cap}"
    );
}

/// #343's positive at the wire: the incident's config is a wish at the window,
/// and it must send. The gate arms on the window, so a bare prompt sits under
/// the ceiling, no rung runs, and the request carries the room the window
/// leaves — the wish shrinks, nothing is refused.
#[tokio::test]
async fn a_wish_at_the_window_sends_and_carries_the_room() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 2, 200);
    let om = state(1_000, crate::om::OmRecord::default());
    let provider = OmScript::scripted(Vec::new());
    let agent = agent_wish_at(dir.path(), store, provider.clone(), om, 16_096, 16_096);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    let built = provider.main_requests.lock();
    assert_eq!(built.len(), 1, "a session on the incident's config sends");
    let request = built.first().expect("one main request");
    let tools_json = serde_json::to_string(&tools::surface::bare_specs()).unwrap();
    let prompt =
        crate::provider::prompt_token_estimate(&request.instructions, &request.input, &tools_json);
    let cap = request.cap.expect("the armed gate's request carries a cap");
    assert!(
        prompt <= 7_904,
        "a bare prompt is under the 7904 ceiling: {prompt}"
    );
    assert_eq!(
        cap,
        clamp_max_output(16_096, 16_096, prompt),
        "the wire carries the room the window leaves, not the wish"
    );
    assert!(cap < 16_096, "the wish at the window is shrunk: {cap}");
    assert!(
        cap >= u64::from(crate::provider::MIN_ANSWER_ROOM),
        "the gate arms only where an answer fits: {cap}"
    );
}

/// The fence's negative (#338): a prompt the declared window cannot carry is
/// refused before the wire, so the fence is never satisfied by an absent
/// request — the breach is sent by nobody, and the cap the clamp derives from
/// that prompt is 0.
#[tokio::test]
async fn a_prompt_that_fills_the_window_leaves_no_cap_and_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // A prompt past `window − margin − answer` is the fence's negative: 53000
    // bytes price ~16600 tokens (the estimator's quarter-of-bytes rule plus its
    // 20% serialization allowance), over the 7904 budget the 16096 window arms.
    // `clamp_max_output` finds no room for an answer, and 0 is refusal.
    let mut store = session_in(dir.path());
    store
        .append(
            EntryKind::User,
            serde_json::json!({ "text": "a".repeat(53_000), "lane": "follow-up" }),
            None,
        )
        .unwrap();
    let provider = OmScript::scripted(Vec::new());
    let om = state(100_000, crate::om::OmRecord::default());
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    let err = agent
        .process()
        .await
        .expect_err("a prompt that fills the window is refused");
    assert!(
        matches!(
            &err,
            AgentError::ContextOverflow { prompt, budget } if *prompt > *budget
        ),
        "expected the pre-wire refusal, got {err:?}"
    );
    let sent = provider.main_requests.lock();
    assert!(
        sent.is_empty(),
        "a request built on this prompt would breach the fence and is never sent: {sent:?}"
    );
    // The fence's arithmetic on the refused prompt: the cap it derives is 0, so
    // the only request this prompt could carry breaches the window. The fence is
    // not satisfied by an absent request — it is satisfied by a refusal.
    let AgentError::ContextOverflow { prompt, budget } = err else {
        panic!("the refusal carries the numbers: {err:?}")
    };
    assert_eq!(
        crate::provider::clamp_max_output(8_000, 16_096, prompt),
        0,
        "the prompt leaves no room for an answer: prompt {prompt}, budget {budget}"
    );
    assert!(
        u64::from(prompt) + u64::from(crate::provider::CONTEXT_GATE_MARGIN) > 16_096,
        "any positive cap on this prompt breaches the window: prompt {prompt} \
         + margin {} > 16096",
        crate::provider::CONTEXT_GATE_MARGIN
    );
}

/// The observation log as `groups` provenance groups of `tokens` tokens each:
/// groups are what reach the prompt (spec §4 — the main context carries one
/// `role: system` item per group).
fn log(groups: usize, tokens: usize) -> String {
    (0..groups)
        .map(|g| {
            format!(
                "<observation-group id=\"g{g}\" range=\"{}:{}\">\n{}\n</observation-group>",
                g + 1,
                g + 1,
                bulk(tokens)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A branch of `count` chained user entries of 3200 chars each: ~8000 tokens
/// of raw window, over the 7904 budget.
fn store_with_raw(dir: &std::path::Path, count: usize, chars: usize) -> SessionStore {
    let mut store = session_in(dir);
    let mut parent: Option<String> = None;
    for _ in 0..count {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "a".repeat(chars), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    store
}

/// The gate's ladder, rung 1 (#338): the buffered chunks promote — the
/// compaction that costs no LLM call — the re-priced prompt fits, and the turn
/// goes out on its one main call: the observer is never asked.
#[tokio::test]
async fn rung_one_fits_the_prompt_by_promoting_without_an_llm_call() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 10, 3_200);
    // Two 5000-token chunks over 8000 pending with a 230-token retention
    // floor: the projection takes the first, whose range ends at the last raw
    // entry, so promoting it empties the raw window.
    let mut om = state(1_000, crate::om::OmRecord::default());
    om.record.pending_tokens = 8_000;
    let chunk = |first: &str, last: &str| crate::om_integration::BufferedChunk {
        range: (first.into(), last.into()),
        last_ts: 100,
        text: "promoted chunk".into(),
        tokens: 5_000,
    };
    om.buffered = vec![chunk("00000001", "00000010"), chunk("00000011", "00000011")];
    om.record.buffered_chunks = om.buffered.clone();
    let provider = OmScript::scripted(Vec::new());
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    // The promote fitted the prompt, so rung 2 (the observer) never ran: a
    // re-pricing is earned by compaction, not by a rung running.
    assert_eq!(
        provider.om_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "rung 1 costs no LLM call"
    );
    assert_eq!(
        provider
            .main_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the turn went out once"
    );
    let om = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert_eq!(
        om.record.cursor.as_ref().map(|c| c.entry_id.as_str()),
        Some("00000010"),
        "rung 1 promoted the first chunk"
    );
    assert!(om.record.active_observations.contains("promoted chunk"));
}

/// Rung 1 negative (#338): nothing is buffered, so promotion compacts nothing
/// and earns no re-pricing; the ladder climbs, every compaction round-trip
/// dies on the dead route, and the prompt is refused pre-wire. Each stage that
/// died is recorded, once.
#[tokio::test]
async fn a_rung_that_compacts_nothing_earns_no_repricing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 10, 3_200);
    let om = state(1_000, crate::om::OmRecord::default());
    let provider = OmScript::dead();
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    let err = agent
        .process()
        .await
        .expect_err("compaction that shrinks nothing leaves the prompt over budget");
    assert!(
        matches!(&err, AgentError::ContextOverflow { prompt, budget } if *prompt > *budget),
        "expected ContextOverflow, got {err:?}"
    );
    assert_eq!(
        provider
            .main_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the refusal is pre-wire"
    );
    assert_eq!(
        provider.om_calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "observe rung, then reflect rung: {:?}",
        provider.om_bodies.lock().len()
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(
        om_failed_stages(&reopened),
        vec!["observe".to_owned(), "reflect".to_owned()],
        "each stage that died is recorded, and only once"
    );
}

/// The gate's ladder, rung 3 (#338): the reflect rung rewrites the managed
/// suffix and the gate demotes the frozen prefix; the prompt is re-priced from
/// that state, the demoted prefix leaves the request, and the turn goes out.
#[tokio::test]
async fn rung_three_demotes_the_frozen_prefix_and_the_prompt_fits() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 1, 40);
    let mut om = state(
        100_000,
        crate::om::OmRecord {
            frozen_prefix: log(1, 3_000),
            active_observations: log(1, 500),
            observation_tokens: 500,
            ..Default::default()
        },
    );
    // The cursor sits on the branch's last entry: the prompt is the log, and
    // the log is mostly the frozen prefix — the material rung 3 exists for.
    om.record.cursor = Some(crate::om::Cursor {
        entry_id: "00000001".into(),
        timestamp: 100,
    });
    let provider = OmScript::scripted(vec!["condensed the suffix".into()]);
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    assert_eq!(
        provider.om_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the reflect rung ran, once"
    );
    assert_eq!(
        provider
            .main_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the re-priced prompt fitted"
    );
    // The reflect request carried the frozen prefix in view (ADR-0004).
    assert!(
        provider.om_bodies.lock()[0].contains("<frozen-prefix>"),
        "rung 3 reflects on the frozen variant"
    );
    let om = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert!(om.record.prefix_demoted, "rung 3 demoted the prefix");
    assert!(om.record.active_observations.contains("condensed"));
}

/// Rung 3 negative (#338): the reflection is accepted (it shrinks the log) and
/// the demotion runs, and the re-priced prompt is still over budget — the
/// ladder ends in the durable pre-wire refusal, and no main call goes out.
#[tokio::test]
async fn the_ladder_refuses_when_rung_three_leaves_the_prompt_over_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 1, 40);
    let mut om = state(
        100_000,
        crate::om::OmRecord {
            active_observations: log(2, 2_500),
            observation_tokens: 5_000,
            ..Default::default()
        },
    );
    om.record.cursor = Some(crate::om::Cursor {
        entry_id: "00000001".into(),
        timestamp: 100,
    });
    // Shorter than the source it replaces, so `validate_compression` takes it,
    // and still far too long for the 7904-token budget.
    let provider = OmScript::scripted(vec![bulk(2_200)]);
    let agent = agent_on(dir.path(), store, provider.clone(), om);
    agent.send("go", Lane::FollowUp);
    let err = agent
        .process()
        .await
        .expect_err("a prompt every rung leaves too large is refused");
    assert!(
        matches!(&err, AgentError::ContextOverflow { prompt, budget } if *prompt > *budget),
        "expected ContextOverflow, got {err:?}"
    );
    let om = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    assert!(
        om.record.active_observations.contains("item 0 covers"),
        "the rung's compaction ran and committed"
    );
    assert_eq!(
        provider
            .main_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the refusal is pre-wire"
    );
    assert_eq!(
        provider.om_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one reflect, then the refusal"
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert!(
        entries_of(&reopened).iter().any(|e| {
            e.kind == EntryKind::Diagnostic
                && e.payload["prompt"].as_u64().unwrap_or_default()
                    > e.payload["budget"].as_u64().unwrap_or_default()
        }),
        "the refusal is durable"
    );
}

/// The `om_failed` positive (#338): two turns, each with a turn-end OM pass
/// whose round-trip exhausts on the dead route — the failure appends exactly
/// one durable record naming the stage, and kills neither turn (#86).
#[tokio::test]
async fn an_exhausted_om_round_trip_records_one_om_failed_naming_the_stage() {
    let dir = tempfile::tempdir().unwrap();
    // 5 chained 900-char entries: ~1125 tokens of raw window, over the
    // 1000-token activation threshold and under the 7904 budget, so the
    // turn-end pass plans an observe and the gate never fires.
    let store = store_with_raw(dir.path(), 5, 900);
    let provider = OmScript::dead();
    let agent = agent_on(
        dir.path(),
        store,
        provider.clone(),
        state(1_000, crate::om::OmRecord::default()),
    );
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    assert_eq!(
        provider
            .main_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "a failed pass kills no turn (#86)"
    );
    assert_eq!(
        provider.om_calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "both turns' passes round-tripped"
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(
        om_failed_stages(&reopened),
        vec!["observe".to_owned()],
        "the #306 latch: a dead route fails every pass, one record names it"
    );
}

/// The `om_failed` negatives (#338): a pass that completes records nothing.
#[tokio::test]
async fn a_healthy_om_pass_records_no_om_failed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_raw(dir.path(), 5, 900);
    let provider = OmScript::scripted(vec!["did the work".into()]);
    let agent = agent_on(
        dir.path(),
        store,
        provider.clone(),
        state(1_000, crate::om::OmRecord::default()),
    );
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    assert_eq!(
        provider.om_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "turn 1's observe committed and advanced the cursor, so turn 2 crosses no threshold: {:?}",
        provider
            .om_bodies
            .lock()
            .iter()
            .map(|b| b.chars().take(40).collect::<String>())
            .collect::<Vec<_>>()
    );
    let mut reopened = SessionStore::for_workspace(dir.path(), "s1");
    reopened.open().unwrap();
    assert_eq!(
        om_failed_stages(&reopened),
        Vec::<String>::new(),
        "a pass that ran files no failure"
    );
}
