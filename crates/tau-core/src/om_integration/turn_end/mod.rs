pub use self::commit::{ObservedSpan, TurnEndAction};
use self::window::{branch_of, raw_after};
use super::{
    BufferedChunk, Cursor, Entry, IDLE_ACTIVATION_SECS, OmError, OmState, SessionStore,
    boundary_ts, entry_text, idle_gap_secs, om, transcript,
};
use crate::provider::{InputEntry, InputMessage, ResponseRequest, TurnProviderRef};

/// Whether the observation cursor already covers a buffered chunk: the chunk's
/// range ends at or behind the cursor. The ids are minted session-sequentially
/// (`SessionStore::mint_id`), so the id's number is the entry's place in the
/// session's chronology and the comparison needs no branch walk; `<=` because a
/// fork inherits the parent's chunks and then observes onto a chunk's exact end.
/// A covered chunk is spent: appending its text re-summarizes raw the log
/// carries, and stamping the cursor at its end moves the cursor backwards,
/// un-observing the entries between it and the cursor (#395).
fn chunk_covered(cursor: Option<&Cursor>, chunk: &BufferedChunk) -> bool {
    let Some(cursor) = cursor else {
        return false;
    };
    match (cursor.entry_id.parse::<u64>(), chunk.range.1.parse::<u64>()) {
        (Ok(seen), Ok(end)) => end <= seen,
        _ => false,
    }
}

impl OmState {
    /// The unobserved raw on the active branch: the entries after the
    /// cursor. A missing cursor means the whole branch; a cursor the branch
    /// does not carry is cut by id number, as `raw_after` does (spec §4).
    pub fn unobserved(&self, store: &mut SessionStore) -> Result<Vec<Entry>, OmError> {
        let branch = branch_of(store)?;
        let Some(cursor) = &self.record.cursor else {
            return Ok(branch);
        };
        Ok(raw_after(&branch, Some(cursor.entry_id.as_str())))
    }

    #[must_use]
    pub fn pending_tokens(&self, entries: &[Entry]) -> u32 {
        entries
            .iter()
            .map(|e| om::token_count(&entry_text(e)))
            .sum()
    }

    /// The raw unobserved volume: the tokens of the entries after the
    /// observation cursor. Every activation gate reads this quantity (spec §4)
    /// and the `om_stalled` record carries it. The buffered chunks are
    /// compressions of the same raw material, so counting them (as a promote
    /// leaves `pending_tokens` set) reports a window no gate can see (#392).
    pub fn unobserved_tokens(&self, store: &mut SessionStore) -> Result<u32, OmError> {
        Ok(self.pending_tokens(&self.unobserved(store)?))
    }

    /// Activation (no LLM call, mastra `swapBufferedToActive`): promote
    /// buffered chunks to the log up to the boundary that lands the
    /// remaining raw at or below the retention floor
    /// (`projected_message_removal`), leaving later chunks buffered. The
    /// observation cursor advances to the last promoted entry; `false` when
    /// there is nothing to promote.
    pub fn promote(&mut self, store: &mut SessionStore) -> Result<bool, OmError> {
        if self.buffered.is_empty() {
            return Ok(false);
        }
        // A chunk the cursor covers is spent material: its text re-summarizes
        // raw the log carries, and stamping its end moves the cursor backwards
        // (#395). Chunks arrive in commit order, not id order — a background
        // cycle tail-pushes, `merge_turn_end` tail-appends, and a fork inherits
        // the parent's vec — so a spent chunk sits anywhere in it. The walk
        // tests each chunk against the cursor as it advances, so a chunk the
        // promote itself covers counts as spent too.
        let mut running = self.record.cursor.clone();
        let mut open: Vec<usize> = Vec::new();
        for (i, chunk) in self.buffered.iter().enumerate() {
            if !chunk_covered(running.as_ref(), chunk) {
                open.push(i);
                running = Some(Cursor {
                    entry_id: chunk.range.1.clone(),
                    timestamp: chunk.last_ts,
                });
            }
        }
        let spent: Vec<usize> = (0..self.buffered.len())
            .filter(|i| !open.contains(i))
            .collect();
        // The removal target is computed against the raw window, not the
        // stored field: a prior promote leaves chunk tokens there, and the
        // compressed chunks understate the volume the ladder must clear (#392).
        let remove = if open.is_empty() {
            0
        } else {
            let chunks: Vec<om::ChunkTokens> = open
                .iter()
                .map(|i| om::ChunkTokens(self.buffered[*i].tokens))
                .collect();
            let raw = self.unobserved_tokens(store)?;
            om::projected_message_removal(&chunks, &self.config, raw)
        };
        let mut count = 0usize;
        if remove > 0 {
            // The returned count is the cumulative at the selected boundary;
            // the cumulative sums are monotone, so the walk lands exactly on it.
            let mut cumulative = 0u32;
            for i in &open {
                cumulative = cumulative.saturating_add(self.buffered[*i].tokens);
                count += 1;
                if cumulative >= remove {
                    break;
                }
            }
        }
        if count == 0 {
            // Pending is already at or below the retention floor: the open
            // chunks stay buffered for the next activation. The spent ones
            // leave — kept, they are the stale material every later target is
            // computed over.
            if !spent.is_empty() {
                self.drain_to(store, &open)?;
            }
            return Ok(false);
        }
        for i in &open[..count] {
            let chunk = &self.buffered[*i];
            self.record.active_observations = om::append_observation(
                &self.record.active_observations,
                &boundary_ts(),
                &chunk.text,
            );
            self.record.cursor = Some(Cursor {
                entry_id: chunk.range.1.clone(),
                timestamp: chunk.last_ts,
            });
        }
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        let kept: Vec<usize> = open[count..].to_vec();
        self.record.pending_tokens = kept.iter().map(|i| self.buffered[*i].tokens).sum();
        self.drain_to(store, &kept)?;
        Ok(true)
    }

