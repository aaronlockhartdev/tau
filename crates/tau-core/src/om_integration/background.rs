//! Mid-episode async buffering (ticket #86 P2, mastra parity): the
//! interval-boundary math and the background buffer cycle. The cycle
//! routes through the inner provider (invariant I2), so it stays
//! structurally invisible to the `ForwardingProvider` reconciliation that
//! the `#82` fix depends on.

use std::sync::{Arc, Mutex};

use crate::om::{
    OmConfig, generate_group_id, observer_system_prompt, parse_observer_output,
    wrap_in_observation_group,
};
use crate::om_integration::{BufferedChunk, OmState};
use crate::provider::{InputEntry, InputMessage, ResponseRequest, TurnProviderRef};

/// The effective buffer interval at a pending level (D5): the full
/// `buffer_tokens` below the ramp point, `buffer_tokens / 2` from there
/// on, so the cycle cadence doubles as the observe threshold approaches
/// (mastra `calculateObservationInterval`: `threshold - 1.1 * bufferTokens`).
fn effective_interval(pending: u64, config: &OmConfig) -> u64 {
    let full = u64::from(config.buffer_tokens);
    let ramp_point = u64::from(config.observe_threshold).saturating_sub(full * 11 / 10);
    if pending >= ramp_point {
        full / 2
    } else {
        full
    }
}

/// An interval boundary was crossed since the last commit (D5): floor
/// division over the effective interval, comparing against the LARGER of
/// the persisted boundary (record) and the in-process boundary (spawn
/// time, lost on restart).
pub(crate) fn boundary_crossed(
    pending: u64,
    config: &OmConfig,
    persisted: u64,
    in_process: u64,
) -> bool {
    let effective = effective_interval(pending, config);
    if effective == 0 {
        return false;
    }
    let last = persisted.max(in_process);
    pending / effective > last / effective
}

/// The background buffer cycle (D4, D8, D10, D14): re-locks the session,
/// selects the not-yet-buffered candidates, and — after a successful
/// Observer round-trip with the per-trigger-point retries — commits ONE
/// chunk to the file (the commit point, D10) and mirrors it into the
/// live state, then clears the in-flight flag. Exhaustion, failure, and
/// the trickle floor all leave only the flag cleared: the in-process
/// boundary already advanced at spawn, so the interval is consumed exactly
/// once and the raw material stays pending for the next trigger (D8).
/// Nothing here reaches the turn's event stream.
pub(crate) async fn buffer_cycle(
    inner: Arc<Mutex<crate::agent::Inner>>,
    config: OmConfig,
    provider: TurnProviderRef,
    model: String,
    boundary: u64,
) {
    // Phase 1 (under the lock): fresh record + candidates. The spawn-site
    // snapshot may be stale if the turn advanced; the file wins.
    let Some((candidates, candidate_tokens, range)) = read_candidates(&inner, &config) else {
        return;
    };

    // Trickle floor (D7): below half an interval, no chunk and no LLM
    // call — the interval is consumed (the in-process boundary advanced at
    // spawn) and the tail rides raw until the next crossing.
    if candidate_tokens < config.buffer_tokens / 2 {
        clear_inflight(&inner);
        return;
    }

    let system = observer_system_prompt();
    let request = ResponseRequest::new(
        model,
        Some(system.as_str()),
        vec![InputEntry::Message(InputMessage {
            role: "user".into(),
            content: super::transcript(&candidates),
        })],
    );
    let result =
        match super::retry::call_with_retry(&provider, &request, "buffer", config.retries.buffer)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("om: buffer cycle abandoned after retries: {e}");
                clear_inflight(&inner);
                return;
            }
        };
    let parsed = parse_observer_output(&result.text);
    if parsed.degenerate {
        clear_inflight(&inner);
        return;
    }
    let group_id = generate_group_id(&parsed.observations);
    let range_str = format!("{}:{}", range.0, range.1);
    let text = wrap_in_observation_group(&parsed.observations, &range_str, &group_id, None);
    commit_chunk(&inner, &candidates, range, candidate_tokens, text, boundary);
}

/// The cycle's phase 1 (D4): under the lock, load the freshest record,
/// rebuild the in-memory buffer mirror (the newest committed range end —
/// a restart loses the in-memory cursor), and select the not-yet-buffered
/// candidates. Failure clears the in-flight flag and returns `None`.
fn read_candidates(
    inner: &Arc<Mutex<crate::agent::Inner>>,
    config: &OmConfig,
) -> Option<(Vec<crate::session::Entry>, u32, (String, String))> {
    let mut g = match inner.lock() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("om: buffer cycle: session lock poisoned: {e}");
            return None;
        }
    };
    let record = match OmState::load_record(&mut g.store) {
        Ok(record) => record,
        Err(e) => {
            eprintln!("om: buffer cycle: record load failed: {e}");
            g.om_inflight = None;
            return None;
        }
    };
    let mut live = OmState {
        record,
        config: config.clone(),
        buffered: Vec::new(),
        buffer_cursor: None,
        changed: false,
    };
    live.buffered = live.record.buffered_chunks.clone();
    live.buffer_cursor = live
        .record
        .buffered_chunks
        .last()
        .map(|c| c.range.1.clone());
    let candidates = match live.unbuffered(&mut g.store) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("om: buffer cycle: candidate read failed: {e}");
            g.om_inflight = None;
            return None;
        }
    };
    let tokens = live.pending_tokens(&candidates);
    let range = (
        candidates.first().expect("non-empty candidates").id.clone(),
        candidates.last().expect("non-empty candidates").id.clone(),
    );
    Some((candidates, tokens, range))
}

