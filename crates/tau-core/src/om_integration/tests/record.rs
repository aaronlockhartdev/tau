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
        .filter(|e| e.kind == EntryKind::Om)
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

/// A record holding one buffered chunk over `1..=5`, as a session file
/// leaves it.
fn record_with_a_chunk() -> OmRecord {
    OmRecord {
        buffered_chunks: vec![BufferedChunk {
            range: ("00000001".into(), "00000005".into()),
            last_ts: 100,
            text: "observed the head".into(),
            tokens: 120,
        }],
        ..Default::default()
    }
}

#[test]
fn reopen_restores_the_buffer_cursor_from_the_durable_chunks() {
    // #392: the saved chunks mark their own end. A state that forgets the
    // cursor hands every later buffer run the whole branch — the Observer is
    // fed history the chunks already stand for.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 10, 4_000);
    let state = OmState::from_config(&crate::config::Om::default(), record_with_a_chunk());
    assert_eq!(state.buffer_cursor.as_deref(), Some("00000005"));
    let tail = state.unbuffered(&mut store).unwrap();
    assert_eq!(
        tail.len(),
        5,
        "the material past the chunks is entries 6..10, not the whole branch"
    );
    assert_eq!(tail[0].id, "00000006");
}

#[test]
fn a_state_without_chunks_buffers_from_the_branch_head() {
    // The negative pair: no durable chunks, no cursor — the first run covers
    // the branch from its head.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 10, 4_000);
    let state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    assert_eq!(state.buffer_cursor, None);
    assert_eq!(state.unbuffered(&mut store).unwrap().len(), 10);
}

#[test]
fn a_reopened_buffer_pass_covers_only_the_new_tail() {
    // The Buffer arm on a reopened session (kill-switch config, so the sync
    // arm runs): its transcript is the entries past the restored cursor, not
    // the branch the durable chunks already stand for (#392).
    let dir = tempfile::tempdir().unwrap();
    // 10 × 4k chars ≈ 10k tokens: past the 6k increment, under the 30k
    // activation.
    let mut store = store_with_text_entries(dir.path(), 10, 4_000);
    let om = crate::config::Om {
        buffer_tokens: 0,
        ..crate::config::Om::default()
    };
    let mut state = OmState::from_config(&om, record_with_a_chunk());
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Buffer { transcript } => assert_eq!(
            transcript.matches(&"w".repeat(4_000)).count(),
            5,
            "the buffer transcript re-covered material the chunks hold"
        ),
        other => panic!("expected Buffer, got {other:?}"),
    }
}

/// The same session rewound and continued: the leaf moves to entry 5, two
/// entries are appended from it, and the leaf follows the new tip (the shape
/// `Command::SessionBranch` leaves: it moves the leaf, and the appends that
/// follow are parented on the branch, which never touches `OmState`). The active
/// branch is 1..5 + the two new ids, so a cursor naming anything past 5 sits on
/// the abandoned side.
fn store_rewound_at_five(dir: &std::path::Path) -> SessionStore {
    let mut store = store_with_text_entries(dir, 10, 4_000);
    store.set_leaf("00000005").unwrap();
    let mut parent = Some("00000005".to_string());
    let mut tip = String::new();
    for _ in 0..2 {
        let entry = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "n".repeat(4_000), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        tip = entry.id.clone();
        parent = Some(entry.id);
    }
    store.set_leaf(&tip).unwrap();
    store
}

#[test]
fn a_reopen_over_a_rewound_branch_covers_only_the_new_material() {
    // #395: the chunks end at 00000009, on the side the leaf left behind. The
    // ids settle where its pass reached — a parent is minted before its child,
    // so a branch's ids rise root-to-leaf and the branch's 1..5 prefix is that
    // covered material. The whole-branch fallback re-covers the prefix; a clamp
    // to the branch tip leaves the new pair unobserved. Neither: the window is
    // the two entries past the cursor's number.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_rewound_at_five(dir.path());
    let record = OmRecord {
        buffered_chunks: vec![BufferedChunk {
            range: ("00000001".into(), "00000009".into()),
            last_ts: 100,
            text: "observed the head".into(),
            tokens: 120,
        }],
        cursor: Some(Cursor {
            entry_id: "00000009".into(),
            timestamp: 100,
        }),
        ..Default::default()
    };
    let state = OmState::from_config(&crate::config::Om::default(), record);
    assert_eq!(state.buffer_cursor.as_deref(), Some("00000009"));
    let tail = state.unbuffered(&mut store).unwrap();
    let ids: Vec<&str> = tail.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        ["00000011", "00000012"],
        "the reopened window is the branch's own material, not the prefix the chunks stand for"
    );
    assert_eq!(
        state.unobserved(&mut store).unwrap().len(),
        2,
        "the observe window agrees: the prefix the cursor covered stays covered"
    );
}

#[test]
fn a_reopen_whose_cursor_is_past_the_whole_branch_observes_nothing() {
    // The negative: the rewind left the branch entirely behind the cursor. The
    // fallback made that window the whole branch — a settled session re-observes
    // material its log already carries; the comparison makes it empty.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 4_000);
    store.set_leaf("00000002").unwrap();
    let record = OmRecord {
        cursor: Some(Cursor {
            entry_id: "00000004".into(),
            timestamp: 400,
        }),
        ..Default::default()
    };
    let state = OmState::from_config(&crate::config::Om::default(), record);
    assert!(
        state.unobserved(&mut store).unwrap().is_empty(),
        "a branch the cursor walked past is not raw material"
    );
}

#[test]
fn a_reopen_with_an_unparseable_cursor_falls_back_to_the_branch() {
    // The fallback's floor: a record this build cannot order (an id minted by
    // another scheme) observes everything rather than skipping material no
    // number can name.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_rewound_at_five(dir.path());
    let record = OmRecord {
        cursor: Some(Cursor {
            entry_id: "legacy-head".into(),
            timestamp: 100,
        }),
        ..Default::default()
    };
    let state = OmState::from_config(&crate::config::Om::default(), record);
    assert_eq!(state.unobserved(&mut store).unwrap().len(), 7);
}