    /// The promote's drain, in lockstep with the durable record: every chunk at
    /// an index outside `kept` leaves both — spent material (its text reaches no
    /// log) and promoted material alike. A promoted chunk that survives in the
    /// record is re-promoted, its text appended a second time (ticket #86 P2).
    /// The record is written only when something left: a promote that changes
    /// nothing appends no `om` entry.
    fn drain_to(&mut self, store: &mut SessionStore, kept: &[usize]) -> Result<(), OmError> {
        let keep = |i: usize| kept.contains(&i);
        if self.buffered.len() == kept.len() {
            return Ok(());
        }
        self.buffered = self
            .buffered
            .iter()
            .enumerate()
            .filter(|(i, _)| keep(*i))
            .map(|(_, c)| c.clone())
            .collect();
        self.record.buffered_chunks = self
            .record
            .buffered_chunks
            .iter()
            .enumerate()
            .filter(|(i, _)| keep(*i))
            .map(|(_, c)| c.clone())
            .collect();
        self.save(store).map_err(OmError::from)
    }

    #[must_use]
    pub fn activation_reached(&self, pending_tokens: u32) -> bool {
        om::should_observe(pending_tokens, self.record.observation_tokens, &self.config)
    }

    pub(super) fn maintain_prefix_budget(&mut self) {
        let combined = om::token_count(&self.record.live_observations());
        if !self.record.prefix_demoted && combined > self.config.reflect_threshold {
            self.record.prefix_demoted = true;
        } else if self.record.prefix_demoted
            && om::token_count(&self.record.active_observations) < self.config.reflect_threshold
        {
            self.record.prefix_demoted = false;
        }
    }
}

impl OmState {
    /// The turn-end policy sequence (R4): the unobserved read and pending
    /// count, the buffered-chunk activation (token threshold, or the fixed
    /// idle timeout, with pending chunks — it runs first, costs no LLM call,
    /// and the plan's gate then reads the raw window that remains), the plan,
    /// and the observe/reflect/buffer/commit loop until `Done`. The state owns
    /// the sequence; `with_store` is the caller's re-lock — each sync store
    /// phase runs under the session lock and releases it before the LLM
    /// round-trips (a std guard cannot cross an await in a spawned future), so
    /// the pass never holds the lock across a provider call.
    pub async fn settle_turn<W>(
        &mut self,
        with_store: &mut W,
        provider: &TurnProviderRef,
        model: &str,
        hook: Option<&crate::agent::OmStatusHook>,
    ) -> Result<(), OmError>
    where
        W: FnMut(&mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>) -> Result<(), OmError>,
    {
        let mut unobserved: Option<Vec<Entry>> = None;
        with_store(&mut |store| {
            unobserved = Some(self.unobserved(store)?);
            Ok(())
        })?;
        let mut unobserved = unobserved.expect("the phase ran");
        self.record.pending_tokens = self.pending_tokens(&unobserved);
        // Activation (no LLM call): the token threshold, or the fixed idle
        // timeout with pending chunks (spec §4).
        let mut idle = 0u64;
        with_store(&mut |store| {
            // The whole log by design (#302): the idle gap is measured over
            // the leaf's branch (spec §3).
            let all = store.entries_range(0, usize::MAX)?;
            let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
            idle = idle_gap_secs(&all, leaf.as_deref());
            Ok(())
        })?;
        if !self.buffered.is_empty()
            && (self.activation_reached(self.record.pending_tokens) || idle >= IDLE_ACTIVATION_SECS)
        {
            let mut promoted = false;
            with_store(&mut |store| {
                promoted = self.promote(store)?;
                Ok(())
            })?;
            if promoted {
                // Promotion moved the observation cursor over the chunk
                // ranges: the plan's gate quantity is the raw window that
                // remains, so a tail that still reaches activation is observed
                // in this settle instead of waiting for the next idle gap
                // (#392).
                with_store(&mut |store| {
                    unobserved = self.unobserved(store)?;
                    Ok(())
                })?;
                self.record.pending_tokens = self.pending_tokens(&unobserved);
            }
        }
        let mut action = self.plan(&unobserved);
        self.run_actions(with_store, provider, model, hook, &mut action)
            .await
    }

