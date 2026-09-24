use super::*;

#[test]
fn config_fold_maps_increment_to_activation() {
    let om = crate::config::Om::default(); // 30k / 40k / 6k
    let state = OmState::from_config(&om, OmRecord::default());
    assert_eq!(state.config.observe_threshold, 30_000);
    assert_eq!(state.config.reflect_threshold, 40_000);
    assert!((state.config.buffer_activation - 0.8).abs() < 1e-9);
    assert_eq!(state.config.buffer_increment(), 6_000);
}

#[test]
fn record_round_trips_through_the_session_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();

    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.active_observations = "first".into();
    state.save(&mut store).unwrap();
    assert_eq!(
        OmState::load_record(&mut store)
            .unwrap()
            .active_observations,
        "first"
    );

    state.record.active_observations = "second".into();
    state.save(&mut store).unwrap();
    // The newest entry is the current state.
    assert_eq!(
        OmState::load_record(&mut store)
            .unwrap()
            .active_observations,
        "second"
    );
}

#[test]
fn load_without_any_record_is_default() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    let record = OmState::load_record(&mut store).unwrap();
    assert!(record.frozen_prefix.is_empty());
    assert!(record.active_observations.is_empty());
    assert!(record.cursor.is_none());
    assert_eq!(record.generation, 0);
}

#[test]
fn fork_copies_observations_and_cursor_without_a_prefix() {
    let parent = OmRecord {
        frozen_prefix: "frozen".into(),
        active_observations: "live".into(),
        cursor: Some(Cursor {
            entry_id: "00000005".into(),
            timestamp: 7,
        }),
        generation: 3,
        observation_tokens: 42,
        pending_tokens: 9,
        prefix_demoted: false,
    };
    let fork = fork_record(&parent);
    assert!(fork.frozen_prefix.is_empty());
    assert_eq!(fork.active_observations, "frozenlive");
    assert_eq!(fork.cursor, parent.cursor);
    assert_eq!(fork.generation, 3);
    // A demoted prefix is owned (not borrowed) in the fork: copied
    // verbatim and undemoted.
    let demoted = OmRecord {
        prefix_demoted: true,
        ..parent
    };
    let fork = fork_record(&demoted);
    assert_eq!(fork.active_observations, "frozenlive");
    assert!(!fork.prefix_demoted);
    assert!(fork.live_observations() == "frozenlive");
}
