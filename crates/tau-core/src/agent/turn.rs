use super::{
    AgentError, AgentSession, KIND_ASSISTANT, KIND_USER, Lane, Ordering, Queued, ResponseRequest,
    ToolBatchPolicy, TurnConfig, TurnResult, Value, lane_name,
};
use assemble::{input_items, repair_tool_call_arguments, with_plan_state};
use sink::StreamEntrySink;

impl AgentSession {
    /// Run turns until the queue is empty.
    pub async fn process(&self) -> Result<(), AgentError> {
        loop {
            let Some(starter) = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held")
                .queue
                .pop_front()
            else {
                return Ok(());
            };
            self.append_user(starter)?;
            self.run_turn().await?;
        }
    }

    /// The child's quiescence (ADR-0001, spec §5.2): a stop, or a
    /// `parent_notify` rest request pending the turn's end. Checked at
    /// every loop exit that would otherwise start another provider call.
    fn child_quiesced(&self) -> bool {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .child
            .as_ref()
            .is_some_and(|c| !c.is_running() || c.rest_requested())
    }

    #[allow(clippy::too_many_lines)] // one turn loop; splitting is refactoring
    async fn run_turn(&self) -> Result<(), AgentError> {
        // A force send or stop that targeted this turn is captured before the
        // call loop clears the per-call kill flag; it marks every segment of
        // the turn interrupted, not just the one cut mid-flight.
        let turn_killed = self.kill.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst);
        // A force/stop that landed before the turn applies to the first call
        // only; later calls in the turn are fresh.
        let mut first_call = true;
        // The turn's tool-call count (the gate's qualification: a turn-end
        // is gated only if the turn used ≥1 tool, spec §5.3) and the gate
        // repair attempts (bounded by `max_retries`).
        let mut tool_calls = 0usize;
        let mut gate_retries = 0u32;
        // The plan state (design: re-injected at turn boundaries and after
        // compaction only): captured once here and reused for every assembly
        // in the turn, so a mid-turn plan-file edit can't churn the provider
        // prefix cache; a mid-turn compaction re-captures it.
        let mut plan_state = self.plan_state();
        loop {
            // Steering and forced messages ride this LLM call (spec §7);
            // follow-ups wait for the turn boundary.
            let (delivered, queue_hook) = {
                let mut inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
                let mut out = Vec::new();
                inner.queue.retain(|m| {
                    if matches!(m.lane, Lane::Steering | Lane::Force) {
                        out.push(m.clone());
                        false
                    } else {
                        true
                    }
                });
                (out, inner.queue_event_hook.clone())
            };
            // A steering/force consumption is a queue change: let the app
            // re-emit the snapshot so the GUI's queue pane updates now, not
            // at the turn boundary.
            if let Some(hook) = queue_hook.as_deref().filter(|_| !delivered.is_empty()) {
                hook();
            }
            for msg in delivered {
                self.append_user(msg)?;
            }

            // The context (spec §4): with OM enabled, the system prompt
            // carries the observation log and the raw window is the
            // unobserved entries; without it, the session's entries as-is.
            // Everything is read under one lock, then assembled purely.
            let (system_prompt, input) = {
                let mut inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
                let base = inner.system_prompt.clone();
                let entries = inner
                    .store
                    .entries_range(0, usize::MAX)
                    .map_err(AgentError::Session)?;
                let leaf_id = inner
                    .store
                    .leaf()
                    .map_err(AgentError::Session)?
                    .map(|e| e.id);
                // Assembly runs on the persistent state, not a clone: it is
                // pure over the record, and the one-shot continuation-hint
                // flip must stick (a clone's flip would be dropped, and the
                // om_turn_end write-back would re-set `changed`, so the hint
                // would re-inject on every assembly).
                if let Some(om) = inner.om.as_mut() {
                    let assembled = om.assemble_context(&base);
                    let raw = om.raw_window_from(&entries, leaf_id.as_deref());
                    let input = assembled
                        .om_input_items
                        .into_iter()
                        .chain(input_items(&raw))
                        .collect();
                    (
                        with_plan_state(assembled.instructions, plan_state.as_deref()),
                        input,
                    )
                } else {
                    (
                        with_plan_state(base, plan_state.as_deref()),
                        input_items(&entries),
                    )
                }
            };
            let turn = self.turn_config();
            let tools = self.tools();
            // The soft prompt size for the output-cap clamp (spec §12, #35):
            // estimated before `input` moves into the request.
            let prompt = crate::provider::prompt_token_estimate(&system_prompt, &input, &tools);
            let mut request =
                ResponseRequest::new(self.model().clone(), Some(system_prompt.as_str()), input)
                    .with_tools(tools);
            if let Some(n) = turn.max_output_tokens {
                request = request.with_max_output_tokens(crate::provider::clamp_max_output(
                    n,
                    turn.context_window,
                    prompt,
                ));
            }
            if let Some(e) = turn.reasoning {
                request = request.with_reasoning(e);
                if turn.reasoning_summary {
                    request = request.with_reasoning_summary();
                }
            }
            if let Some(t) = turn.temperature {
                request = request.with_temperature(t);
            }
            if let Some(p) = turn.top_p {
                request = request.with_top_p(p);
            }
            if let Some(p) = turn.frequency_penalty {
                request = request.with_frequency_penalty(p);
            }
            if let Some(p) = turn.presence_penalty {
                request = request.with_presence_penalty(p);
            }
            if let Some((key, retention)) = &turn.prompt_cache {
                request = request.with_prompt_cache(key.clone(), *retention);
            }

            // A force has already killed the stream it targeted; every
            // fresh call starts un-killed (spec §7).
            self.kill.store(false, Ordering::SeqCst);
            // The call's assistant entry is minted up front (ADR-0008):
            // the streamed snapshots and the final append share the one id.
            let (entry_id, parent, timestamp, upsert) = {
                let mut inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
                let parent = inner.store.leaf().ok().flatten().map(|e| e.id);
                (
                    inner.store.mint_id(),
                    parent,
                    inner.store.now(),
                    inner.entry_upsert_hook.clone(),
                )
            };
            let mut sink = StreamEntrySink {
                kill: self.kill.clone(),
                stop: self.stop.clone(),
                id: entry_id.clone(),
                parent,
                timestamp,
                upsert,
                text: String::new(),
                reasoning: String::new(),
                last: None,
            };
            let mut result = self.provider.call(&request, &mut sink).await?;
            // ticket #70: repair malformed tool-call arguments at receipt, so
            // the stored entry + echoed history are valid JSON.
            repair_tool_call_arguments(&mut result);
            self.append_assistant(&entry_id, &result, turn_killed && first_call)?;
            first_call = false;

            if !result.completed {
                // A force-killed turn skips the OM pass: the Observer needs
                // no tool batch in flight, and the interrupted material
                // joins the next turn's unobserved window (v0).
                // Per the policy, the in-flight tool batch is let to
                // complete, then one final call so the model sees the tool
                // results alongside the forced message (spec §7).
                if self.tool_batch_on_force() == ToolBatchPolicy::Complete
                    && !result.calls.is_empty()
                {
                    self.run_tools(&result.calls).await?;
                    tool_calls += result.calls.len();
                    self.om_mid_loop();
                    // A mid-turn compaction is a re-injection point (design).
                    plan_state = self.plan_state();
                    // Quiescence (ADR-0001, spec §5.2): a rest request or
                    // a stop that landed with the tool batch ends the turn
                    // here — the fast path burns no final call for a child
                    // that already declared its rest.
                    if self.child_quiesced() {
                        break;
                    }
                    continue;
                }
                break;
            }
            if result.calls.is_empty() {
                // The model ended the turn: the synchronous OM pass runs
                // before any follow-up starts the next one (spec §4).
                self.om_turn_end().await?;
                // The completion gate (spec §5.3): a qualifying turn-end
                // (≥1 tool call, no QUESTION: marker) runs the check. A
                // pass stands the turn; a fail records a system entry,
                // bounces a steering-lane repair turn, and — after
                // max_retries — marks the session gate_failed.
                let end = self
                    .gate_turn_end(&result.text, tool_calls, &mut gate_retries)
                    .await?;
                if end {
                    break;
                }
                // A failed check bounced a repair: continue the loop so the
                // injected steering message rides the next LLM call.
                continue;
            }
            self.run_tools(&result.calls).await?;
            tool_calls += result.calls.len();
            self.om_mid_loop();
            // A mid-turn compaction is a re-injection point (design): the
            // next assembly carries the refreshed state.
            plan_state = self.plan_state();
            if self.child_quiesced() {
                // Quiescence (ADR-0001): a stop, or a `parent_notify`
                // `done` declaration (the fast path — spec §5.2), ends the
                // turn here, so the core auto-terminates the loop and the
                // observed rest at the turn end settles the state without
                // burning another provider call.
                break;
            }
        }
        // A force/stop that cut this turn is consumed here: it must not bleed
        // into the next turn (the one a force send starts).
        self.kill.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// The watcher-parsed plan state (design: the plan file is the source of
    /// truth), read under the session lock. Captured once per turn and
    /// re-captured after a mid-turn compaction — the re-injection points —
    /// not on every assembly, so a mid-turn plan-file edit can't churn the
    /// provider prefix cache for the rest of the turn.
    fn plan_state(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .plan_hook
            .as_ref()
            .and_then(|h| h())
    }

