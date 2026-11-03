//! Mid-episode async buffering (ticket #86 P2, mastra parity): the
//! interval-boundary math and the background buffer cycle. The cycle
//! routes through the inner provider (invariant I2), so it stays
//! structurally invisible to the `ForwardingProvider` reconciliation that
//! the `#82` fix depends on.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::om::{
    OmConfig, generate_group_id, observer_system_prompt, parse_observer_output,
    wrap_in_observation_group,
};
use crate::om_integration::{BufferedChunk, OmState};
use crate::provider::{InputEntry, InputMessage, ResponseRequest, TurnProviderRef};
use tau_protocol::EntryKind;

/// The effective buffer interval at a pending level (D5, #408): the full
/// `buffer_increment` below the ramp point, `buffer_increment / 2` from
/// there on, so the cycle cadence doubles as the observe threshold approaches
/// (mastra `calculateObservationInterval`: `threshold - 1.1 * interval`).
fn effective_interval(pending: u64, config: &OmConfig) -> u64 {
    let full = u64::from(config.buffer_increment());
    let ramp_point = u64::from(config.observe_threshold).saturating_sub(full * 11 / 10);
    if pending >= ramp_point {
        full / 2
    } else {
        full
    }
}

/// The trickle floor (D7): below half an interval of spawned volume, a
/// cycle gets no chunk and no LLM call. The basis is the interval knob —
/// #408 retired the quantity that used to name it.
#[must_use]
fn trickle_floor(config: &OmConfig) -> u32 {
    config.buffer_increment() / 2
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
    let Some((candidates, coverage, range, volume)) = read_candidates(&inner, &config) else {
        return;
    };

    // Trickle floor (D7): the spawned volume — all not-yet-buffered
    // material, not the call's bounded slice — below half an interval gets
    // no chunk and no LLM call; the interval is consumed (the in-process
    // boundary advanced at spawn) and the tail rides raw until the next
    // crossing.
    if volume < trickle_floor(&config) {
        clear_inflight(&inner);
        return;
    }

    let addressed = model.clone();
    let system = observer_system_prompt();
    let request = ResponseRequest::new(
        model,
        Some(system.as_str()),
        vec![InputEntry::Message(InputMessage {
            role: "user".into(),
            content: super::transcript(&candidates),
        })],
    );
    let result = match super::retry::call_with_retry(
        &provider,
        &request,
        super::OmStage::Buffer,
        config.retries.buffer,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Exhaustion is a fact the record keeps, not a line the log forgets
            // (scope 4): this is `retries.buffer`'s most frequent consumer, so
            // the incident's dead `om_model` route failed here every interval
            // with nothing to show. The stage is the one the retry loop ran
            // under; the #306 latch keeps it to one record.
            inner.lock().record_om_failure(super::OmStage::Buffer, &e);
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
    commit_chunk(
        &inner,
        &candidates,
        range,
        coverage,
        text,
        boundary,
        &addressed,
    );
}

/// What one background slice call consumes: the bounded entries, their raw
/// coverage, the covered range, and the whole spawned volume the trickle
/// floor gates on.
type SliceBatch = (Vec<crate::session::Entry>, u32, (String, String), u32);

/// The cycle's phase 1 (D4): under the lock, load the freshest record,
/// rebuild the in-memory buffer mirror (the newest committed range end —
/// a restart loses the in-memory cursor), and select the not-yet-buffered
/// candidates. Failure clears the in-flight flag and returns `None`.
fn read_candidates(
    inner: &Arc<Mutex<crate::agent::Inner>>,
    config: &OmConfig,
) -> Option<SliceBatch> {
    let mut g = inner.lock();
    let record = match OmState::load_record(&mut g.store) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!("om: buffer cycle: record load failed: {e}");
            g.om_inflight = None;
            return None;
        }
    };
    let mut live = OmState {
        record,
        config: config.clone(),
        buffered: Vec::new(),
        buffer_cursor: None,
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
            tracing::warn!("om: buffer cycle: candidate read failed: {e}");
            g.om_inflight = None;
            return None;
        }
    };
    // The turn-end pass may have committed everything the spawn snapshot
    // saw (D4: the file cursor wins over the snapshot): nothing left to
    // buffer. Consume the trigger and stop — a panic under the session's
    // inner lock would leave a half-mutated `Inner` that no later reader
    // could tell apart from a good one.
    if candidates.is_empty() {
        g.om_inflight = None;
        return None;
    }
    // The cycle's call is the next increment-sized slice, not every
    // unbuffered entry: no Observer call's input is a whole window (#399).
    let (span, _, coverage) = live.observe_head(&candidates, config.buffer_increment());
    let cut = candidates
        .iter()
        .position(|e| e.id == span.last_id)
        .map_or(candidates.len(), |i| i + 1);
    let head = candidates[..cut].to_vec();
    // The floor in `buffer_cycle` reads this, the volume the trigger
    // covered, not the clipped slice's coverage.
    let volume = live.pending_tokens(&candidates);
    Some((head, coverage, (span.first_id, span.last_id), volume))
}

