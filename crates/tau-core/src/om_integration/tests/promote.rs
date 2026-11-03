use super::*;

/// Two buffered chunks of 3k tokens each (default config: 30k threshold,
/// 0.8 activation → 6k retention floor). `pending_tokens` is the caller's
/// decoy for the stored field: the target is computed from the raw window
/// in the store, not from that field (#392).
fn state_with_two_chunks(pending_tokens: u32) -> OmState {
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.pending_tokens = pending_tokens;
    state.buffered = vec![
        BufferedChunk {
            range: ("00000001".into(), "00000002".into()),
            last_ts: 100,
            text: "obs one".into(),
            tokens: 3000,
        },
        BufferedChunk {
            range: ("00000003".into(), "00000004".into()),
            last_ts: 200,
            text: "obs two".into(),
            tokens: 3000,
        },
    ];
    // The record is the durable chunk store (ticket #86 P2): the mirror
    // and the record are kept in lockstep.
    state.record.buffered_chunks = state.buffered.clone();
    state
}

#[test]
fn promote_with_no_buffered_chunks_is_false() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 2, 50);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    assert!(!state.promote(&mut store).unwrap());
}

#[test]
fn promote_leaves_chunks_buffered_at_or_below_the_floor() {
    // 5k raw under the 6k floor: nothing to remove — even with the stored
    // field inflated to 50k, the field no longer drives the target (#392).
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 2, 10_000);
    let mut state = state_with_two_chunks(50_000);
    assert!(!state.promote(&mut store).unwrap());
    assert_eq!(state.buffered.len(), 2);
    assert!(state.record.cursor.is_none());
}

#[test]
fn promote_moves_the_chunks_to_the_log_and_advances_the_cursor() {
    // 10k raw over the 6k floor: the target is 4k, both 3k chunks cross
    // it, so both are promoted and the cursor lands on the last one. The
    // stored field sits at 0: the raw window fires the promote on its own,
    // which is exactly the state the sink used to make unreachable (#392).
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 10_000);
    let mut state = state_with_two_chunks(0);
    assert!(state.promote(&mut store).unwrap());

    assert!(state.record.active_observations.contains("obs one"));
    assert!(state.record.active_observations.contains("obs two"));
    assert_eq!(
        state.record.cursor,
        Some(Cursor {
            entry_id: "00000004".into(),
            timestamp: 200,
        })
    );
    assert_eq!(state.record.pending_tokens, 0);
    assert!(state.buffered.is_empty());
    // The promoted record persists in the session file.
    let reloaded = OmState::load_record(&mut store).unwrap();
    assert_eq!(reloaded.cursor, state.record.cursor);
    assert!(reloaded.active_observations.contains("obs two"));
}

#[test]
fn promote_stops_at_the_boundary_and_keeps_the_rest_buffered() {
    // 10k raw, 6k floor → 4k target: the first 5k chunk alone crosses
    // it, so only it is promoted; the second stays buffered with its
    // tokens still pending.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 10_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.pending_tokens = 0;
    state.buffered = vec![
        BufferedChunk {
            range: ("00000001".into(), "00000002".into()),
            last_ts: 100,
            text: "obs one".into(),
            tokens: 5000,
        },
        BufferedChunk {
            range: ("00000003".into(), "00000004".into()),
            last_ts: 200,
            text: "obs two".into(),
            tokens: 5000,
        },
    ];
    // The record is the durable chunk store (ticket #86 P2): the mirror
    // and the record are kept in lockstep.
    state.record.buffered_chunks = state.buffered.clone();
    assert!(state.promote(&mut store).unwrap());

    assert!(state.record.active_observations.contains("obs one"));
    assert!(!state.record.active_observations.contains("obs two"));
    assert_eq!(
        state.record.cursor,
        Some(Cursor {
            entry_id: "00000002".into(),
            timestamp: 100,
        })
    );
    assert_eq!(state.record.pending_tokens, 5000);
    assert_eq!(state.buffered.len(), 1);
    assert_eq!(state.buffered[0].text, "obs two");
}

/// A buffered chunk the observation cursor already covers, for the #395 legs:
/// the cursor has seen its range, so it is spent material.
fn covered_chunk(first: &str, last: &str, tokens: u32, text: &str) -> BufferedChunk {
    BufferedChunk {
        range: (first.into(), last.into()),
        last_ts: 100,
        text: text.into(),
        tokens,
    }
}

#[test]
fn a_chunk_the_cursor_covers_promotes_neither_text_nor_cursor() {
    // The #395 pin. Ten raw entries (7500 tokens each); the cursor has observed
    // through 00000006, and the two buffered chunks cover 00000001..00000004 —
    // material the log already carries. The raw window (4 × 7500) is over the
    // 6000 floor, so the walk selects both chunks; stamping the cursor at the
    // second's end lands on 00000004, behind the 00000006 it started at, and
    // the entries between the two are un-observed. Their text also re-appends.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 10, 30_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.cursor = Some(Cursor {
        entry_id: "00000006".into(),
        timestamp: 600,
    });
    state.record.active_observations = "prior observation".into();
    state.buffered = vec![
        covered_chunk("00000001", "00000002", 3000, "obs one"),
        covered_chunk("00000003", "00000004", 3000, "obs two"),
    ];
    state.record.buffered_chunks = state.buffered.clone();

    assert!(
        !state.promote(&mut store).unwrap(),
        "a chunk the cursor covers is spent: it compacts nothing"
    );
    assert_eq!(
        state.record.cursor,
        Some(Cursor {
            entry_id: "00000006".into(),
            timestamp: 600,
        }),
        "the cursor never moves behind the raw the promote did not cover"
    );
    assert!(
        !state.record.active_observations.contains("obs one")
            && !state.record.active_observations.contains("obs two"),
        "spent material is not summarized into the log a second time: {:?}",
        state.record.active_observations
    );
    assert!(
        state.buffered.is_empty(),
        "the spent chunks leave the buffer"
    );
    assert!(
        state.record.buffered_chunks.is_empty(),
        "and the durable record, or a reload re-promotes them"
    );
}

