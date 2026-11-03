//! The turn-end sequence (R4) driven through `settle_turn` — the actual
//! turn-end entry point, not just `session::branch`.

use super::*;

#[tokio::test]
async fn settle_turn_terminates_on_a_cyclic_chain() {
    // Duplicate ids make the parent chain cyclic (corruption): the branch
    // walk inside settle_turn must terminate, not clone entries forever
    // (the 80 GB runaway's amplifier).
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    let mut a: Entry = serde_json::from_str(
        r#"{"id":"00000001","parentId":"00000002","timestamp":1,"type":"user","payload":{"text":"a"}}"#,
    )
    .unwrap();
    let mut b: Entry = serde_json::from_str(
        r#"{"id":"00000002","parentId":"00000001","timestamp":2,"type":"user","payload":{"text":"b"}}"#,
    )
    .unwrap();
    a.crc = Some(a.compute_crc());
    b.crc = Some(b.compute_crc());
    let header = std::fs::read_to_string(store.path()).unwrap();
    std::fs::write(
        store.path(),
        format!(
            "{}{}\n{}\n",
            header,
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        ),
    )
    .unwrap();
    // The file is valid (CRCs check) and cyclic (a ⇄ b).
    store.open().unwrap();

    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let provider = crate::provider::canned("the observer never runs at this size");
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    // Terminates with the default record (nothing to observe) — the point
    // is that the walk ends, not what the plan picks.
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn settle_turn_runs_the_observe_pass_and_commits() {
    let dir = tempfile::tempdir().unwrap();
    // ~50k chars per entry: the unobserved raw clears the observe
    // threshold (30k tokens), so the pass plans an Observe.
    let mut store = store_with_text_entries(dir.path(), 3, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let provider = crate::provider::canned(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"<observations>user is setting up a workbench</observations>\"}\n\ndata: [DONE]\n\n",
    );
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
    // The observation is in the record, the cursor advanced, and the
    // record persisted to the session file. The cursor lands on the bounded
    // span, not the window's end: 3 × 12.5k tokens over a 30k bound admits two
    // entries, and the third is left for the next settle (#394).
    assert!(state.record.active_observations.contains("workbench"));
    assert_eq!(state.record.cursor.as_ref().unwrap().entry_id, "00000002");
    assert_eq!(state.record.pending_tokens, 12_500);
    assert!(
        OmState::load_record(&mut store)
            .unwrap()
            .active_observations
            .contains("workbench")
    );
}

/// OM bloat (divergence A): the durable record must carry no raw observer
/// input. mastra's `ObservationalMemoryRecord` stores only the compact
/// observation *output* (and token counts) — never the raw input. tau once
/// persisted the full 777 KB observer transcript as a record field (the 835 KB
/// blob in the 2026-10-06 bench run); that field is removed, so the serialized
/// record has no `om_input` at all.
#[tokio::test]
async fn observe_does_not_persist_the_raw_transcript_to_the_record() {
    let dir = tempfile::tempdir().unwrap();
    // 3 × 50k chars ≈ 37.5k tokens: clears the 30k observe threshold, so the
    // pass plans an Observe over the full 150k-char unobserved window.
    let mut store = store_with_text_entries(dir.path(), 3, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let provider = crate::provider::canned(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"<observations>user is setting up a workbench</observations>\"}\n\ndata: [DONE]\n\n",
    );
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();

    // Non-vacuity guard: the observe ran and committed an observation…
    assert!(state.record.active_observations.contains("workbench"));
    // …but the raw 150k-char input must NOT be in the durable record.
    let serialized = serde_json::to_string(&state.record).unwrap();
    assert!(
        !serialized.contains("\"om_input\""),
        "the raw observer input must not be a field of the durable record (mastra stores only the observation output)"
    );
}

/// OM bloat (divergence A): the durable record carries no raw observer
/// reasoning either — mastra stores no raw reasoning in the record, and tau's
/// `om_thinking` field is removed, so the serialized record has no
/// `om_thinking` at all.
#[tokio::test]
async fn observe_does_not_persist_the_raw_reasoning_to_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 3, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    // A large, distinctive reasoning block plus a tiny observation.
    let reasoning = "R".repeat(20_000);
    let provider = crate::provider::canned(&format!(
        "data: {{\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"{reasoning}\"}}\n\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"<observations>tiny</observations>\"}}\n\ndata: [DONE]\n\n"
    ));
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();

    assert!(state.record.active_observations.contains("tiny"));
    let serialized = serde_json::to_string(&state.record).unwrap();
    assert!(
        !serialized.contains("\"om_thinking\""),
        "the raw observer reasoning must not be a field of the durable record"
    );
}

/// OM bloat, mastra parity (divergence A): the serialized durable record must
/// be bounded by the observation *output*, not the raw input. Observing a
/// 150k-char window that yields a ~30-char observation must not produce a
/// 150k-char record.
#[tokio::test]
async fn record_size_tracks_the_observation_not_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 3, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let provider = crate::provider::canned(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"<observations>user is setting up a workbench</observations>\"}\n\ndata: [DONE]\n\n",
    );
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();

    let serialized = serde_json::to_string(&state.record).unwrap();
    assert!(
        serialized.len() < 5_000,
        "durable record is {} chars for a {}-char observation — dominated by the persisted raw input, not the observation",
        serialized.len(),
        state.record.active_observations.len()
    );
}

/// A buffered chunk over the entries `range.0..=range.1`, as a cycle's
/// commit leaves it.
fn chunk_covering(range: (String, String), last_ts: u64, text: &str, tokens: u32) -> BufferedChunk {
    BufferedChunk {
        range,
        last_ts,
        text: text.into(),
        tokens,
    }
}

fn append_entries(store: &mut SessionStore, n: usize, chars: usize) {
    let leaf = store.leaf().unwrap().map(|e| e.id);
    let mut parent = leaf;
    for _ in 0..n {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "w".repeat(chars), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
}

#[tokio::test]
async fn settle_promotes_the_chunks_then_observes_the_raw_tail_at_activation() {
    // #392: activation with chunks pending runs the rung that costs no LLM
    // call first (the promote), and the plan's gate then reads the raw window
    // that remains — so a tail past the promoted chunks is observed in the
    // same settle, over the tail alone.
    let dir = tempfile::tempdir().unwrap();
    // 26 × 8k chars ≈ 52k tokens, past the 30k activation.
    let mut store = store_with_text_entries(dir.path(), 26, 8_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let head = chunk_covering(
        ("00000001".into(), "00000006".into()),
        100,
        "observed the head",
        120,
    );
    state.buffered = vec![head.clone()];
    state.record.buffered_chunks = vec![head];
    state.buffer_cursor = Some("00000006".into());
    let recorder = std::sync::Arc::new(RecordingProvider::new("observed the tail"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();

    // The free rung: the chunk's text is in the log.
    assert!(
        state
            .record
            .active_observations
            .contains("observed the head")
    );
    // The gate on the raw volume that remains (20 entries, ~40k tokens): the
    // The span is the bound's slice, not the whole tail (#394): 15 of the 20
    // tail entries (30k of the 40k), cursor on entry 21, the rest for the next
    // settle.
    assert!(
        state
            .record
            .active_observations
            .contains("observed the tail"),
        "the tail past the promoted chunks stayed unobserved: {:?}",
        state.record.active_observations
    );
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000021");
    assert!(state.buffered.is_empty());
    // The Observer's transcript is the tail, not the window the chunks cover,
    // and the tail's slice, not the whole tail (#394).
    assert_eq!(recorder.observer_calls(), 1);
    assert_eq!(
        recorder.observer_transcripts()[0]
            .matches(&"w".repeat(8_000))
            .count(),
        15
    );
}

#[tokio::test]
async fn settle_below_activation_promotes_nothing_and_calls_no_observer() {
    // The negative pair: a window under activation with chunks pending leaves
    // the chunks buffered, the cursor where it was, and no Observer call. The
    // stamped quantity is the raw window, not the chunks' tokens.
    let dir = tempfile::tempdir().unwrap();
    // 3 × 8k chars ≈ 6k tokens, under the 30k activation.
    let mut store = store_with_text_entries(dir.path(), 3, 8_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let head = chunk_covering(
        ("00000001".into(), "00000002".into()),
        100,
        "observed the head",
        120,
    );
    state.buffered = vec![head.clone()];
    state.record.buffered_chunks = vec![head];
    let recorder = std::sync::Arc::new(RecordingProvider::new("observed the tail"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();

    assert_eq!(recorder.observer_calls(), 0);
    assert!(state.record.cursor.is_none());
    assert_eq!(state.buffered.len(), 1);
    let raw = state.unobserved_tokens(&mut store).unwrap();
    assert!(matches!(raw, 6_000..30_000), "raw window {raw}");
    assert_eq!(state.record.pending_tokens, raw);
}

#[tokio::test]
async fn continuous_settles_activate_on_tokens_alone() {
    // #392: continuous turns reach activation on tokens, with no idle gap.
    // Cycles are always on (#408): a settle below activation does nothing,
    // so the window accumulates until the threshold makes the settle
    // observe.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 0, 0);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let recorder = std::sync::Arc::new(RecordingProvider::new("observed the round"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    let mut activations = 0;
    for round in 0..20 {
        append_entries(&mut store, 3, 8_000); // ~6k tokens per round
        let raw_before = state.unobserved_tokens(&mut store).unwrap();
        let cursor_before = state.record.cursor.clone();
        let mut with_store =
            |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
        state
            .settle_turn(&mut with_store, &provider, "om-model", None)
            .await
            .unwrap();
        let raw_after = state.unobserved_tokens(&mut store).unwrap();
        // One quantity: what the settle stamps is the window its gate read, so
        // the stall diagnostic beside it cannot name a number the gate missed.
        assert_eq!(
            state.record.pending_tokens, raw_after,
            "round {round} stamped the buffered chunks, not the raw window"
        );
        if state.activation_reached(raw_before) {
            activations += 1;
            assert!(
                state.record.cursor.is_some() && state.record.cursor != cursor_before,
                "round {round}: activation at {raw_before} tokens moved no cursor",
            );
            assert!(
                raw_after < raw_before,
                "round {round}: activation left the {raw_before}-token window whole",
            );
        }
    }
    assert!(activations > 0, "no round ever reached activation");
    assert!(recorder.observer_calls() > 0, "the Observer never ran");
    assert!(state.record.observation_tokens > 0);
    // The rounds ran back to back: no settle here had the idle escape.
    let all = store.entries_range(0, usize::MAX).unwrap();
    let leaf = store.leaf().unwrap().map(|e| e.id);
    assert!(
        crate::om_integration::idle_gap_secs(&all, leaf.as_deref())
            < crate::om_integration::IDLE_ACTIVATION_SECS,
        "the test's own premise: the rounds sat idle"
    );
}

#[tokio::test]
async fn settles_observe_bounded_slices_never_the_whole_window() {
    // #408: every paid Observer call a settle makes is a bounded slice. The
    // sync Buffer arm is gone — the sub-activation band belongs to the
    // background cycles — so what a settle may pay for is the activation
    // backstop: threshold-sized, never the whole window. 28 × 2_000 chars
    // ≈ 14k raw; the threshold sits at 6k. Two settles observe two 6k
    // slices; the third finds 2k left under activation and calls nothing.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 28, 2_000);
    let om = crate::config::Om {
        observe_threshold: 6_000,
        ..crate::config::Om::default()
    };
    let mut state = OmState::from_config(&om, OmRecord::default());
    let recorder = std::sync::Arc::new(RecordingProvider::new("observed the slice"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    for _ in 0..4 {
        let mut with_store =
            |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
        state
            .settle_turn(&mut with_store, &provider, "om-model", None)
            .await
            .unwrap();
    }
    // Two 6k slices and a third settle with 2k under activation: no call
    // ever carried more than a slice — the whole-window request is gone.
    assert_eq!(recorder.observer_calls(), 2, "the settle count is off");
    let spans: Vec<usize> = recorder
        .observer_transcripts()
        .iter()
        .map(|seen| seen.matches(&"w".repeat(2_000)).count())
        .collect();
    assert_eq!(spans, vec![12, 12], "a call carried more than a slice");
    // The backstop advances the observation cursor as it goes; the gate
    // quantity it leaves behind is the raw window, and the stall beside it
    // reads the same number (#392).
    assert!(state.record.cursor.is_some());
    assert!(
        state.buffered.is_empty(),
        "the settle buffered: the sync arm is retired (#408)"
    );
    assert_eq!(state.record.pending_tokens, 2_000);
}

/// The drain fixture's config: a 1000-token activation (a 1200-token drain
/// line), a reflect rung far out of reach, and the cycles' interval small
/// enough to be irrelevant at these volumes (#409).
fn drain_config(retries: crate::config::OmRetries) -> crate::config::Om {
    crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1_000,
        reflect_threshold: 1_000_000,
        buffer_increment: 200,
        buffer_tokens: None,
        retries,
    }
}

#[tokio::test]
async fn a_backlogged_window_drains_in_bounded_slices() {
    // #409 (mastra `blockAfter` parity): a window at 1.2× activation is not
    // left for the next settle — the settle drains bounded slices, each
    // planned from the state the last slice's commit moved, until the
    // window drops under the drain line.
    let dir = tempfile::tempdir().unwrap();
    // 12 × 1600 chars = 400 tokens each: 4800 raw, past the 1200 drain line.
    let mut store = store_with_text_entries(dir.path(), 12, 1_600);
    let mut state = OmState::from_config(
        &drain_config(crate::config::OmRetries::default()),
        OmRecord::default(),
    );
    let recorder = std::sync::Arc::new(RecordingProvider::new("drained a slice"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
    // Each slice is bound-sized — two entries (800 tokens; a third would
    // cross the 1000 bound) — and the cursor rides the shown span: five
    // slices drain to 800 raw, under the 1200 line.
    let transcripts = recorder.observer_transcripts();
    assert_eq!(recorder.observer_calls(), 5);
    for t in &transcripts {
        assert_eq!(t.matches(&"w".repeat(1_600)).count(), 2);
    }
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000010");
    assert_eq!(state.record.pending_tokens, 800);
    assert!(state.record.active_observations.contains("drained a slice"));
}

#[tokio::test]
async fn a_window_under_the_drain_line_pays_one_slice() {
    // The negative pair: between activation (1000) and the drain line
    // (1200) the settle pays its single backstop slice and stops — the
    // async cycles own that band; the valve adds no new blocking in the
    // healthy path.
    let dir = tempfile::tempdir().unwrap();
    // 11 × 400 chars = 100 tokens each: 1100 raw — past activation, under
    // the drain line.
    let mut store = store_with_text_entries(dir.path(), 11, 400);
    let mut state = OmState::from_config(
        &drain_config(crate::config::OmRetries::default()),
        OmRecord::default(),
    );
    let recorder = std::sync::Arc::new(RecordingProvider::new("one backstop slice"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
    assert_eq!(recorder.observer_calls(), 1);
    // The bound is inclusive: ten entries ride to exactly 1000.
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000010");
}

#[tokio::test]
async fn a_drain_slice_that_fails_costs_the_retry_budget_and_returns() {
    // Lag blocks the settle, death never loops it (#409): a dead endpoint
    // costs the observe retry budget — attempts, not slices — the cursor
    // stays frozen, and the failure reaches the caller, which latches
    // `om_failed` and lets the turn through (#86).
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 12, 1_600);
    let mut state = OmState::from_config(
        &drain_config(crate::config::OmRetries {
            observe: 2,
            buffer: 0,
            reflect: 0,
        }),
        OmRecord::default(),
    );
    let failing = std::sync::Arc::new(FailingProvider::new());
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = failing.clone();
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    let outcome = state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await;
    assert!(matches!(
        outcome,
        Err(OmError::Provider(
            OmStage::Observe,
            crate::provider::ProviderError::Status { status: 500, .. }
        ))
    ));
    assert_eq!(
        failing.calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "the drain paid past the retry budget"
    );
    assert!(state.record.cursor.is_none());
}

#[tokio::test]
async fn a_degenerate_slice_stops_the_drain() {
    // The observer answers, the answer observes nothing: the cursor does not
    // move, and the settle never pays twice for one frozen span — the drain
    // stops on no-progress, the window stays pending for the cycles and the
    // next settle.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 12, 1_600);
    let mut state = OmState::from_config(
        &drain_config(crate::config::OmRetries::default()),
        OmRecord::default(),
    );
    let recorder = std::sync::Arc::new(RecordingProvider::new(""));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
    assert_eq!(recorder.observer_calls(), 1);
    assert!(state.record.cursor.is_none());
}