/// The cycle's phase 2 (D10): the commit point — under the lock, reload
/// the freshest record (the turn-end pass may have saved during the
/// round-trip), push the chunk, advance the persisted boundary, save,
/// then mirror into the live state and clear the in-flight flag.
fn commit_chunk(
    inner: &Arc<Mutex<crate::agent::Inner>>,
    candidates: &[crate::session::Entry],
    range: (String, String),
    tokens: u32,
    text: String,
    boundary: u64,
) {
    let Ok(mut g) = inner.lock() else {
        return;
    };
    let mut latest = match OmState::load_record(&mut g.store) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("om: buffer cycle: record reload failed: {e}");
            g.om_inflight = None;
            return;
        }
    };
    if !latest.buffered_chunks.iter().any(|c| c.range == range) {
        latest.buffered_chunks.push(BufferedChunk {
            range,
            last_ts: candidates.last().expect("non-empty candidates").timestamp,
            text,
            tokens,
        });
        latest.last_buffered_at_tokens = boundary;
        if let Ok(payload) = serde_json::to_value(&latest) {
            let parent = g.store.leaf().ok().flatten().map(|e| e.id);
            if g.store
                .append(crate::om_integration::KIND_OM, payload, parent.as_deref())
                .is_err()
            {
                // The chunk is lost (rework at the next trigger, not data
                // loss); the interval stays consumed.
                eprintln!("om: buffer cycle: chunk commit failed");
            } else if let Some(slot) = g.om.as_mut() {
                slot.record = latest;
                slot.buffered = slot.record.buffered_chunks.clone();
                slot.buffer_cursor = slot
                    .record
                    .buffered_chunks
                    .last()
                    .map(|c| c.range.1.clone());
            }
        }
    }
    g.om_inflight = None;
}

/// Clears the in-flight flag (the cycle's abort paths).
fn clear_inflight(inner: &Arc<Mutex<crate::agent::Inner>>) {
    if let Ok(mut g) = inner.lock() {
        g.om_inflight = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(observe: u32, buffer: u32) -> OmConfig {
        OmConfig {
            observe_threshold: observe,
            buffer_tokens: buffer,
            ..Default::default()
        }
    }

    #[test]
    fn boundary_crosses_at_interval_multiples() {
        // T1: 30k / 6k, fresh session: crossings at 6k, 12k, 18k, 24k.
        let cfg = config(30_000, 6_000);
        for k in 1..=4u64 {
            assert!(
                boundary_crossed(k * 6_000, &cfg, 0, 0),
                "{k} intervals crossed"
            );
        }
        assert!(!boundary_crossed(5_999, &cfg, 0, 0));
    }

    #[test]
    fn boundary_is_monotone_and_persisted_wins() {
        // After a 12k commit, the next crossing needs 18k of pending.
        let cfg = config(30_000, 6_000);
        assert!(!boundary_crossed(12_000, &cfg, 12_000, 0));
        assert!(!boundary_crossed(17_999, &cfg, 12_000, 0));
        assert!(boundary_crossed(18_000, &cfg, 12_000, 0));
        // The in-process boundary (spawn time) is the other tier: a
        // triggered-but-uncommitted cycle suppresses an immediate
        // re-trigger at the same level.
        assert!(!boundary_crossed(12_000, &cfg, 0, 12_000));
    }

    #[test]
    fn ramp_halves_the_interval_near_threshold() {
        // ramp point = 30k - 1.1*6k = 23.4k: below it the interval is
        // 6k, from it on 3k (research §3.1 defaults).
        let cfg = config(30_000, 6_000);
        assert_eq!(effective_interval(23_399, &cfg), 6_000);
        assert_eq!(effective_interval(23_400, &cfg), 3_000);
    }

    #[test]
    fn disabled_when_buffer_tokens_is_zero() {
        // The kill switch (D1): buffer_tokens = 0 makes the effective
        // interval 0, so no boundary ever crosses — the sync turn-end
        // path owns everything.
        let cfg = config(30_000, 0);
        assert!(!boundary_crossed(60_000, &cfg, 0, 0));
        assert!(!boundary_crossed(60_000, &cfg, 12_000, 30_000));
    }
}
