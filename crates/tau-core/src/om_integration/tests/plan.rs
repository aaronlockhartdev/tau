use super::*;

#[test]
fn plan_picks_observe_at_activation_and_commit_advances_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    // 3 × 50k chars ≈ 37.5k tokens: the raw window past the 30k activation.
    let mut store = store_with_text_entries(dir.path(), 3, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let unobserved = state.unobserved(&mut store).unwrap();
    // The plan's own action carries the span the commit stamps — the pair is the
    // contract, so this leg feeds `commit` what `plan` produced.
    let mut action = match state.plan(&unobserved) {
        action @ TurnEndAction::Observe { .. } => action,
        other => panic!("expected Observe, got {other:?}"),
    };
    let result = turn_result("<observations>user is setting up a workbench</observations>");
    state.commit(&mut store, &mut action, &result).unwrap();
    assert_eq!(action, TurnEndAction::Done);
    let cursor = state.record.cursor.clone().unwrap();
    // The bounded span (#394): 3 × 12.5k tokens observed up to the removal
    // target (37.5k − the 6k floor = 31.5k), which lands inside entry 3 — the
    // cursor stops on entry 2 and entry 3 is left for the next settle.
    assert_eq!(cursor.entry_id, "00000002");
    let groups = crate::om::parse_observation_groups(&state.record.active_observations);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].range, "00000001:00000002");
    assert!(groups[0].content.contains("workbench"));
    assert_eq!(state.record.pending_tokens, 12_500);
    // The observation persists across a re-load from the session file.
    assert!(
        OmState::load_record(&mut store)
            .unwrap()
            .active_observations
            .contains("workbench")
    );
}

#[test]
fn commit_persists_functional_fields_and_omits_raw_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 3, 100);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let result = crate::provider::TurnResult {
        text: "<observations>user set up a workbench</observations>\n<suggested-response>Walk through it.</suggested-response>".into(),
        reasoning: "deciding what is worth remembering".into(),
        usage: None,
        duration: None,
        outcome: crate::provider::StreamOutcome::Completed,
        calls: Vec::new(),
        blocks: Vec::new(),
        mid_stream_errors: Vec::new(),
    };
    let mut action = TurnEndAction::Observe {
        transcript: "the observed transcript".into(),
        span: ObservedSpan {
            first_id: "00000001".into(),
            last_id: "00000003".into(),
            last_ts: 3,
        },
    };
    state.commit(&mut store, &mut action, &result).unwrap();

    let entries = store.entries_range(0, usize::MAX).unwrap();
    let om = entries.iter().find(|e| e.kind == EntryKind::Om).unwrap();
    let p = &om.payload;
    // The raw observer input/reasoning must NOT be persisted (mastra parity);
    // only the functional suggested-response and the model name are.
    assert!(p.get("om_input").is_none());
    assert!(p.get("om_thinking").is_none());
    assert_eq!(
        p.get("om_suggested_response").and_then(|v| v.as_str()),
        Some("Walk through it.")
    );
    assert!(p.get("om_model").is_some());
}

#[test]
fn plan_below_activation_is_done_the_cycles_own_the_interval() {
    // #408: the background slice is always on. The [increment, threshold)
    // band belongs to the cycles: a settle over that window does nothing —
    // there is no sync Buffer arm left to absorb the tail.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 25, 1000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    // 25 × 1000 chars ≈ 6.3k raw tokens: below the 30k threshold, past the
    // 6k increment.
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Done => {}
        other => panic!("expected Done, got {other:?}"),
    }
    assert!(state.buffered.is_empty());
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
            observation_tokens: u32::try_from(crate::config::Om::default().reflect_threshold)
                .expect("token threshold, well under u32::MAX"),
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
    assert!(!suffix.contains("<suggested-response>"), "{suffix:?}");
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
        span: ObservedSpan {
            first_id: "00000001".into(),
            last_id: "00000003".into(),
            last_ts: 3,
        },
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

#[test]
fn plan_gates_on_the_raw_window_a_promote_left_behind_the_chunks() {
    // #392: a promote stamps `pending_tokens` with the tokens of the chunks it
    // left buffered (promote_moves_the_chunks_to_the_log_… pins that). The
    // gate is the raw window, so a window past activation is observed while
    // chunks remain buffered, and a window under it is not.
    let chunk = BufferedChunk {
        range: ("00000001".into(), "00000002".into()),
        last_ts: 100,
        text: "obs one".into(),
        tokens: 120,
    };
    let dir = tempfile::tempdir().unwrap();
    // 20 × 50k chars ≈ 250k tokens, far past the 30k activation.
    let mut store = store_with_text_entries(dir.path(), 20, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.buffered = vec![chunk.clone()];
    state.record.buffered_chunks = vec![chunk.clone()];
    state.record.pending_tokens = chunk.tokens;
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Observe { .. } => {}
        other => panic!("expected Observe over the raw window, got {other:?}"),
    }

    // The negative pair: the same chunks over a window under activation.
    let small = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(small.path(), 3, 100);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.buffered = vec![chunk.clone()];
    state.record.buffered_chunks = vec![chunk];
    state.record.pending_tokens = 120;
    let unobserved = state.unobserved(&mut store).unwrap();
    match state.plan(&unobserved) {
        TurnEndAction::Done => {}
        other => panic!("expected Done under activation, got {other:?}"),
    }
}

