use super::*;

/// Two buffered chunks of 3k tokens each (default config: 30k threshold,
/// 0.8 activation → 6k retention floor).
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
    // 5k pending < 6k floor: nothing to remove, the buffer stays whole.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 50);
    let mut state = state_with_two_chunks(5000);
    assert!(!state.promote(&mut store).unwrap());
    assert_eq!(state.buffered.len(), 2);
    assert!(state.record.cursor.is_none());
}

#[test]
fn promote_moves_the_chunks_to_the_log_and_advances_the_cursor() {
    // 10k pending over the 6k floor: the target is 4k, both 3k chunks
    // cross it, so both are promoted and the cursor lands on the last one.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 50);
    let mut state = state_with_two_chunks(10_000);
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
    assert!(state.changed);
    // The promoted record persists in the session file.
    let reloaded = OmState::load_record(&mut store).unwrap();
    assert_eq!(reloaded.cursor, state.record.cursor);
    assert!(reloaded.active_observations.contains("obs two"));
}

#[test]
fn promote_stops_at_the_boundary_and_keeps_the_rest_buffered() {
    // 10k pending, 6k floor → 4k target: the first 5k chunk alone crosses
    // it, so only it is promoted; the second stays buffered with its
    // tokens still pending.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 50);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    state.record.pending_tokens = 10_000;
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
    assert_eq!(state.buffered.len(), 1);
    assert_eq!(state.buffered[0].text, "obs two");
}

#[test]
fn promote_drains_the_durable_record_chunks() {
    // The record is the durable chunk store (ticket #86 P2): promote must
    // drain it in lockstep with the in-memory mirror, or a reloaded state
    // re-promotes the same chunks and appends their text to the log twice.
    let dir = tempfile::tempdir().unwrap();
    let mut store = store_with_text_entries(dir.path(), 4, 50);
    let mut state = state_with_two_chunks(10_000);
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
