use super::*;

/// A settle of `state` over `store`, the way the agent loop calls it.
async fn settle(
    state: &mut OmState,
    store: &mut SessionStore,
    provider: &std::sync::Arc<dyn crate::provider::TurnProvider>,
) {
    let mut with_store = |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(store);
    state
        .settle_turn(&mut with_store, provider, "om-model", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_sunken_session_pays_its_window_a_bounded_slice_per_settle() {
    // #394: a sunken session (cursor frozen at the branch head, no chunks
    // buffered) clears activation with a window the size of its whole history.
    // The first settle observes a threshold-sized slice and leaves the tail; the
    // next settle takes the next slice. The window is paid a slice per settle,
    // never handed to the Observer whole.
    let dir = tempfile::tempdir().unwrap();
    // 20 × 16k chars ≈ 80k tokens: past the 30k activation, and past the
    // threshold-sized span the bound names.
    let mut store = store_with_text_entries(dir.path(), 20, 16_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let recorder = std::sync::Arc::new(RecordingProvider::new("observed a slice"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();

    settle(&mut state, &mut store, &provider).await;
    // Each entry is 4k tokens: the 30k bound admits 7 (28k), an eighth would
    // carry the span to 32k. The Observer is shown 7 of the 20 entries.
    assert_eq!(
        recorder.observer_transcripts()[0]
            .matches(&"w".repeat(16_000))
            .count(),
        7,
        "the first settle's span is not the threshold-sized slice the bound names"
    );
    assert_eq!(state.record.cursor.as_ref().unwrap().entry_id, "00000007");
    let raw_after_first = state.unobserved_tokens(&mut store).unwrap();
    assert_eq!(raw_after_first, 52_000);
    // The stamped quantity is the window the cursor left behind, never 0 for a
    // span that stopped short (#392's gate quantity).
    assert_eq!(state.record.pending_tokens, raw_after_first);

    // The tail is observed by the next settle, not lost to the bound.
    settle(&mut state, &mut store, &provider).await;
    assert_eq!(
        recorder.observer_transcripts()[1]
            .matches(&"w".repeat(16_000))
            .count(),
        7
    );
    assert_eq!(state.record.cursor.as_ref().unwrap().entry_id, "00000014");
    assert_eq!(recorder.observer_calls(), 2);

    // The spans are consecutive and disjoint: the slices tile the window, so the
    // bound costs no coverage — it spreads it over settles.
    let groups = crate::om::parse_observation_groups(&state.record.active_observations);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].range, "00000001:00000007");
    assert_eq!(groups[1].range, "00000008:00000014");
}

#[tokio::test]
async fn a_window_at_the_threshold_is_observed_whole() {
    // The negative pair: a window that clears activation without passing the
    // bound is observed in full, cursor on the last entry, window empty. The
    // bound caps an over-threshold window; it does not truncate an ordinary one.
    let dir = tempfile::tempdir().unwrap();
    // 4 × 8k chars ≈ 8k tokens, and the threshold is set to that volume, so the
    // window reaches activation exactly at the bound.
    let mut store = store_with_text_entries(dir.path(), 4, 8_000);
    let om = crate::config::Om {
        observe_threshold: 8_000,
        ..crate::config::Om::default()
    };
    let mut state = OmState::from_config(&om, OmRecord::default());
    let recorder = std::sync::Arc::new(RecordingProvider::new("observed the window"));
    let provider: std::sync::Arc<dyn crate::provider::TurnProvider> = recorder.clone();
    settle(&mut state, &mut store, &provider).await;

    assert_eq!(
        recorder.observer_transcripts()[0]
            .matches(&"w".repeat(8_000))
            .count(),
        4
    );
    assert_eq!(state.record.cursor.as_ref().unwrap().entry_id, "00000004");
    assert_eq!(state.record.pending_tokens, 0);
}

#[test]
fn a_span_the_observer_saw_is_the_span_the_commit_stamps() {
    // The pair the bound depends on: `plan` hands the commit the span it
    // transcribed, so the cursor cannot land on material the Observer never saw.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 20, 16_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let unobserved = state.unobserved(&mut store).unwrap();
    let action = state.plan(&unobserved);
    let TurnEndAction::Observe { transcript, span } = &action else {
        panic!("expected Observe, got {action:?}");
    };
    // The transcript is the span's entries and nothing else: the window carries
    // 20 entries, the transcript 7.
    assert_eq!(
        transcript.matches(&"w".repeat(16_000)).count(),
        7,
        "the transcript is not the span the bound names"
    );
    assert_eq!(
        crate::om_integration::transcript(&unobserved)
            .matches(&"w".repeat(16_000))
            .count(),
        20
    );
    assert_eq!(span.first_id, "00000001");
    assert_eq!(span.last_id, "00000007");
}

#[test]
fn the_span_never_shrinks_below_the_first_entry() {
    // The bound's floor: a window whose first entry alone is past the bound is
    // observed by that entry, never by nothing — a settle at activation always
    // advances the cursor.
    let dir = tempfile::tempdir().unwrap();
    // One entry of 12k chars = 3k tokens: the bound is the entry's own size.
    let om = crate::config::Om {
        observe_threshold: 3_000,
        ..crate::config::Om::default()
    };
    let mut store = store_with_text_entries(dir.path(), 1, 12_000);
    let mut state = OmState::from_config(&om, OmRecord::default());
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Observe { span, .. } => {
            assert_eq!(span.first_id, "00000001");
            assert_eq!(span.last_id, "00000001");
        }
        action => panic!("expected Observe, got {action:?}"),
    }
}
