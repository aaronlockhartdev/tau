use super::{
    AgentError, AgentSession, Arc, AtomicBool, CallOutput, Entry, EntryEventHook, FunctionCall,
    FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage, KIND_ASSISTANT,
    KIND_SYSTEM, KIND_TOOL, KIND_USER, Lane, MAX_ROUNDS, Ordering, Queued, ResponseRequest,
    SessionStore, ToolBatchPolicy, TurnConfig, TurnEvent, TurnResult, TurnSink, Value, lane_name,
    tools,
};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
impl AgentSession {
    /// Run turns until the queue is empty.
    pub async fn process(&self) -> Result<(), AgentError> {
        loop {
            let Some(starter) = self.inner.lock().unwrap().queue.pop_front() else {
                return Ok(());
            };
            self.append_user(starter)?;
            self.run_turn().await?;
        }
    }

    #[allow(clippy::too_many_lines)] // one turn loop; splitting is refactoring
    async fn run_turn(&self) -> Result<(), AgentError> {
        let mut rounds = 0usize;
        // A force send or stop that targeted this turn is captured before the
        // call loop clears the per-call kill flag; it marks every segment of
        // the turn interrupted, not just the one cut mid-flight.
        let turn_killed = self.kill.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst);
        // A force/stop that landed before the turn applies to the first call
        // only; later calls in the turn are fresh.
        let mut first_call = true;
        loop {
            rounds += 1;
            if rounds > MAX_ROUNDS {
                // A runaway turn dies visibly, not silently: the session
                // records why it stopped.
                self.append(
                    KIND_SYSTEM,
                    tau_protocol::payload::SystemPayload {
                        note: format!(
                            "Stopped after {MAX_ROUNDS} tool rounds without the model ending the turn"
                        ),
                    }
                    .to_value(),
                )?;
                break;
            }
            // Steering and forced messages ride this LLM call (spec §7);
            // follow-ups wait for the turn boundary.
            let (delivered, queue_hook) = {
                let mut inner = self.inner.lock().unwrap();
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
                let mut inner = self.inner.lock().unwrap();
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
                // The active tasks' resume contracts (spec §5.3): re-injected
                // into every assembly while active, so a compaction can
                // never make a session lose sight of them. All active tasks
                // ride (bounded) — a session may work several (review N4).
                let contracts = crate::task::active_tasks(&crate::task::fold_entries(&entries))
                    .iter()
                    .take(5)
                    .map(|t| {
                        serde_json::to_string(&crate::task::resume_contract(t)).unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let contract = if contracts.is_empty() {
                    None
                } else {
                    Some(contracts)
                };
                // Assembly runs on the persistent state, not a clone: it is
                // pure over the record, and the one-shot continuation-hint
                // flip must stick (a clone's flip would be dropped, and the
                // om_turn_end write-back would re-set `changed`, so the hint
                // would re-inject on every assembly).
                if let Some(om) = inner.om.as_mut() {
                    let instructions = om.assemble_context(&base, contract.as_deref());
                    let raw = om.raw_window_from(&entries, leaf_id.as_deref());
                    (instructions, input_items(&raw))
                } else {
                    let mut instructions = base;
                    if let Some(contract) = &contract {
                        instructions.push_str("\n\n# Task (resume contract)\n");
                        instructions.push_str(contract);
                    }
                    (instructions, input_items(&entries))
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
                let mut inner = self.inner.lock().unwrap();
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
            let result = self.provider.call(&request, &mut sink).await?;
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
                    continue;
                }
                break;
            }
            if result.calls.is_empty() {
                // The model ended the turn: the synchronous OM pass runs
                // before any follow-up starts the next one (spec §4).
                self.om_turn_end().await?;
                break;
            }
            self.run_tools(&result.calls).await?;
            if self
                .inner
                .lock()
                .unwrap()
                .child
                .as_ref()
                .is_some_and(|c| !c.is_running())
            {
                // Quiescence (ADR-0001): the child's notify ended it, so the
                // core auto-terminates its loop — a done child never burns
                // another provider call.
                break;
            }
        }
        // A force/stop that cut this turn is consumed here: it must not bleed
        // into the next turn (the one a force send starts).
        self.kill.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn run_tools(&self, calls: &[FunctionCall]) -> Result<(), AgentError> {
        for call in calls {
            let args = serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null);
            let tc = tools::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args: args.clone(),
            };
            // One id per call (ADR-0008): the call phase re-emits the entry
            // wire-only (empty output); the result persists the same id, so
            // the file keeps one line for the call.
            let (entry_id, parent, timestamp, upsert) = {
                let mut inner = self.inner.lock().unwrap();
                let parent = inner.store.leaf().ok().flatten().map(|e| e.id);
                (
                    inner.store.mint_id(),
                    parent,
                    inner.store.now(),
                    inner.entry_upsert_hook.clone(),
                )
            };
            if let Some(hook) = &upsert {
                hook(&Entry {
                    id: entry_id.clone(),
                    parent,
                    kind: KIND_TOOL.to_owned(),
                    timestamp,
                    payload: tau_protocol::payload::ToolPayload {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        args: args.clone(),
                        output: tau_protocol::payload::ToolOutput::Text(String::new()),
                    }
                    .to_value(),
                    blob: None,
                    first_kept_entry_id: None,
                    crc: None,
                });
            }
            // The surface routes the call (R1): the loop matches no tool
            // names — the table in tools::surface owns the mapping. The
            // links are cloned under the lock and invoked outside it: the
            // routes re-lock this session's `inner` (a child notify
            // records a state entry; a spawn reads the parent's OM
            // record), and the lock is not reentrant.
            let (sup, link, cwd, image_max) = {
                let inner = self.inner.lock().unwrap();
                (
                    inner.subagents.clone(),
                    inner.child.clone(),
                    inner.cwd.clone(),
                    inner.turn.image_max_bytes,
                )
            };
            let output = tools::surface::dispatch(self, &cwd, image_max, sup, link, &tc).await;
            self.append_id(
                &entry_id,
                KIND_TOOL,
                tau_protocol::payload::ToolPayload {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    args,
                    output,
                }
                .to_value(),
            )?;
        }
        Ok(())
    }

