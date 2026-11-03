//! The drain valve (#409): at the settle, a window past the drain line is
//! caught up in bounded slices, in the open. Lag blocks the settle, never
//! the turn.

use super::super::{OmError, OmState, SessionStore};
use super::TurnEndAction;
use crate::provider::TurnProviderRef;

/// The drain line as a percentage of the *effective* observe threshold
/// (#409): 120%, mastra's `blockAfter: 1.2` parity. Past it, "catch up
/// when idle" stops being a schedule and becomes lag, so the settle
/// drains the backlog itself.
const DRAIN_AFTER_PERCENT: u64 = 120;

/// The per-settle cap on drain slices (#409). Each slice is
/// threshold-sized, so eight covers any catch-up a turn can produce in a
/// session that honors its interval; the cap is the cost bound for
/// degenerate folds — a threshold so small the slices are entry-sized —
/// where an uncapped drain would pay one call per entry inside one
/// settle. The remainder drains at the next settle: the window is still
/// past the line, and the next settle's gate re-reads it.
const MAX_DRAIN_SLICES: u32 = 8;

impl OmState {
    /// The drain line: `DRAIN_AFTER_PERCENT` of the session's effective
    /// observe threshold — the same ramped number the plan's activation
    /// and the slice bound read, so line and plan ride one quantity.
    pub(in crate::om_integration::turn_end) fn drain_line(&self) -> u32 {
        let threshold = u64::from(
            self.config
                .observe_threshold_for(self.record.observation_tokens),
        );
        u32::try_from(threshold * DRAIN_AFTER_PERCENT / 100).unwrap_or(u32::MAX)
    }

    /// Run the settled plan, then drain if the window sits at or past the
    /// line. Each slice plans from live state: the window is re-read under
    /// the session lock and the buffer mirror re-built from the durable
    /// record — the same phase-1 rebuild `read_candidates` runs — so a
    /// background cycle committing between slices is never observed twice
    /// (#402's discipline, N-fold).
    ///
    /// A slice that advances nothing — a degenerate answer — ends the
    /// drain: the settle never pays twice for one frozen span. A failed
    /// slice returns `Err` with everything already paid committed; the
    /// agent latches `om_failed` once and the turn proceeds (#86).
    ///
    /// The gate quantity is the whole raw window past the observation
    /// cursor; the plan's is the slice past BOTH cursors — raw the log
    /// already carries in a chunk is not pending work. The promote, which
    /// runs before the gate, closes that gap: chunk-covered raw becomes
    /// observations free, and what stays chunk-covered is exactly what the
    /// plan refuses to re-show.
    pub(in crate::om_integration::turn_end) async fn run_and_drain<W>(
        &mut self,
        with_store: &mut W,
        provider: &TurnProviderRef,
        model: &str,
        hook: Option<&crate::agent::OmStatusHook>,
        action: &mut TurnEndAction,
    ) -> Result<(), OmError>
    where
        W: FnMut(&mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>) -> Result<(), OmError>,
    {
        let draining = self.record.pending_tokens >= self.drain_line();
        let cursor_before = self.record.cursor.clone();
        self.run_actions(with_store, provider, model, hook, action)
            .await?;
        if !draining || self.record.cursor == cursor_before {
            return Ok(());
        }
        let mut slices = 0u32;
        loop {
            let mut unobserved: Option<Vec<crate::session::Entry>> = None;
            with_store(&mut |store| {
                unobserved = Some(self.unobserved(store)?);
                self.sync_buffer_mirror_from_durable(store)?;
                Ok(())
            })?;
            let unobserved = unobserved.expect("the phase ran");
            self.record.pending_tokens = self.pending_tokens(&unobserved);
            if self.record.pending_tokens < self.drain_line() {
                break;
            }
            let cursor_before = self.record.cursor.clone();
            let mut slice = self.plan(&unobserved);
            if matches!(slice, TurnEndAction::Done) {
                break;
            }
            self.run_actions(with_store, provider, model, hook, &mut slice)
                .await?;
            slices += 1;
            if self.record.cursor == cursor_before || slices >= MAX_DRAIN_SLICES {
                break;
            }
        }
        Ok(())
    }

    /// Rebuild the buffer mirror from the durable record — the phase-1
    /// rebuild `read_candidates` runs (D4: the file wins over any
    /// snapshot). A cycle commits between drain slices through the
    /// session's own store; the drain's clone of the mirror is as stale as
    /// the pre-join clone #402 fixed, and `plan` cuts on it.
    fn sync_buffer_mirror_from_durable(&mut self, store: &mut SessionStore) -> Result<(), OmError> {
        let record = OmState::load_record(store)?;
        self.buffered = record.buffered_chunks;
        self.buffer_cursor = super::super::last_chunk_end(&self.buffered);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::om::OmConfig;

    fn state(observe_threshold: u64, observation_tokens: u32) -> OmState {
        OmState {
            record: crate::om::OmRecord {
                observation_tokens,
                ..Default::default()
            },
            config: OmConfig {
                observe_threshold: u32::try_from(observe_threshold).unwrap(),
                ..Default::default()
            },
            buffered: Vec::new(),
            buffer_cursor: None,
        }
    }

    #[test]
    fn the_drain_line_is_the_ramped_threshold_at_120_percent() {
        // The line rides the same effective threshold the plan gates on:
        // 30k base, no observations, and the share-budget ramp both land.
        assert_eq!(state(30_000, 0).drain_line(), 36_000);
        assert_eq!(state(1_000, 0).drain_line(), 1_200);
        // The ramp: with observations carrying the budget, the effective
        // threshold grows and the line grows with it.
        let mut ramped = state(30_000, 20_000);
        ramped.config.share_token_budget = true;
        ramped.config.reflect_threshold = 40_000;
        // effective = max(observe, observe + reflect - observed) = 50k.
        assert_eq!(ramped.drain_line(), 60_000);
    }

    #[test]
    fn the_drain_line_never_overflows_the_token_domain() {
        // A threshold at the u32 ceiling must not wrap the line to a small
        // number: the line saturates.
        assert_eq!(state(u64::from(u32::MAX), 0).drain_line(), u32::MAX);
    }
}