#[test]
fn a_covered_prefix_is_skipped_and_the_walk_promotes_from_the_first_open_chunk() {
    // The positive: the cursor covers the walk's first two chunks and the third
    // is open. Raw 00000005..00000006 (2 × 7500) is over the floor, so the
    // third chunk promotes and the cursor moves forward from 00000004 to
    // 00000006; the skipped pair reaches neither log nor cursor.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 6, 30_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.cursor = Some(Cursor {
        entry_id: "00000004".into(),
        timestamp: 400,
    });
    state.buffered = vec![
        covered_chunk("00000001", "00000002", 3000, "obs one"),
        covered_chunk("00000003", "00000004", 3000, "obs two"),
        covered_chunk("00000005", "00000006", 3000, "obs three"),
    ];
    state.record.buffered_chunks = state.buffered.clone();

    assert!(
        state.promote(&mut store).unwrap(),
        "the open chunk promotes"
    );
    assert_eq!(
        state.record.cursor,
        Some(Cursor {
            entry_id: "00000006".into(),
            timestamp: 100,
        }),
        "the cursor advances over the chunk the walk promoted"
    );
    assert!(state.record.active_observations.contains("obs three"));
    assert!(
        !state.record.active_observations.contains("obs one")
            && !state.record.active_observations.contains("obs two"),
        "the covered prefix reaches the log: {:?}",
        state.record.active_observations
    );
    assert!(state.buffered.is_empty());
}

#[test]
fn spent_chunks_drain_when_the_floor_leaves_nothing_to_remove() {
    // The stale-target pin: raw sits at 5000 tokens, under the 6000 floor, so
    // the removal target is 0 and the walk promotes nothing. The covered chunk
    // must still drain, or every later activation recomputes the same target
    // over the same spent chunk and it is buffered forever.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 2, 10_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.cursor = Some(Cursor {
        entry_id: "00000002".into(),
        timestamp: 200,
    });
    state.buffered = vec![covered_chunk("00000001", "00000002", 3000, "obs one")];
    state.record.buffered_chunks = state.buffered.clone();

    assert!(!state.promote(&mut store).unwrap());
    assert!(
        state.buffered.is_empty(),
        "a spent chunk is not kept for an activation that can never take it"
    );
    assert!(state.record.buffered_chunks.is_empty());
    assert_eq!(
        state.record.cursor,
        Some(Cursor {
            entry_id: "00000002".into(),
            timestamp: 200,
        })
    );
}

#[test]
fn a_spent_chunk_at_the_tail_leaves_and_the_open_one_advances_the_cursor() {
    // The vec holds commit order, not id order: a background cycle tail-pushes
    // and `merge_turn_end` tail-appends (#395). The spent chunk (ending at 3,
    // behind the cursor at 4) is LAST here. A prefix scan stops at the open head
    // and never reaches it: it promotes both and stamps the cursor at 00000003,
    // backwards from 00000006, un-observing entries 4..6 with the spent text in
    // the log a second time.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 6, 30_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.cursor = Some(Cursor {
        entry_id: "00000004".into(),
        timestamp: 400,
    });
    state.buffered = vec![
        covered_chunk("00000005", "00000006", 3000, "obs five"),
        covered_chunk("00000001", "00000003", 3000, "obs spent"),
    ];
    state.record.buffered_chunks = state.buffered.clone();

    assert!(
        state.promote(&mut store).unwrap(),
        "the open chunk promotes"
    );
    assert!(state.record.active_observations.contains("obs five"));
    assert!(
        !state.record.active_observations.contains("obs spent"),
        "the tail's spent text reached the log: {:?}",
        state.record.active_observations
    );
    assert_eq!(
        state.record.cursor,
        Some(Cursor {
            entry_id: "00000006".into(),
            timestamp: 100,
        }),
        "the cursor advanced over the open chunk, never to the spent one's 3"
    );
    assert!(
        state.buffered.is_empty() && state.record.buffered_chunks.is_empty(),
        "the spent tail chunk leaves too, or every later target is computed over it"
    );
}

#[test]
fn promote_drains_the_durable_record_chunks() {
    // The record is the durable chunk store (ticket #86 P2): promote must
    // drain it in lockstep with the in-memory mirror, or a reloaded state
    // re-promotes the same chunks and appends their text to the log twice.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 10_000);
    let mut state = state_with_two_chunks(0);
    state.record.buffered_chunks = state.buffered.clone();

    assert!(state.promote(&mut store).unwrap());
    assert!(state.buffered.is_empty());
    assert!(state.record.buffered_chunks.is_empty());

    let reloaded = OmState::load_record(&mut store).unwrap();
    assert!(
        reloaded.buffered_chunks.is_empty(),
        "promoted chunks must not survive in the durable record"
    );
}