    /// The turn-end OM pass (ticket #22, R4): one call — `settle_turn`
    /// owns the whole sequence (the unobserved read, activation, the plan,
    /// the observe/reflect round-trips, the commit). The state runs on a
    /// clone and is written back at the end; the store is handed to it
    /// phase by phase through the re-lock, so the session lock is never
    /// held across a provider call (a std guard cannot cross an await in a
    /// spawned future).
    async fn om_turn_end(&self) -> Result<(), AgentError> {
        // The OM model (global config, spec §4); empty = the session's
        // model. Resolved with the state clone, before the pass.
        let (state, model, hook) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.om.clone(),
                if inner.om_model.is_empty() {
                    inner.model.clone()
                } else {
                    inner.om_model.clone()
                },
                inner.om_status_hook.clone(),
            )
        };
        let Some(mut state) = state else {
            return Ok(());
        };
        // The re-lock: each sync store phase takes the session lock and
        // releases it before the next provider call.
        let mut with_store =
            |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), crate::om_integration::OmError>| {
                let mut guard = self.inner.lock().unwrap();
                f(&mut guard.store)
            };
        state
            .settle_turn(&mut with_store, &self.provider, &model, hook.as_ref())
            .await
            .map_err(AgentError::Om)?;
        self.inner.lock().unwrap().om = Some(state);
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // one append pass; splitting is refactoring
    fn append_user(&self, msg: Queued) -> Result<(), AgentError> {
        // A steering report was already appended at queue time (so the GUI
        // showed it immediately); don't append it a second time here.
        if msg.in_file {
            return Ok(());
        }
        let skill = msg
            .skill
            .as_ref()
            .map(|(name, location)| tau_protocol::payload::SkillRef {
                name: name.clone(),
                location: location.clone(),
            });
        self.append(
            KIND_USER,
            tau_protocol::payload::UserPayload {
                text: msg.text,
                lane: lane_name(msg.lane).to_owned(),
                source: msg.source,
                skill,
            }
            .to_value(),
        )
    }

    fn append_assistant(
        &self,
        id: &str,
        result: &TurnResult,
        turn_forced: bool,
    ) -> Result<(), AgentError> {
        // A partial cut before anything arrived has nothing to record
        // (review N10): an empty, call-less interrupted entry is noise.
        if !result.completed
            && result.text.is_empty()
            && result.reasoning.is_empty()
            && result.calls.is_empty()
        {
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
        let id = self.inner.lock().unwrap().store.mint_id();
        self.append_id(&id, kind, payload)
    }

    /// Append under a pre-minted id (ADR-0008): the re-emitted item keeps
    /// the id its first emission carried.
    pub(super) fn append_id(&self, id: &str, kind: &str, payload: Value) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
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
        self.inner.lock().unwrap().tool_batch_on_force
    }

    fn turn_config(&self) -> TurnConfig {
        self.inner.lock().unwrap().turn.clone()
    }
}

