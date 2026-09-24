use super::*;

#[test]
fn plan_picks_observe_at_activation_and_commit_advances_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 3, 100);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.pending_tokens = state.config.observe_threshold;
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Observe { .. } => {}
        other => panic!("expected Observe, got {other:?}"),
    }
    let result = turn_result("<observations>user is setting up a workbench</observations>");
    let mut action = TurnEndAction::Observe {
        transcript: String::new(),
    };
    state.commit(&mut store, &mut action, &result).unwrap();
    assert_eq!(action, TurnEndAction::Done);
    let cursor = state.record.cursor.clone().unwrap();
    assert_eq!(cursor.entry_id, "00000003");
    let groups = crate::om::parse_observation_groups(&state.record.active_observations);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].range, "00000000:00000003");
    assert!(groups[0].content.contains("workbench"));
    assert_eq!(state.record.pending_tokens, 0);
    // The observation persists across a re-load from the session file.
    assert!(
        OmState::load_record(&mut store)
            .unwrap()
            .active_observations
            .contains("workbench")
    );
}

#[test]
fn plan_buffers_below_activation_and_commit_holds_the_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 25, 1000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    // 25k tokens: below the 30k threshold, past the 6k increment.
    state.record.pending_tokens = 25_000;
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Buffer { .. } => {}
        other => panic!("expected Buffer, got {other:?}"),
    }
    let result = turn_result("<observations>chunk</observations>");
    let mut action = TurnEndAction::Buffer {
        transcript: String::new(),
    };
    state.commit(&mut store, &mut action, &result).unwrap();
    assert_eq!(state.buffered.len(), 1);
    assert!(state.record.active_observations.is_empty());
    assert!(state.record.cursor.is_none());
}

#[test]
fn plan_reflects_at_the_observation_threshold_and_commit_rewrites_the_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 1, 100);
    let prefix = "FROZEN PARENT LOG".to_owned();
    let suffix =
        crate::om::wrap_in_observation_group(&"x".repeat(4000), "00000001:00000002", "b", None);
    let mut state = OmState::from_config(
        &crate::config::Om::default(),
        OmRecord {
            frozen_prefix: prefix.clone(),
            active_observations: suffix,
            observation_tokens: crate::config::Om::default().reflect_threshold as u32,
            ..Default::default()
        },
    );
    match state.plan(&[]) {
        TurnEndAction::Reflect { level } => assert_eq!(level, 0),
        other => panic!("expected Reflect, got {other:?}"),
    }
    // A realistic TAGGED reflection: only the <observations> content
    // may land in the log — the other sections are not observation
    // material (B1).
    let tagged = "<observations>condensed suffix</observations>".to_owned()
        + "\n<current-task>finish the refactor</current-task>"
        + "\n<suggested-response>report the summary</suggested-response>";
    let result = turn_result(&tagged);
    let mut action = TurnEndAction::Reflect { level: 0 };
    state.commit(&mut store, &mut action, &result).unwrap();
    assert_eq!(action, TurnEndAction::Done);
    assert_eq!(state.record.frozen_prefix, prefix);
    assert_eq!(state.record.generation, 1);
    let suffix = &state.record.active_observations;
    assert!(suffix.contains("condensed suffix"), "{suffix:?}");
    assert!(!suffix.contains("<observations>"), "{suffix:?}");
    assert!(!suffix.contains("<current-task>"), "{suffix:?}");
    assert!(!suffix.contains("<suggested-response>"), "{suffix:?}");
    assert!(!suffix.contains("finish the refactor"), "{suffix:?}");
    assert!(!suffix.contains("report the summary"), "{suffix:?}");
    assert_eq!(
        state.record.live_observations(),
        format!("{}{}", prefix, state.record.active_observations)
    );
}

#[test]
fn commit_escalates_a_rejected_reflection_level() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 1, 100);
    let mut state = OmState::from_config(
        &crate::config::Om::default(),
        OmRecord {
            active_observations: "y".repeat(400),
            ..Default::default()
        },
    );
    let result = turn_result(&"z".repeat(800)); // bigger than the source
    let mut action = TurnEndAction::Reflect { level: 0 };
    state.commit(&mut store, &mut action, &result).unwrap();
    match action {
        TurnEndAction::Reflect { level } => assert_eq!(level, 1),
        other => panic!("expected escalated Reflect, got {other:?}"),
    }
    assert_eq!(state.record.generation, 0);
}

#[test]
fn commit_keeps_the_cursor_on_degenerate_observation_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 3, 100);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    // 100 identical 30-char lines (over 2k chars, >50% duplicates) —
    // the ported degenerate detector rejects this shape.
    let body = (0..100)
        .map(|_| "the same observation line")
        .collect::<Vec<_>>()
        .join("\n");
    let result = turn_result(&format!("<observations>{body}</observations>"));
    let mut action = TurnEndAction::Observe {
        transcript: String::new(),
    };
    state.commit(&mut store, &mut action, &result).unwrap();
    assert_eq!(action, TurnEndAction::Done);
    assert!(state.record.cursor.is_none());
    assert!(state.record.active_observations.is_empty());
}

#[test]
fn the_overflow_ladder_demotes_the_prefix_and_readmits_it() {
    let mut state = OmState {
        config: OmConfig {
            reflect_threshold: 100,
            ..Default::default()
        },
        record: OmRecord {
            frozen_prefix: "a".repeat(400),       // 100 tokens
            active_observations: "b".repeat(400), // 100 tokens
            ..Default::default()
        },
        buffered: Vec::new(),
        buffer_cursor: None,
        changed: false,
    };
    // 200 combined tokens over the 100 budget: the prefix demotes out of
    // the live context (it stays in the record, recall reaches it).
    state.maintain_prefix_budget();
    assert!(state.record.prefix_demoted);
    assert_eq!(state.record.live_observations(), "b".repeat(400));
    // The suffix falls under the budget: the prefix comes back.
    state.record.active_observations = "b".repeat(40);
    state.record.observation_tokens = 10;
    state.maintain_prefix_budget();
    assert!(!state.record.prefix_demoted);
    assert_eq!(
        state.record.live_observations(),
        format!("{}{}", "a".repeat(400), "b".repeat(40))
    );
}

#[test]
fn the_frozen_prompt_carries_the_marker_and_the_suffix_only() {
    let prompt = crate::om::build_reflector_prompt_frozen("FROZEN", "LIVE SUFFIX", 2);
    assert!(prompt.contains("<frozen-prefix>\nFROZEN\n</frozen-prefix>"));
    assert!(prompt.contains("byte-verbatim"));
    assert!(prompt.contains("LIVE SUFFIX"));
    // The compression guidance for level 2 is appended.
    assert!(prompt.contains(crate::om::COMPRESSION_GUIDANCE[1]));
}