/// The cycle's phase 2 (D10): the commit point — under the lock, reload
/// the freshest record (the turn-end pass may have saved during the
/// round-trip), push the chunk, advance the persisted boundary, save,
/// then mirror into the live state and clear the in-flight flag. A chunk
/// whose range intersects a durable one is a late re-observe of the same
/// raw and is dropped — the interval stays consumed, the span is observed
/// once (#402).
/// The record names the endpoint the pass ran at, so a chunk committed by a
/// background cycle is attributed like a synchronous pass's (#348).
// `pub(crate)`, not `pub(super)`: the agent test module drives the commit
// point directly — the join interlock (#402) is an agent-level mechanism.
pub(crate) fn commit_chunk(
    inner: &Arc<Mutex<crate::agent::Inner>>,
    candidates: &[crate::session::Entry],
    range: (String, String),
    tokens: u32,
    text: String,
    boundary: u64,
    model: &str,
) {
    let mut g = inner.lock();
    let mut latest = match OmState::load_record(&mut g.store) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("om: buffer cycle: record reload failed: {e}");
            g.om_inflight = None;
            return;
        }
    };
    model.clone_into(&mut latest.om_model);
    if !latest
        .buffered_chunks
        .iter()
        .any(|c| crate::om_integration::ranges_overlap(&c.range, &range))
    {
        latest.buffered_chunks.push(BufferedChunk {
            range,
            last_ts: candidates
                .last()
                .expect("read_candidates guards a non-empty batch")
                .timestamp,
            text,
            tokens,
        });
        latest.last_buffered_at_tokens = boundary;
        if let Ok(payload) = serde_json::to_value(&latest) {
            let parent = g.store.leaf().ok().flatten().map(|e| e.id);
            if g.store
                .append(EntryKind::Om, payload, parent.as_deref())
                .is_err()
            {
                // The chunk is lost (rework at the next trigger, not data
                // loss); the interval stays consumed.
                tracing::warn!("om: buffer cycle: chunk commit failed");
            } else if let Some(slot) = g.om.as_mut() {
                slot.record = latest;
                slot.buffered = slot.record.buffered_chunks.clone();
                slot.buffer_cursor = super::last_chunk_end(&slot.record.buffered_chunks);
            }
        }
    }
    g.om_inflight = None;
}

/// Clears the in-flight flag (the cycle's abort paths).
fn clear_inflight(inner: &Arc<Mutex<crate::agent::Inner>>) {
    let mut g = inner.lock();
    g.om_inflight = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(observe: u32, increment: u32) -> OmConfig {
        OmConfig {
            observe_threshold: observe,
            buffer_activation: 1.0 - f64::from(increment) / f64::from(observe),
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
    fn a_zero_increment_never_crosses() {
        // #408: the interval knob is the increment. A config that folds to a
        // zero increment — the retention floor at 1.0 — leaves the boundary
        // math no multiples to cross: the cycles stay silent at any volume.
        let cfg = config(30_000, 0);
        assert_eq!(cfg.buffer_increment(), 0);
        assert!(!boundary_crossed(60_000, &cfg, 0, 0));
        assert!(!boundary_crossed(60_000, &cfg, 12_000, 30_000));
    }

    #[test]
    fn the_trickle_floor_is_half_the_increment() {
        // #408: the floor's basis is `buffer_increment`, not a constant.
        // The fixture-shape increment (230) gets a 115 floor — a 675-volume
        // cycle passes — and a wide 40k increment gets a 20k floor, so a
        // 5k trickle pays no Observer round-trip.
        assert_eq!(trickle_floor(&config(1_000, 230)), 115);
        assert_eq!(trickle_floor(&config(50_000, 40_000)), 20_000);
        assert_eq!(trickle_floor(&config(30_000, 6_000)), 3_000);
    }
}