    /// The observe/reflect round-trips for one action, run to `Done`: the
    /// turn-end sequence calls this after `plan`; the context gate calls it
    /// with its own action, driving the ladder's rungs without a plan (#338).
    /// Each pass reports its activity on the status gauge, the way the
    /// turn-end sequence does.
    pub(crate) async fn run_actions<W>(
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
        loop {
            // The activity the status bar's gauge shows while this run is in
            // flight; `Done` closes the run (the hook is the app's om_status
            // emitter, absent in tests and on children).
            if let Some(hook) = hook {
                let kind = match &action {
                    TurnEndAction::Done => "idle",
                    TurnEndAction::Observe { .. } | TurnEndAction::Buffer { .. } => "observing",
                    TurnEndAction::Reflect { .. } => "reflecting",
                };
                hook(kind);
            }
            let result = match &action {
                TurnEndAction::Done => break,
                // The Observer's call at two trigger points, each with its own
                // retry knob (ticket #86); the stage is what names a failure's
                // place in the `om_failed` record (#338).
                TurnEndAction::Observe { transcript, .. } => {
                    let request = Self::observer_request(model, transcript);
                    super::retry::call_with_retry(
                        provider,
                        &request,
                        super::OmStage::Observe,
                        self.config.retries.observe,
                    )
                    .await?
                }
                TurnEndAction::Buffer { transcript } => {
                    let request = Self::observer_request(model, transcript);
                    super::retry::call_with_retry(
                        provider,
                        &request,
                        super::OmStage::Buffer,
                        self.config.retries.buffer,
                    )
                    .await?
                }
                TurnEndAction::Reflect { level } => {
                    let prompt = self.reflector_prompt(*level);
                    let system = om::reflector_system_prompt();
                    let request = ResponseRequest::new(
                        model.to_owned(),
                        Some(system.as_str()),
                        vec![InputEntry::Message(InputMessage {
                            role: "user".into(),
                            content: prompt,
                        })],
                    );
                    super::retry::call_with_retry(
                        provider,
                        &request,
                        super::OmStage::Reflect,
                        self.config.retries.reflect,
                    )
                    .await?
                }
            };
            with_store(&mut |store| self.commit(store, action, &result))?;
        }
        Ok(())
    }

    /// The Observer's request for a transcript: the observer system prompt and
    /// the transcript as its one user item.
    fn observer_request(model: &str, transcript: &str) -> ResponseRequest {
        ResponseRequest::new(
            model.to_owned(),
            Some(om::observer_system_prompt().as_str()),
            vec![InputEntry::Message(InputMessage {
                role: "user".into(),
                content: transcript.to_owned(),
            })],
        )
    }

    /// The gate's overflow rung (#338): re-compact the managed suffix at each
    /// escalating reflect level, then demote the frozen prefix. The turn-end
    /// pass reaches the demotion only at `reflect_threshold`; the gate applies
    /// it because the prompt — not the threshold — is over budget. `with_store`
    /// is the caller's re-lock, as in `settle_turn`.
    pub(crate) async fn overflow_compact<W>(
        &mut self,
        with_store: &mut W,
        provider: &TurnProviderRef,
        model: &str,
        hook: Option<&crate::agent::OmStatusHook>,
    ) -> Result<(), OmError>
    where
        W: FnMut(&mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>) -> Result<(), OmError>,
    {
        // The reflect levels escalate the compression asked for (spec §4);
        // `commit` advances the level while a rewrite is refused and lands at
        // `Done` when one is applied or the levels run out.
        let mut action = TurnEndAction::Reflect { level: 0 };
        self.run_actions(with_store, provider, model, hook, &mut action)
            .await?;
        // The prefix is the log's oldest material, so it leaves last; recall
        // still reaches it (spec §4's overflow ladder).
        self.record.prefix_demoted = true;
        with_store(&mut |store| self.save(store).map_err(OmError::from))
    }

