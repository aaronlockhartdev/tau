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
        ..Default::default()
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
    assert_eq!(fork.live_observations(), "frozenlive");
}

#[test]
fn a_record_over_the_blob_threshold_round_trips() {
    // A record past the 100 KB blob threshold stores a null inline
    // payload + zstd sidecar; the readback must resolve the sidecar,
    // not fall back to an empty record (silent OM state loss on every
    // reopen — the long-observation-log case).
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();

    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.active_observations = "obs ".repeat(30_000); // 120 KB > 100 KB
    state.record.cursor = Some(Cursor {
        entry_id: "00000001".into(),
        timestamp: 5,
    });
    state.save(&mut store).unwrap();

    // The test must exercise the blob path: the newest om entry is
    // stored with a null inline payload and a sidecar reference.
    let om_entries: Vec<_> = store
        .entries_range(0, usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == KIND_OM)
        .collect();
    let newest = om_entries.last().unwrap();
    assert!(
        newest.payload.is_null(),
        "expected the blob path, got an inline payload"
    );
    assert!(newest.blob.is_some());

    let reloaded = OmState::load_record(&mut store).unwrap();
    assert_eq!(
        reloaded.active_observations,
        state.record.active_observations
    );
    assert_eq!(reloaded.cursor, state.record.cursor);
}

#[test]
fn a_corrupt_duplicate_id_cycle_terminates() {
    // A duplicate id makes the parent chain cyclic; the branch walk must
    // terminate, not clone entries forever (the 80GB runaway's amplifier).
    let a: Entry = serde_json::from_str(
        r#"{"id":"00000001","parentId":"00000002","timestamp":1,"type":"user","payload":{"text":"a"}}"#,
    )
    .unwrap();
    let b: Entry = serde_json::from_str(
        r#"{"id":"00000002","parentId":"00000001","timestamp":2,"type":"user","payload":{"text":"b"}}"#,
    )
    .unwrap();
    let entries = vec![a, b];
    let branch = branch_entries(&entries, Some("00000001"));
    assert!(
        branch.len() <= entries.len(),
        "the walk escaped the entry count: {} entries",
        branch.len()
    );
}