    fn append_user(&self, msg: Queued) -> Result<(), AgentError> {
        let skill = msg
            .skill
            .as_ref()
            .map(|(name, location)| tau_protocol::payload::SkillRef {
                name: name.clone(),
                location: location.clone(),
            });
        let payload = tau_protocol::payload::UserPayload {
            text: msg.text,
            lane: lane_name(msg.lane).to_owned(),
            source: msg.source,
            skill,
        }
        .to_value();
        // A notified steering's id was minted at queue time (its upsert
        // already fired): the line lands now, under that id, after the
        // in-flight entry it was queued behind (#148).
        match msg.entry_id {
            Some(id) => self.append_id(&id, KIND_USER, payload),
            None => self.append(KIND_USER, payload),
        }
    }

    fn append_assistant(
        &self,
        id: &str,
        result: &TurnResult,
        turn_forced: bool,
    ) -> Result<(), AgentError> {
        // An empty, call-less assistant entry has nothing to record
        // (review N10), cut or completed: no content is no entry.
        if result.text.is_empty() && result.reasoning.is_empty() && result.calls.is_empty() {
            return Ok(());
        }
        self.append_id(
            id,
            KIND_ASSISTANT,
            tau_protocol::payload::AssistantPayload {
                text: result.text.clone(),
                reasoning: result.reasoning.clone(),
                // Interrupted means the user cut the turn (stop button or a
                // force send) -- not that the provider omitted [DONE], which
                // some do intermittently on tool-call segments of a turn that
                // ran to completion. `turn_killed` covers a stop that landed
                // before the turn; the live flags cover one mid-flight.
                interrupted: turn_forced
                    || self.kill.load(Ordering::SeqCst)
                    || self.stop.load(Ordering::SeqCst)
                    || self.closed.load(Ordering::SeqCst),
                usage: result.usage.clone(),
                calls: result.calls.clone(),
            }
            .to_value(),
        )
    }

    pub(super) fn append(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        let id = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .store
            .mint_id();
        self.append_id(&id, kind, payload)
    }

    /// Append under a pre-minted id (ADR-0008): the re-emitted item keeps
    /// the id its first emission carried.
    pub(super) fn append_id(&self, id: &str, kind: &str, payload: Value) -> Result<(), AgentError> {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        // A fresh session has no leaf: the first entry starts the branch.
        // Any other failure is a storage error and propagates.
        let parent = match inner.store.leaf() {
            Ok(leaf) => leaf.map(|e| e.id),
            Err(e) => return Err(AgentError::Session(e)),
        };
        let parent = parent.as_deref();
        inner.store.append_entry(id, kind, payload, parent)?;
        Ok(())
    }

    fn tool_batch_on_force(&self) -> ToolBatchPolicy {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .tool_batch_on_force
    }

    fn turn_config(&self) -> TurnConfig {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .turn
            .clone()
    }
}

mod assemble;
mod om_pass;
mod sink;
mod tools;

#[cfg(test)]
mod tests;
