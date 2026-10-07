use super::{
    BufferedChunk, Cursor, Entry, IDLE_ACTIVATION_SECS, OmError, OmState, SessionStore,
    branch_entries, entry_text, idle_gap_secs, is_raw, now_ms, om, transcript,
};
use crate::provider::{InputEntry, InputMessage, ResponseRequest, TurnProviderRef};

impl OmState {
    /// The unobserved raw on the active branch: the entries after the
    /// cursor. A missing cursor means the whole branch; a cursor not on
    /// this branch (a branch switch) means the branch is unobserved —
    /// the cursor is path-scoped (spec §4).
    pub fn unobserved(&self, store: &mut SessionStore) -> Result<Vec<Entry>, OmError> {
        let all = store.entries_range(0, usize::MAX)?;
        let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
        let branch = branch_entries(&all, leaf.as_deref());
        let Some(cursor) = &self.record.cursor else {
            return Ok(branch);
        };
        let unobserved = match branch.iter().position(|e| e.id == cursor.entry_id) {
            Some(i) => &branch[i + 1..],
            None => &branch[..],
        };
        Ok(unobserved.iter().filter(|e| is_raw(e)).cloned().collect())
    }

    #[must_use]
    pub fn pending_tokens(&self, entries: &[Entry]) -> u32 {
        entries
            .iter()
            .map(|e| om::token_count(&entry_text(e)))
            .sum()
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
        let chunks: Vec<om::ChunkTokens> = self
            .buffered
            .iter()
            .map(|c| om::ChunkTokens(c.tokens))
            .collect();
        let remove =
            om::projected_message_removal(&chunks, &self.config, self.record.pending_tokens);
        if remove == 0 {
            // Pending is already at or below the retention floor: the
            // chunks stay buffered for the next activation.
            return Ok(false);
        }
        // The returned count is the cumulative at the selected boundary;
        // the cumulative sums are monotone, so the walk lands exactly on it.
        let mut count = 0usize;
        let mut cumulative = 0u32;
        for chunk in &self.buffered {
            cumulative = cumulative.saturating_add(chunk.tokens);
            count += 1;
            if cumulative >= remove {
                break;
            }
        }
        for chunk in &self.buffered[..count] {
            self.record.active_observations =
                om::append_observation(&self.record.active_observations, &now_ms(), &chunk.text);
            self.record.cursor = Some(Cursor {
                entry_id: chunk.range.1.clone(),
                timestamp: chunk.last_ts,
            });
        }
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        self.record.pending_tokens = self.buffered.iter().skip(count).map(|c| c.tokens).sum();
        self.buffered.drain(0..count);
        // The record is the durable chunk store (ticket #86 P2): promote
        // drains it in lockstep, or the promoted chunks would be re-promoted
        // (their text appended to the log a second time) on the next
        // activation.
        self.record.buffered_chunks.drain(0..count);
        self.changed = true;
        self.save(store)?;
        Ok(true)
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
/// The action a turn-end plan calls for. The provider call runs outside the
/// session's write lock (the OM call is an LLM round-trip, not a user-facing
/// stream); only the sync store ops run under it.
#[derive(Debug, PartialEq)]
pub enum TurnEndAction {
    Observe { transcript: String },
    Reflect { level: u8 },
    Buffer { transcript: String },
    Done,
}

impl OmState {
    /// The turn-end policy sequence (R4): the unobserved read and pending
    /// count, the buffered-chunk activation (token threshold or the fixed
    /// idle timeout), the plan, and the observe/reflect/buffer/commit loop
    /// until `Done`. The state owns the sequence; `with_store` is the
    /// caller's re-lock — each sync store phase runs under the session
    /// lock and releases it before the LLM round-trips (a std guard cannot
    /// cross an await in a spawned future), so the pass never holds the
    /// lock across a provider call.
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
        let unobserved = unobserved.expect("the phase ran");
        self.record.pending_tokens = self.pending_tokens(&unobserved);
        // Activation (no LLM call): the token threshold, or the fixed idle
        // timeout with pending chunks (spec §4).
        let mut idle = 0u64;
        with_store(&mut |store| {
            let all = store.entries_range(0, usize::MAX)?;
            let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
            idle = idle_gap_secs(&all, leaf.as_deref());
            Ok(())
        })?;
        if !self.buffered.is_empty()
            && (self.activation_reached(self.record.pending_tokens) || idle >= IDLE_ACTIVATION_SECS)
        {
            with_store(&mut |store| self.promote(store).map(|_| ()))?;
        }
        let mut action = self.plan(&unobserved);
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
                TurnEndAction::Observe { transcript } | TurnEndAction::Buffer { transcript } => {
                    let system = om::observer_system_prompt();
                    let request = ResponseRequest::new(
                        model.to_owned(),
                        Some(system.as_str()),
                        vec![InputEntry::Message(InputMessage {
                            role: "user".into(),
                            content: transcript.clone(),
                        })],
                    );
                    // Per-trigger-point retry counts (ticket #86): observe
                    // and buffer have separate knobs.
                    let (label, retries) = if matches!(action, TurnEndAction::Observe { .. }) {
                        ("observe", self.config.retries.observe)
                    } else {
                        ("buffer", self.config.retries.buffer)
                    };
                    super::retry::call_with_retry(provider, &request, label, retries).await?
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
                        "reflect",
                        self.config.retries.reflect,
                    )
                    .await?
                }
            };
            model.clone_into(&mut self.record.om_model);
            with_store(&mut |store| self.commit(store, &mut action, &result))?;
        }
        Ok(())
    }

    /// The turn-end decision (pure over the record — `settle_turn` feeds it
    /// the unobserved entries it just read): reflect at the observation
    /// threshold, observe at activation, buffer at the increment (spec §4).
    /// The sync Buffer arm is the kill-switch path only: with async
    /// buffering enabled (`buffer_tokens > 0`) the mid-loop background
    /// cycles own the sub-threshold range (ticket #86 P2, D2/T6), so the
    /// turn-end pass never buffers.
    pub fn plan(&mut self, unobserved: &[Entry]) -> TurnEndAction {
        if om::should_reflect(self.record.observation_tokens, &self.config) {
            return TurnEndAction::Reflect { level: 0 };
        }
        if self.activation_reached(self.record.pending_tokens) {
            return TurnEndAction::Observe {
                transcript: transcript(unobserved),
            };
        }
        if self.config.buffer_tokens == 0
            && self.record.pending_tokens >= self.config.buffer_increment()
        {
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

    /// Commit a completed turn-end action (sync — the agent loop runs it
    /// under the session lock after the LLM round-trip). The action
    /// advances: a rejected reflection level escalates, anything applied
    /// becomes `Done`.
    pub fn commit(
        &mut self,
        store: &mut SessionStore,
        action: &mut TurnEndAction,
        result: &crate::provider::TurnResult,
    ) -> Result<(), OmError> {
        let parsed = om::parse_observer_output(&result.text);
        // Capture the suggested-response steering for the observation-producing
        // actions (Observe/Buffer). A Reflect commit reuses save() too, but its
        // suggested-response is the reflector's, not an observation's.
        if let TurnEndAction::Observe { .. } | TurnEndAction::Buffer { .. } = action {
            parsed
                .suggested_response
                .clone_into(&mut self.record.om_suggested_response);
        }
        match action {
            TurnEndAction::Observe { .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_observation(store, &parsed.observations)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Buffer { .. } => {
                if !parsed.degenerate && !parsed.observations.trim().is_empty() {
                    self.apply_buffer(store, &parsed.observations)?;
                }
                *action = TurnEndAction::Done;
            }
            TurnEndAction::Reflect { level } => {
                let source = self.record.reflect_source().to_owned();
                // The committed suffix is the parsed <observations> content,
                // never the raw response (mastra `parseReflectorOutput`):
                // the <suggested-response> section is steering, not
                // observation material, and must not pollute the log.
                let parsed = om::parse_reflector_output(&result.text, Some(&source));
                if !parsed.degenerate
                    && !parsed.observations.trim().is_empty()
                    && om::validate_compression(&source, &parsed.observations)
                {
                    let new_suffix =
                        if om::parse_observation_groups(&parsed.observations).is_empty() {
                            om::wrap_in_observation_group(
                                &parsed.observations,
                                &om::combine_group_ranges(&om::parse_observation_groups(&source)),
                                &om::generate_group_id(&parsed.observations),
                                Some("reflection"),
                            )
                        } else {
                            parsed.observations
                        };
                    self.record.active_observations = new_suffix;
                    self.record.generation += 1;
                    self.record.observation_tokens =
                        om::token_count(&self.record.active_observations);
                    self.maintain_prefix_budget();
                    self.changed = true;
                    self.save(store)?;
                    *action = TurnEndAction::Done;
                } else if result.completed && result.mid_stream_errors.is_empty() {
                    // A well-formed completion that fails validation at
                    // this level: escalate (the cap is enforced here).
                    if *level < 4 {
                        *level += 1;
                    } else {
                        *action = TurnEndAction::Done;
                    }
                }
            }
            TurnEndAction::Done => {}
        }
        Ok(())
    }
    /// Apply one observation: wrap the unobserved entries in their
    /// provenance group, append it after the boundary, advance the cursor
    /// past them, persist (spec §4).
    fn apply_observation(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
    ) -> Result<(), OmError> {
        let entries = self.unobserved(store)?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let first = entries.first().unwrap_or(last);
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        self.record.active_observations =
            om::append_observation(&self.record.active_observations, &now_ms(), &wrapped);
        self.record.cursor = Some(Cursor {
            entry_id: last.id.clone(),
            timestamp: last.timestamp,
        });
        self.record.observation_tokens = om::token_count(&self.record.active_observations);
        self.record.pending_tokens = 0;
        self.changed = true;
        self.save(store)?;
        Ok(())
    }

    /// Apply one buffered observation over the entries NOT already covered
    /// by a previous run (the buffer cursor keeps runs disjoint, mastra's
    /// `lastBufferedAtTokens`): no observation-cursor advance and no save —
    /// the buffer is in memory until activation (the shared tail of the
    /// observe path).
    fn apply_buffer(
        &mut self,
        store: &mut SessionStore,
        observations: &str,
    ) -> Result<(), OmError> {
        let entries = self.unbuffered(store)?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let first = entries.first().unwrap_or(last);
        let range = format!("{}:{}", first.id, last.id);
        let id = om::generate_group_id(observations);
        let wrapped = om::wrap_in_observation_group(observations, &range, &id, None);
        let chunk = BufferedChunk {
            range: (first.id.clone(), last.id.clone()),
            last_ts: last.timestamp,
            text: wrapped,
            tokens: om::token_count(observations),
        };
        // The record is the durable chunk store (ticket #86 P2): the sync
        // path writes there too, so the turn-end write-back merge (which
        // re-syncs the in-memory mirror from the record) cannot drop the
        // chunk.
        self.record.buffered_chunks.push(chunk.clone());
        self.buffered.push(chunk);
        self.buffer_cursor = Some(last.id.clone());
        Ok(())
    }

    /// The raw entries not yet covered by a buffered run: after the buffer
    /// cursor, or the whole branch when no run has happened (the buffer
    /// cursor never lags the observation cursor — promotion advances the
    /// latter to the buffered material's end).
    pub(super) fn unbuffered(&self, store: &mut SessionStore) -> Result<Vec<Entry>, OmError> {
        let all = store.entries_range(0, usize::MAX)?;
        let leaf = store.leaf().map_err(OmError::Session)?.map(|e| e.id);
        let branch = branch_entries(&all, leaf.as_deref());
        let after = match &self.buffer_cursor {
            Some(id) => match branch.iter().position(|e| e.id == *id) {
                Some(i) => &branch[i + 1..],
                None => &branch[..],
            },
            None => &branch[..],
        };
        Ok(after.iter().filter(|e| is_raw(e)).cloned().collect())
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

    /// The context's system-prompt part (spec §4): the base prompt, the
    /// observation log (a demoted prefix drops out — it stays in the
    /// session file, reachable via `recall`), the active task's resume
    /// contract (ticket #24 fills the slot; the loop passes `None`
    /// today), and the continuation hint after a log change. Pure over the
    /// record (no store access), so the one-shot `changed` flip sticks
    /// when the loop runs it on the persistent state under the lock.
    pub fn assemble_context(&mut self, base: &str, task_contract: Option<&str>) -> String {
        let mut instructions = base.to_owned();
        let observations = self.record.agent_observations();
        if !observations.is_empty() {
            instructions.push_str("\n\n");
            instructions.push_str(om::OBSERVATION_CONTEXT_PROMPT);
            instructions.push('\n');
            instructions.push_str(om::OBSERVATION_CONTEXT_INSTRUCTIONS);
            instructions.push_str("\n\n");
            instructions.push_str(&observations);
        }
        if let Some(contract) = task_contract {
            instructions.push_str("\n\n# Task (resume contract)\n");
            instructions.push_str(contract);
        }
        if self.changed && !observations.is_empty() {
            self.changed = false;
            // One-shot steering: hand the observer's suggested-response to the
            // model alongside the hint (which says to follow it), then clear it
            // so a later reflect/promote can't re-inject a stale signal.
            let suggested = self.record.om_suggested_response.clone();
            self.record.om_suggested_response = String::new();
            instructions.push_str("\n\n<system-reminder>");
            instructions.push_str(om::OBSERVATION_CONTINUATION_HINT);
            if !suggested.trim().is_empty() {
                instructions.push_str("\n\nSuggested response: ");
                instructions.push_str(&suggested);
            }
            instructions.push_str("</system-reminder>");
        }
        instructions
    }

    /// The raw window over already-read entries (pure — no store
    /// access): the active-branch raw entries after the cursor (the
    /// unobserved tail), bounded by the observe threshold via promotion,
    /// not by a per-assembly floor prune. A leading tool run is cut so
    /// no result is orphaned from its call (spec §4).
    #[must_use]
    pub fn raw_window_from(&self, entries: &[Entry], leaf_id: Option<&str>) -> Vec<Entry> {
        let branch = branch_entries(entries, leaf_id);
        let unobserved = match self.record.cursor.as_ref() {
            Some(cursor) => match branch.iter().position(|e| e.id == cursor.entry_id) {
                Some(i) => &branch[i + 1..],
                None => &branch[..],
            },
            None => &branch[..],
        };
        let mut raw: Vec<Entry> = unobserved.iter().filter(|e| is_raw(e)).cloned().collect();
        // A window starting at a tool result would orphan it from its call:
        // advance past the leading tool run (spec §4 cut rule). No floor prune:
        // unsummarized raw stays in the window until promotion advances the cursor.
        while let Some(entry) = raw.first() {
            if entry.kind == "tool" {
                raw.remove(0);
            } else {
                break;
            }
        }
        raw
    }
}
