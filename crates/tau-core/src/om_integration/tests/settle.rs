//! The turn-end sequence (R4) driven through `settle_turn` — the actual
//! turn-end entry point, not just `branch_entries`.

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
    // record persisted to the session file.
    assert!(state.record.active_observations.contains("workbench"));
    assert_eq!(state.record.cursor.as_ref().unwrap().entry_id, "00000003");
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