/// The sink the loop gives the provider: the per-call kill flag (force)
/// or the persistent stop flag (ticket #23) stops the stream (spec §7).
/// It also accumulates the streamed text and re-emits it as the growing
/// assistant entry (ADR-0008's wire-only snapshots), at most one per
/// display frame.
struct StreamEntrySink {
    kill: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    id: String,
    parent: Option<String>,
    timestamp: u64,
    upsert: Option<EntryEventHook>,
    text: String,
    reasoning: String,
    last: Option<Instant>,
}

/// The snapshot cadence: one re-emission per display frame (ADR-0008).
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(16);

impl TurnSink for StreamEntrySink {
    fn event(&mut self, event: TurnEvent) -> bool {
        if self.kill.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        match &event {
            TurnEvent::Text(t) => {
                self.text.push_str(t);
                self.emit_snapshot();
            }
            TurnEvent::Reasoning(t) => {
                self.reasoning.push_str(t);
                self.emit_snapshot();
            }
            // Completed carries no content; the final append's upsert is
            // the stream's last emission.
            TurnEvent::Completed(_) => {}
        }
        true
    }
    fn stop_signal(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(stop_flags(&self.kill, &self.stop))
    }
}

impl StreamEntrySink {
    /// The frame-aligned re-emission: at most one snapshot per window, the
    /// first token opening the card.
    fn emit_snapshot(&mut self) {
        let Some(hook) = &self.upsert else {
            return;
        };
        let now = Instant::now();
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < SNAPSHOT_INTERVAL)
        {
            return;
        }
        self.last = Some(now);
        let entry = Entry {
            id: self.id.clone(),
            parent: self.parent.clone(),
            kind: KIND_ASSISTANT.to_owned(),
            timestamp: self.timestamp,
            payload: tau_protocol::payload::AssistantPayload {
                text: self.text.clone(),
                reasoning: self.reasoning.clone(),
                interrupted: false,
                usage: None,
                calls: vec![],
            }
            .to_value(),
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        };
        hook(&entry);
    }
}
/// The stop-flag poll cadence: a human clicking stop, not a tight race.
const STOP_POLL_MS: u64 = 50;

/// The prefill half of a kill/stop (spec §7): a poll over the flags so the
/// provider tears the in-flight request down before the first stream
/// event arrives.
async fn stop_flags(kill: &AtomicBool, stop: &AtomicBool) {
    loop {
        if kill.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(STOP_POLL_MS)).await;
    }
}

/// The session's entries (file order) mapped to responses-API input items:
/// user messages, assistant text + calls, and tool results (spec §5.4).
fn input_items(entries: &[Entry]) -> Vec<InputEntry> {
    let mut out = Vec::new();
    for entry in entries {
        match entry.kind.as_str() {
            KIND_USER => {
                if let Some(text) = entry.payload.get("text").and_then(Value::as_str) {
                    // A wake message from a child (ticket #23) is stored as a
                    // user entry with its source: the prefix keeps the parent
                    // from reading the child's words as the user's.
                    let content = match entry.payload.get("source").and_then(Value::as_str) {
                        Some(source) => format!("Sub-agent {source}: {text}"),
                        None => text.to_owned(),
                    };
                    out.push(InputEntry::Message(InputMessage {
                        role: "user".into(),
                        content,
                    }));
                }
            }
            KIND_ASSISTANT => {
                if let Some(text) = entry
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    out.push(InputEntry::Message(InputMessage {
                        role: "assistant".into(),
                        content: text.to_owned(),
                    }));
                }
                if let Some(calls) = entry.payload.get("calls").and_then(Value::as_array) {
                    for call in calls {
                        out.push(InputEntry::Call(FunctionCallInput {
                            kind: "function_call",
                            id: call
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            call_id: call
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            name: call
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            arguments: call
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        }));
                    }
                }
            }
            KIND_TOOL => {
                let (Some(call_id), Some(output)) = (
                    entry.payload.get("call_id").and_then(Value::as_str),
                    entry
                        .payload
                        .get("output")
                        .and_then(CallOutput::from_payload),
                ) else {
                    continue;
                };
                // The call-phase entry (recorded before dispatch, empty
                // output) is not an input item: its pair's result entry
                // carries the output for the same call_id.
                if matches!(&output, CallOutput::Text(t) if t.is_empty()) {
                    continue;
                }
                out.push(InputEntry::CallOutput(FunctionCallOutputInput {
                    kind: "function_call_output",
                    call_id: call_id.to_owned(),
                    output,
                }));
            }
            _ => {}
        }
    }
    out
}
