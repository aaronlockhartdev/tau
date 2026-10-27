use super::{
    AgentError, AgentSession, InputEntry, Lane, Ordering, Queued, ResponseRequest, ToolBatchPolicy,
    TurnConfig, TurnResult, Value, lane_name,
};
use crate::provider::{StreamOutcome, ToolSpec};
use assemble::{Assembled, input_items, repair_tool_call_arguments, with_plan_state};
use estimate::{PromptAnchor, prompt_size, tool_fingerprint};
use sink::StreamEntrySink;
use tau_protocol::{Diagnostic, EntryKind};

/// The cap on the provider's body inside a `provider_rejected` record. That
/// body is an untrusted `response.text()` — a proxy's HTML error page is
/// megabytes, and only its head distinguishes the failure. The gate caps its
/// output for the same reason (`gate::MAX_OUTPUT`).
pub(crate) const REJECTION_BODY_CAP: usize = 2_000;

impl AgentSession {
    /// Run turns until the queue is empty. A stop (ticket #223) ends the
    /// drain at the next turn boundary: the in-flight turn cuts, and the
    /// remaining queued messages stay queued for the next explicit send —
    /// they are not turned into interrupted one-call turns.
    pub async fn process(&self) -> Result<(), AgentError> {
        loop {
            // A stop set while the previous turn ran (or before the first):
            // drain nothing further.
            if self.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let Some(starter) = self.inner.lock().queue.pop_front() else {
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
        // The prompt estimate walks toward the truth (plan §6.4): the provider's
        // count for the request the turn last sent, so only material appended
        // after it is ever a guess. It is set after the first completed call.
        let mut anchor: Option<PromptAnchor> = None;
        loop {
            // Steering and forced messages ride this LLM call (spec §7);
            // follow-ups wait for the turn boundary.
            let (delivered, queue_hook) = {
                let mut inner = self.inner.lock();
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

            let turn = self.turn_config();
            let tools = self.tools();
            // The context (spec §4): with OM enabled, the system prompt
            // carries the observation log and the raw window is the
            // unobserved entries; without it, the session's entries as-is.
            let mut assembled = self.assemble(plan_state.as_deref())?;
            let tools_json = tool_fingerprint(&tools);
            let mut prompt = prompt_size(anchor.as_ref(), &assembled, &tools_json);
            // The context gate (ticket #210, spec §4): when the assembled
            // prompt would exceed the safe budget and there is unobserved
            // material to compact, run the OM pass now so the call fits.
            let budget = turn.context_gate().budget();
            if let Some(budget) = budget
                && prompt > budget
            {
                if self.om_pending_tokens() > 0 {
                    self.om_turn_end().await?;
                    // The compaction replaced the prefix an anchored count
                    // would describe, so this call is priced whole.
                    // A mid-turn compaction is a re-injection point (design).
                    plan_state = self.plan_state();
                    assembled = self.assemble(plan_state.as_deref())?;
                    prompt = prompt_size(None, &assembled, &tools_json);
                    if prompt > budget {
                        tracing::warn!(
                            prompt,
                            budget,
                            "context gate: prompt still over budget after OM pass"
                        );
                    }
                } else {
                    // The gate's blind spot (#255): over budget with no
                    // unobserved material to compact. Refuse loudly before
                    // the wire instead of sending a doomed 400, and leave a
                    // durable trace of the refusal.
                    self.diagnostic(&Diagnostic::ContextOverflow {
                        prompt: u64::from(prompt),
                        budget: u64::from(budget),
                    })?;
                    return Err(AgentError::ContextOverflow { prompt, budget });
                }
            }
            let request = self.build_request(
                &assembled.instructions,
                &assembled.items,
                &tools,
                &turn,
                prompt,
                None,
            );

            // A force has already killed the stream it targeted; every
            // fresh call starts un-killed (spec §7).
            self.kill.store(false, Ordering::SeqCst);
            // The call's assistant entry is minted up front (ADR-0008):
            // the streamed snapshots and the final append share the one id.
            let (entry_id, parent, timestamp, upsert) = {
                let mut inner = self.inner.lock();
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
            let mut result = match self.provider.call(&request, &mut sink).await {
                Ok(result) => result,
                // A prefill rejection: the sink is untouched, so the same
                // one may carry the retry's stream (the #256 recovery).
                Err(e) => {
                    self.recover_from_provider_error(
                        e,
                        &mut sink,
                        &assembled.instructions,
                        &assembled.items,
                        &tools,
                        &turn,
                        prompt,
                    )
                    .await?
                }
            };
            // ticket #70: repair malformed tool-call arguments at receipt, so
            // the stored entry + echoed history are valid JSON.
            repair_tool_call_arguments(&mut result);
            // Two separate facts, decided apart: whether the *user* cut the turn
            // (stop button, force send — #229) and how the *provider* ended the
            // response (#307). A stop the user asked for already rides the entry
            // as `interrupted`; only a cut nobody asked for is a cutoff.
            let cut_by_user = (turn_killed && first_call)
                || self.kill.load(Ordering::SeqCst)
                || self.stop.load(Ordering::SeqCst)
                || self.closed.load(Ordering::SeqCst);
            self.append_assistant(&entry_id, &result, cut_by_user)?;
            if let Some(ended_by) = reported_cutoff(result.outcome, cut_by_user) {
                self.diagnostic(&Diagnostic::TurnCutoff {
                    model: self.model(),
                    ended_by,
                    blocks: settled_blocks(&result),
                })?;
            }

            // The next call anchors on what the provider charged for this one
            // (plan §6.4). A call that never completed, or one whose provider
            // reported no usage, leaves the estimate unanchored — the honest
            // answer, since there is no recorded count to trust.
            anchor = match (&result.usage, result.outcome) {
                (Some(usage), StreamOutcome::Completed) => {
                    PromptAnchor::after_call(&assembled, &tools_json, usage.input_tokens)
                }
                _ => None,
            };
            first_call = false;

            if result.outcome != StreamOutcome::Completed {
                // A turn the provider or the user cut short skips the OM pass:
                // the Observer needs
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
                // pass stands the turn; a fail records a gate_attempt,
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
    /// One provider request, assembled from the turn's parts: the output cap
    /// is clamped to the declared window (spec §12), and a
    /// `max_output_override` stands in for the turn's cap — the one-shot
    /// recovery path (#256).
    fn build_request(
        &self,
        system_prompt: &str,
        input: &[InputEntry],
        tools: &[ToolSpec],
        turn: &TurnConfig,
        prompt: u32,
        max_output_override: Option<u64>,
    ) -> ResponseRequest {
        // The #98 wire log's delta view: stamp the offset of the items
        // already logged by this session's previous request, then advance
        // it — under the session lock, so a mid-turn re-assembly after an
        // OM compaction logs its (shrunken) window, not a stale offset.
        // The lock is the cost, so it's taken only where `debug!` is live:
        // `level_enabled!` folds to a compile-time const under a `max_level_*`
        // feature, so a release build skips it and `prev_len` stays 0.
        let prev_len = if tracing::level_enabled!(tracing::Level::DEBUG) {
            let mut inner = self.inner.lock();
            let prev = inner.prev_input_len;
            inner.prev_input_len = input.len();
            prev
        } else {
            0
        };
        let address = turn.request_address(&self.model());
        let mut request = ResponseRequest::new(address, Some(system_prompt), input.to_vec())
            .with_tools(tools.to_vec())
            .with_wire_delta(prev_len);
        if let Some(n) = max_output_override.or(turn.max_output_tokens) {
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
        request
    }

    /// A provider rejection at call time: a context-length 400 gets one
    /// bounded retry at a halved `max_output_tokens` (#256) — the server
    /// computes the window remainder from that cap. Anything else (and a
    /// second context 400) is recorded as a `provider_rejected` record with the
    /// provider's status and body, then propagates.
    #[allow(clippy::too_many_arguments)] // the turn's parts, threaded once
    async fn recover_from_provider_error(
        &self,
        e: crate::provider::ProviderError,
        sink: &mut StreamEntrySink,
        system_prompt: &str,
        input: &[InputEntry],
        tools: &[ToolSpec],
        turn: &TurnConfig,
        prompt: u32,
    ) -> Result<TurnResult, AgentError> {
        let mut fatal = e;
        if let Some(cap) = turn.max_output_tokens
            && fatal.is_context_length_error()
        {
            // One bounded retry (no loop) at a halved cap, floored so the
            // recovery never degrades to a degenerate window.
            let retry = (cap / 2).max(256);
            let request =
                self.build_request(system_prompt, input, tools, turn, prompt, Some(retry));
            match self.provider.call(&request, sink).await {
                Ok(result) => return Ok(result),
                Err(retry_err) => fatal = retry_err,
            }
        }
        self.record_rejection(&fatal)?;
        Err(AgentError::Provider(fatal))
    }

    /// The durable trace of a rejected call (#256): a diagnostic record
    /// carrying the provider's status and body.
    fn record_rejection(&self, e: &crate::provider::ProviderError) -> Result<(), AgentError> {
        match e {
            crate::provider::ProviderError::Status { status, body } => {
                self.diagnostic(&Diagnostic::ProviderRejected {
                    status: *status,
                    body: crate::truncate::head(body, REJECTION_BODY_CAP),
                })
            }
            // A dead stream or a transport failure is the turn's error to
            // surface; the record channel carries what the provider answered.
            crate::provider::ProviderError::Request(_)
            | crate::provider::ProviderError::IdleTimeout
            | crate::provider::ProviderError::MalformedStream(_) => Ok(()),
        }
    }

    /// Assemble the system prompt and input items for a provider call
    /// (spec §4). With OM enabled the system prompt carries the observation
    /// log and the input is the unobserved raw window; without it the
    /// session's entries are used as-is. Everything is read under one lock,
    /// then assembled purely.
    fn assemble(&self, plan_state: Option<&str>) -> Result<Assembled, AgentError> {
        let mut inner = self.inner.lock();
        let base = inner.system_prompt.clone();
        let model = inner.model.clone();
        // A child session is labeled at assembly against its
        // parent (the link names it); a plain session is not.
        let parent_session = inner.child.as_ref().map(|c| c.parent_session().to_owned());
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
            let om_items = assembled.om_input_items.len();
            let items = assembled
                .om_input_items
                .into_iter()
                .chain(input_items(&raw, parent_session.as_deref()))
                .collect();
            Ok(Assembled {
                model,
                instructions: with_plan_state(assembled.instructions, plan_state),
                items,
                entries,
                om_items,
                parent_session,
            })
        } else {
            Ok(Assembled {
                model,
                instructions: with_plan_state(base, plan_state),
                items: input_items(&entries, parent_session.as_deref()),
                entries,
                om_items: 0,
                parent_session,
            })
        }
    }

    /// The watcher-parsed plan state (design: the plan file is the source of
    /// truth), read under the session lock. Captured once per turn and
    /// re-captured after a mid-turn compaction — the re-injection points —
    /// not on every assembly, so a mid-turn plan-file edit can't churn the
    /// provider prefix cache for the rest of the turn.
    fn plan_state(&self) -> Option<String> {
        self.inner.lock().plan_hook.as_ref().and_then(|h| h())
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
            Some(id) => self.append_id(&id, EntryKind::User, payload),
            None => self.append(EntryKind::User, payload),
        }
    }

    fn append_assistant(
        &self,
        id: &str,
        result: &TurnResult,
        cut_by_user: bool,
    ) -> Result<(), AgentError> {
        // An empty, call-less assistant entry has nothing to record
        // (review N10), cut or completed: no content is no entry.
        if result.text.is_empty() && result.reasoning.is_empty() && result.calls.is_empty() {
            return Ok(());
        }
        self.append_id(
            id,
            EntryKind::Assistant,
            tau_protocol::payload::AssistantPayload {
                text: result.text.clone(),
                reasoning: result.reasoning.clone(),
                // Interrupted means the user cut the turn (stop button or a
                // force send) -- not that the provider omitted [DONE], which
                // some do intermittently on tool-call segments of a turn that
                // ran to completion.
                interrupted: cut_by_user,
                // How the provider's response ended, recorded on every turn
                // (#307): a cap or a dropped stream is visible on the record.
                stop_reason: Some(result.outcome),
                usage: result.usage.clone(),
                duration_secs: result.duration.map(|d| d.as_secs_f64()),
                calls: result.calls.clone(),
            }
            .to_value(),
        )
    }

    pub(super) fn append(&self, kind: EntryKind, payload: Value) -> Result<(), AgentError> {
        let id = self.inner.lock().store.mint_id();
        self.append_id(&id, kind, payload)
    }

    /// Append under a pre-minted id (ADR-0008): the re-emitted item keeps
    /// the id its first emission carried.
    pub(super) fn append_id(
        &self,
        id: &str,
        kind: EntryKind,
        payload: Value,
    ) -> Result<(), AgentError> {
        let mut inner = self.inner.lock();
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
        self.inner.lock().tool_batch_on_force
    }

    fn turn_config(&self) -> TurnConfig {
        self.inner.lock().turn.clone()
    }
}

/// The endings that are a provider-side cutoff: neither a clean completion nor
/// a stop the user asked for (#307).
fn cutoff(outcome: StreamOutcome) -> Option<StreamOutcome> {
    match outcome {
        StreamOutcome::Completed | StreamOutcome::Killed => None,
        other => Some(other),
    }
}

/// The cutoff a turn reports to the record. A user's stop outranks a provider's
/// ending: the entry already carries `interrupted`, and two entries naming one
/// cause is how the record starts contradicting itself (#307, plan §6.11).
pub(crate) fn reported_cutoff(outcome: StreamOutcome, cut_by_user: bool) -> Option<StreamOutcome> {
    if cut_by_user { None } else { cutoff(outcome) }
}

/// How much of the response reached the record: the text and reasoning runs,
/// plus each call. `TurnResult::blocks` is a debug-build wire audit, so the
/// count comes from what an assistant entry would actually carry.
fn settled_blocks(result: &TurnResult) -> u64 {
    u64::from(!result.text.is_empty())
        + u64::from(!result.reasoning.is_empty())
        + u64::try_from(result.calls.len()).unwrap_or(u64::MAX)
}

mod assemble;
mod estimate;
mod om_pass;
mod sink;
mod tools;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_anchor;