    /// The turn-end decision (pure over the record — `settle_turn` feeds it
    /// the unobserved entries it just read): reflect at the observation
    /// threshold, observe at activation, buffer at the increment (spec §4).
    /// Activation and the increment are evaluated on the raw volume of those
    /// entries — the quantity the stall diagnostic reports — never on
    /// `pending_tokens`, which a promote rewrites with the tokens of the
    /// chunks it left buffered (#392).
    /// The sync Buffer arm is the kill-switch path only: with async
    /// buffering enabled (`buffer_tokens > 0`) the mid-loop background
    /// cycles own the sub-threshold range (ticket #86 P2, D2/T6), so the
    /// turn-end pass never buffers.
    pub fn plan(&mut self, unobserved: &[Entry]) -> TurnEndAction {
        let raw = self.pending_tokens(unobserved);
        if om::should_reflect(self.record.observation_tokens, &self.config) {
            return TurnEndAction::Reflect { level: 0 };
        }
        if self.activation_reached(raw) {
            // A settle observes a bounded span, not the whole window: the bound
            // is the activation threshold, the volume one Observer pass is sized
            // for. A window that clears activation by more than that — a sunken
            // session, a first settle over a frozen cursor — pays a
            // threshold-sized slice per settle instead of the whole window in one
            // Observer call (#394; the gate is #392). The span the Observer was
            // shown is the span the commit stamps, so the tail stays unobserved
            // for the next settle.
            let (span, transcript) = self.observe_head(unobserved);
            return TurnEndAction::Observe { transcript, span };
        }
        if self.config.buffer_tokens == 0 && raw >= self.config.buffer_increment() {
            // The transcript covers only the not-yet-buffered tail of the
            // unobserved range (the buffer cursor keeps runs disjoint).
            let start = match &self.buffer_cursor {
                Some(id) => match unobserved.iter().position(|e| e.id == *id) {
                    Some(i) => i + 1,
                    None => 0,
                },
                None => 0,
            };
            return TurnEndAction::Buffer {
                transcript: transcript(&unobserved[start..]),
            };
        }
        TurnEndAction::Done
    }

    /// The span one Observer call covers: the leading entries of the unobserved
    /// window, up to the activation threshold, and that span's transcript. The
    /// threshold is the volume one Observer pass is sized for (spec §4: raw
    /// oscillates between the retention floor and the threshold), so a window
    /// that clears activation by more than that — a sunken session's history, a
    /// first settle over a frozen cursor — is observed a threshold-sized slice
    /// per settle instead of in one call whose input is the whole window
    /// (#394; the gate is #392).
    ///
    /// Two floors keep the span worth a round-trip. It ends at an entry
    /// boundary, the way the §4 window cut does, so a tool call and its result
    /// stay in one span. And it reaches the first entry that carries text: a
    /// span of content-less entries (an opening entry with an empty payload) is
    /// a call that observes nothing, and the cursor would advance over material
    /// the Observer was never shown. A window at activation is never observed by
    /// nothing.
    #[must_use]
    fn observe_head(&self, unobserved: &[Entry]) -> (ObservedSpan, String) {
        let bound = self
            .config
            .observe_threshold_for(self.record.observation_tokens);
        // The span reaches the first entry with prompt material, so the Observer
        // is given something to observe.
        let content_floor = unobserved
            .iter()
            .position(|e| !entry_text(e).trim().is_empty())
            .map_or(unobserved.len(), |i| i + 1);
        let mut len = content_floor;
        let mut seen = self.pending_tokens(&unobserved[..len]);
        for entry in &unobserved[len..] {
            let tokens = self.pending_tokens(std::slice::from_ref(entry));
            if seen.saturating_add(tokens) > bound {
                break;
            }
            seen = seen.saturating_add(tokens);
            len += 1;
        }
        let head = &unobserved[..len];
        (ObservedSpan::of(head), transcript(head))
    }

    /// The Reflector prompt for a level (spec §4): the frozen variant
    /// (ADR-0004) when a frozen prefix is present — the prefix never
    /// enters the prompt body and stays byte-verbatim.
    #[must_use]
    pub fn reflector_prompt(&self, level: u8) -> String {
        let source = self.record.reflect_source();
        if self.record.frozen_prefix.is_empty() {
            om::build_reflector_prompt(source, level)
        } else {
            om::build_reflector_prompt_frozen(&self.record.frozen_prefix, source, level)
        }
    }
}

mod assemble;
mod commit;
mod window;
