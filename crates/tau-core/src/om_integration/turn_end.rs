use super::*;

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
        self.changed = true;
        self.save(store)?;
        Ok(true)
    }

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
    /// The turn-end decision (pure — the agent loop reads the unobserved
    /// entries and updates the pending count under the session lock, then
    /// calls this outside it): reflect at the observation threshold,
    /// observe at activation, buffer at the increment (spec §4).
    pub fn plan(&mut self, unobserved: &[Entry]) -> TurnEndAction {
        if om::should_reflect(self.record.observation_tokens, &self.config) {
            return TurnEndAction::Reflect { level: 0 };
        }
        if self.activation_reached(self.record.pending_tokens) {
            return TurnEndAction::Observe {
                transcript: transcript(unobserved),
            };
        }
        if self.record.pending_tokens >= self.config.buffer_increment() {
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
                // the <current-task>/<suggested-response> sections are not
                // observation material and must not pollute the log.
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
        self.buffered.push(BufferedChunk {
            range: (first.id.clone(), last.id.clone()),
            last_ts: last.timestamp,
            text: wrapped,
            tokens: om::token_count(observations),
        });
        self.buffer_cursor = Some(last.id.clone());
        Ok(())
    }

    /// The raw entries not yet covered by a buffered run: after the buffer
    /// cursor, or the whole branch when no run has happened (the buffer
    /// cursor never lags the observation cursor — promotion advances the
    /// latter to the buffered material's end).
    fn unbuffered(&self, store: &mut SessionStore) -> Result<Vec<Entry>, OmError> {
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
        let observations = self.record.live_observations();
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
            instructions.push_str("\n\n<system-reminder>");
            instructions.push_str(om::OBSERVATION_CONTINUATION_HINT);
            instructions.push_str("</system-reminder>");
        }
        instructions
    }

    /// The raw window over already-read entries (pure — no store
    /// access): the active-branch entries after the cursor, pruned to
    /// the retention floor from the head, never cutting a tool result
    /// from its call (spec §4 cut rule).
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
        let floor = self.config.retention_floor() as u64;
        let total: u64 = raw
            .iter()
            .map(|e| om::token_count(&entry_text(e)) as u64)
            .sum();
        if total > floor {
            let mut keep_from = 0;
            let mut running = total;
            for (i, entry) in raw.iter().enumerate() {
                running = running.saturating_sub(om::token_count(&entry_text(entry)) as u64);
                keep_from = i + 1;
                if running <= floor {
                    break;
                }
            }
            raw = raw.split_off(keep_from);
            // A window starting at a tool result would orphan it from its
            // call: advance past the leading result run.
            while let Some(entry) = raw.first() {
                if entry.kind == "tool" {
                    raw.remove(0);
                } else {
                    break;
                }
            }
        }
        raw
    }
}
