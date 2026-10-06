use super::{
    AgentError, AgentSession, Arc, AtomicBool, CallOutput, Entry, EntryEventHook, FunctionCall,
    FunctionCallInput, FunctionCallOutputInput, Inner, InputEntry, InputMessage, KIND_ASSISTANT,
    KIND_TOOL, KIND_USER, Lane, Ordering, Queued, ResponseRequest, SessionStore, ToolBatchPolicy,
    TurnConfig, TurnEvent, TurnResult, TurnSink, Value, lane_name, tools,
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
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

    #[allow(clippy::too_many_lines)] // one turn loop; splitting is refactoring
    async fn run_turn(&self) -> Result<(), AgentError> {
        // A force send or stop that targeted this turn is captured before the
        // call loop clears the per-call kill flag; it marks every segment of
        // the turn interrupted, not just the one cut mid-flight.
        let turn_killed = self.kill.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst);
        // A force/stop that landed before the turn applies to the first call
        // only; later calls in the turn are fresh.
        let mut first_call = true;
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
                    self.om_mid_loop();
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
            self.om_mid_loop();
            if self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held")
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
                let inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
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
            let inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
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
        // D13.1: join the in-flight buffer cycle BEFORE the pass — the
        // pass's activation (promote) reads the buffer the cycle writes.
        // 60s bound: a cycle outliving it is fine, the write-back merge
        // below picks up its commit (D13.2) and the turn doesn't hang.
        let handle = {
            let mut inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            inner.om_inflight.take()
        };
        if let Some(handle) = handle {
            let timed_out = tokio::time::timeout(std::time::Duration::from_secs(60), handle)
                .await
                .is_err();
            if timed_out {
                eprintln!(
                    "om: buffer cycle outlived the 60s join; the turn-end merge picks up its commit"
                );
            }
        }
        // The re-lock: each sync store phase takes the session lock and
        // releases it before the next provider call.
        let mut with_store =
            |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), crate::om_integration::OmError>| {
                let mut guard = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
                f(&mut guard.store)
            };
        // The pass goes through the session's inner provider, not the
        // forwarding seam (ticket #86): OM calls are not user-facing
        // streams, and keeping them out of the forwarding call registry
        // means a failed pass cannot surface as an interrupted `StreamEnd`
        // in the post-turn reconciliation (the #82 class). A plain
        // (non-forwarding) provider has no registry — use it as-is.
        let om_provider = self
            .provider
            .forwarding_inner()
            .unwrap_or_else(|| self.provider.clone());
        // A failed pass (a provider error that survived its retries, a
        // storage error) must not kill the turn (ticket #86): the entries
        // stay unobserved and the next turn-end retries; the partial state
        // is written back as-is.
        if let Err(e) = state
            .settle_turn(&mut with_store, &om_provider, &model, hook.as_ref())
            .await
        {
            eprintln!("om: turn-end pass failed: {e}");
            // Close the status gauge: a failed pass never reaches the
            // loop's `Done` arm, the only place it goes idle.
            if let Some(hook) = &hook {
                hook("idle");
            }
        }
        // D13.2: the write-back is a field-split merge, not a clobber — a
        // cycle that committed during the pass's LLM round-trip (the
        // join timed out) keeps its chunks; the pass's observation
        // fields win.
        {
            let mut inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            match crate::om_integration::OmState::load_record(&mut inner.store) {
                Ok(fresh) => {
                    let live = inner.om.clone().unwrap_or_else(|| state.clone());
                    state.merge_turn_end(&live, &fresh);
                    // Persist only when the merge changed the DURABLE chunk state — a
                    // racing cycle's commit (the join timed out) or a
                    // sync-buffered chunk. Compare the chunk fields, not
                    // the whole record: the pass refreshes
                    // `pending_tokens` on every run, so a full equality
                    // would re-save (append an `om` entry) on every turn
                    // of any session with unobserved entries.
                    let chunk_delta = state.record.buffered_chunks != fresh.buffered_chunks
                        || state.record.last_buffered_at_tokens > fresh.last_buffered_at_tokens;
                    if !chunk_delta {
                        // The common case: the merge is a no-op over the
                        // file's chunk state, so nothing is re-saved.
                    } else if let Err(e) = state.save(&mut inner.store) {
                        eprintln!("om: write-back merge save failed: {e}");
                    }
                    inner.om = Some(state);
                }
                Err(e) => {
                    eprintln!("om: record load for the write-back merge failed: {e}");
                    inner.om = Some(state);
                }
            }
        }
        Ok(())
    }

    /// The mid-episode interval-boundary trigger (ticket #86 P2, D3/D4):
    /// runs after each tool round — the moment new unobserved tokens
    /// exist. At the observe threshold it promotes buffered chunks
    /// (activation, no LLM call); below it, it spawns the background
    /// buffer cycle when an interval boundary was crossed. The spawn
    /// happens INSIDE the lock section: the in-flight flag is set before
    /// the task can re-acquire the lock (D14), so the cycle and the
    /// trigger never race on it. With `buffer_tokens = 0` (kill switch)
    /// this is a no-op and the pass keeps today's sync behavior.
    fn om_mid_loop(&self) {
        let mut guard = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        let Inner {
            store,
            om,
            om_inflight,
            om_buffer_boundary,
            om_model,
            model,
            ..
        } = &mut *guard;
        let Some(om) = om.as_mut() else {
            return;
        };
        if om.config.buffer_tokens == 0 {
            return;
        }
        let Ok(unobserved) = om.unobserved(store) else {
            // A store read failure degrades: keep the last known pending,
            // no trigger this round (the turn-end pass retries the read).
            return;
        };
        om.record.pending_tokens = om.pending_tokens(&unobserved);
        let spawn_pending = if om.activation_reached(om.record.pending_tokens) {
            // Activation: promote the buffered chunks into the log (no LLM
            // call); the sync turn-end pass owns the at/above-threshold
            // range from here (the mastra partition).
            match om.promote(store) {
                Ok(_) => None,
                Err(e) => {
                    eprintln!("om: mid-loop activation failed: {e}");
                    None
                }
            }
        } else if crate::om_integration::background::boundary_crossed(
            u64::from(om.record.pending_tokens),
            &om.config,
            om.record.last_buffered_at_tokens,
            *om_buffer_boundary,
        ) {
            Some(om.record.pending_tokens)
        } else {
            None
        };
        let Some(pending) = spawn_pending else {
            return;
        };
        if om_inflight.is_some() {
            // A cycle is already running: it owns the interval; the next
            // crossing (one more interval of material) triggers again.
            return;
        }
        // The in-process boundary advances at TRIGGER time (D6); the
        // persisted one only at the cycle's successful commit.
        *om_buffer_boundary = u64::from(pending);
        let inner = std::sync::Arc::clone(&self.inner);
        let config = om.config.clone();
        let provider = self
            .provider
            .forwarding_inner()
            .unwrap_or_else(|| self.provider.clone());
        let model_owned = if om_model.is_empty() {
            // The session model (same resolution as the turn-end pass).
            model.clone()
        } else {
            om_model.clone()
        };
        *om_inflight = Some(tokio::spawn(
            crate::om_integration::background::buffer_cycle(
                inner,
                config,
                provider,
                model_owned,
                u64::from(pending),
            ),
        ));
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
    // Two-phase tool recording writes a call-phase entry (empty output)
    // before its result-phase entry, so the last tool entry per call_id in
    // file order is the result. Emit exactly that one — a per-call_id
    // last-wins dedupe (ticket #37) — so a tool that returns "" still pairs
    // its function_call with an output and the Responses API never 400s on
    // a missing output.
    let last_tool = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let cid = e.payload.get("call_id").and_then(Value::as_str)?;
            e.payload
                .get("output")
                .and_then(CallOutput::from_payload)
                .is_some()
                .then(|| (cid.to_owned(), i))
        })
        .fold(HashMap::new(), |mut m, (cid, i)| {
            m.insert(cid, i);
            m
        });
    let mut out = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
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
                // Only the last recorded phase per call_id is an input item
                // (the result); an earlier phase for the same call_id is the
                // call, not a second result.
                if last_tool.get(call_id) != Some(&i) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_entry(id: &str, call_id: &str, output: &str) -> Entry {
        Entry {
            id: id.to_owned(),
            parent: None,
            timestamp: 0,
            kind: KIND_TOOL.to_owned(),
            payload: tau_protocol::payload::ToolPayload {
                call_id: call_id.to_owned(),
                name: "bash".to_owned(),
                args: Value::Null,
                output: tau_protocol::payload::ToolOutput::Text(output.to_owned()),
            }
            .to_value(),
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        }
    }

    #[test]
    fn empty_tool_result_is_emitted_as_call_output() {
        // A tool that returns "" still pairs its call: the result phase is
        // emitted even though its text is empty (ticket #37). The old
        // skip-empties rule dropped it and 400'd the next request.
        let entries = vec![tool_entry("e1", "call-1", "")];
        let items = input_items(&entries);
        assert!(
            matches!(&items[..], [InputEntry::CallOutput(o)] if o.call_id == "call-1"),
            "expected one call output for the empty result, got {items:?}"
        );
    }

    #[test]
    fn dedupe_keeps_only_the_last_phase_per_call_id() {
        // Two entries share a call_id (the call phase and its result): the
        // dedupe keeps only the last (the result), not both (ticket #37).
        let entries = vec![
            tool_entry("e1", "call-1", ""),
            tool_entry("e2", "call-1", "done"),
        ];
        let items = input_items(&entries);
        assert!(
            matches!(
                &items[..],
                [InputEntry::CallOutput(o)]
                    if o.call_id == "call-1"
                        && matches!(&o.output, CallOutput::Text(t) if t == "done")
            ),
            "expected exactly the result phase, got {items:?}"
        );
    }
}