#[test]
fn the_observe_backstop_starts_past_the_buffer_cursor_not_over_chunk_material() {
    // #408: a durable chunk already carries an observation over the raw it
    // covers. The settle's backstop observes past the chunk's end, bounded
    // by the threshold, and never shows the Observer the span the chunk
    // stands for.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 40, 1_000);
    let om = crate::config::Om {
        observe_threshold: 1_000,
        reflect_threshold: 2_000,
        ..crate::config::Om::default()
    };
    let mut state = OmState::from_config(&om, OmRecord::default());
    let chunk = BufferedChunk {
        range: ("00000001".into(), "00000024".into()),
        last_ts: 100,
        text: "observed the head".into(),
        tokens: 6_000,
    };
    state.buffered = vec![chunk.clone()];
    state.record.buffered_chunks = vec![chunk];
    state.buffer_cursor = Some("00000024".into());
    let unobserved = state.unobserved(&mut store).unwrap();
    let mut action = state.plan(&unobserved);
    match &action {
        TurnEndAction::Observe { span, transcript } => {
            assert_eq!(
                span.first_id, "00000025",
                "the backstop re-observed the span the chunk stands for"
            );
            // 1000 chars ≈ 250 tokens: the 1k bound fits four entries.
            assert_eq!(span.last_id, "00000028", "the backstop overran its bound");
            assert_eq!(
                transcript.matches(&"w".repeat(1_000)).count(),
                4,
                "the transcript is not the bounded slice"
            );
        }
        other => panic!("expected Observe, got {other:?}"),
    }
    state
        .commit(
            &mut store,
            &mut action,
            &turn_result("<observations>tiny</observations>"),
        )
        .unwrap();
    assert_eq!(
        state.record.cursor.as_ref().map(|c| c.entry_id.as_str()),
        Some("00000028")
    );
    // The chunk stands until activation promotes it; the backstop's
    // observation and the chunk's are disjoint spans of the branch.
    assert_eq!(state.buffered.len(), 1);
}

#[test]
fn a_fully_buffered_window_plans_done_not_an_empty_slice() {
    // #399: the durable chunks can cover the whole window while the gate
    // quantity stays past the increment (buffering advances no observation
    // cursor, #392). The Buffer arm's tail is then empty — the plan is Done,
    // not a span over no entries.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 40, 1_000);
    let om = crate::config::Om {
        ..crate::config::Om::default()
    };
    let mut state = OmState::from_config(&om, OmRecord::default());
    let chunk = BufferedChunk {
        range: ("00000001".into(), "00000040".into()),
        last_ts: 100,
        text: "observed the whole window".into(),
        tokens: 10_000,
    };
    state.buffered = vec![chunk.clone()];
    state.record.buffered_chunks = vec![chunk];
    state.buffer_cursor = Some("00000040".into());
    let unobserved = state.unobserved(&mut store).unwrap();
    // The leg's premise: the gate quantity is the raw window, unchanged by
    // buffering, and the arm's gate passes.
    assert!(state.pending_tokens(&unobserved) >= state.config.buffer_increment());
    match state.plan(&unobserved) {
        TurnEndAction::Done => {}
        other => panic!("expected Done over a fully-buffered window, got {other:?}"),
    }
}

#[test]
fn an_empty_window_with_a_zero_threshold_plans_done() {
    // #399: `observe_threshold = 0` is a legal config and clears activation
    // on zero raw tokens. With the cursor past every entry the window is
    // empty — the plan is Done, not a span over no entries.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 3, 100);
    let om = crate::config::Om {
        observe_threshold: 0,
        ..crate::config::Om::default()
    };
    let record = OmRecord {
        cursor: Some(crate::om::Cursor {
            entry_id: "00000003".into(),
            timestamp: u64::MAX,
        }),
        ..OmRecord::default()
    };
    let mut state = OmState::from_config(&om, record);
    let unobserved = state.unobserved(&mut store).unwrap();
    assert!(unobserved.is_empty(), "the cursor is past every entry");
    match state.plan(&unobserved) {
        TurnEndAction::Done => {}
        other => panic!("expected Done over an empty window, got {other:?}"),
    }
}
