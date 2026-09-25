use super::*;

impl AgentSession {
    /// Run turns until the queue is empty.
    pub async fn process(&self) -> Result<(), AgentError> {
        loop {
            let Some(starter) = self.inner.lock().unwrap().queue.pop_front() else {
                return Ok(());
            };
            self.append_user(starter).await?;
            self.run_turn().await?;
        }
    }

    async fn run_turn(&self) -> Result<(), AgentError> {
        let mut rounds = 0usize;
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
            let delivered = {
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
                out
            };
            for msg in delivered {
                self.append_user(msg).await?;
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
                match inner.om.as_mut() {
                    Some(om) => {
                        let instructions = om.assemble_context(&base, contract.as_deref());
                        let raw = om.raw_window_from(&entries, leaf_id.as_deref());
                        (instructions, input_items(&raw))
                    }
                    None => {
                        let mut instructions = base;
                        if let Some(contract) = &contract {
                            instructions.push_str("\n\n# Task (resume contract)\n");
                            instructions.push_str(contract);
                        }
                        (instructions, input_items(&entries))
                    }
                }
            };
            let mut request =
                ResponseRequest::new(self.model().to_owned(), Some(system_prompt.as_str()), input)
                    .with_tools(self.tools().clone());
            if let Some(n) = self.turn_config().max_output_tokens {
                request = request.with_max_output_tokens(n);
            }
            if let Some(e) = self.turn_config().reasoning {
                request = request.with_reasoning(e);
            }

            // A force has already killed the stream it targeted; every
            // fresh call starts un-killed (spec §7).
            self.kill.store(false, Ordering::SeqCst);
            let mut sink = KillSink {
                kill: self.kill.clone(),
                stop: self.stop.clone(),
            };
            let result = self.provider.call(&request, &mut sink).await?;
            self.append_assistant(&result)?;

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
            let output = if call.name == "task_assign" {
                // Cross-session: routed through the supervisor, which runs
                // both sides on the sessions' own stores (review B3).
                let sup = {
                    let inner = self.inner.lock().unwrap();
                    inner.subagents.clone()
                };
                match sup {
                    Some(sup) => {
                        let task = tc.args.get("task").and_then(Value::as_str).unwrap_or("");
                        let worker = tc.args.get("worker").and_then(Value::as_str).unwrap_or("");
                        match sup.assign_task(task, worker) {
                            Ok(()) => format!("assigned {task} to {worker}").into(),
                            Err(e) => e.into(),
                        }
                    }
                    None => "task_assign: this session has no sub-agents".into(),
                }
            } else if call.name.starts_with("task_") {
                // Tasks live in this session's own store (spec §5.3) — parent
                // and child alike. Routed before the sub-agent surface so a
                // child (which has no supervisor) still gets its tools.
                self.task_tool(&tc).into()
            } else if call.name == "recall" {
                self.recall_scoped(&args).into()
            } else {
                // The sub-agent surface routes outside the core tools: the
                // parent's supervisor tools, or the child's `parent_notify`
                // (ticket #23); everything else is a core tool. The links
                // are cloned under the lock and invoked outside it: the
                // routes re-lock this session's `inner` (a child notify
                // records a state entry; a spawn reads the parent's OM
                // record), and the lock is not reentrant.
                let (sup, link) = {
                    let inner = self.inner.lock().unwrap();
                    (inner.subagents.clone(), inner.child.clone())
                };
                // Route by name, not by which link exists: a parent session
                // carries a supervisor AND core tools (ticket #23's original
                // `if let Some(sup)` swallowed every core tool into
                // route_parent, which only knows sub-agent names), and a
                // child carries a link AND core tools.
                if call.name.starts_with("subagent_") {
                    match sup {
                        Some(sup) => crate::subagent::route_parent(&sup, &tc).into(),
                        None => format!("{}: not available in this session", call.name).into(),
                    }
                } else if call.name == "parent_notify" {
                    match link {
                        Some(link) => link.notify(&args).into(),
                        None => "parent_notify: not available in a top-level session".into(),
                    }
                } else {
                    tools::dispatch(&self.cwd(), &tc).await
                }
            };
            self.append(
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

    /// The turn-end OM pass (ticket #22): the state runs on a clone and
    /// every store op takes the session lock briefly; the LLM round-trips
    /// run between the lock scopes, so a force or steering send is never
    /// blocked on an OM call.
    async fn om_turn_end(&self) -> Result<(), AgentError> {
        let (mut state, hook) = {
            let inner = self.inner.lock().unwrap();
            (inner.om.clone(), inner.om_status_hook.clone())
        };
        let Some(state) = &mut state else {
            return Ok(());
        };
        let mut action = {
            let mut inner = self.inner.lock().unwrap();
            let unobserved = state.unobserved(&mut inner.store).map_err(AgentError::Om)?;
            state.record.pending_tokens = state.pending_tokens(&unobserved);
            // Activation (no LLM call): the token threshold, or the fixed
            // idle timeout with pending chunks (spec §4).
            let idle = {
                let all = inner
                    .store
                    .entries_range(0, usize::MAX)
                    .map_err(|e| AgentError::Om(e.into()))?;
                let leaf = inner
                    .store
                    .leaf()
                    .map_err(|e| AgentError::Om(e.into()))?
                    .map(|e| e.id);
                crate::om_integration::idle_gap_secs(&all, leaf.as_deref())
            };
            if !state.buffered.is_empty()
                && (state.activation_reached(state.record.pending_tokens)
                    || idle >= crate::om_integration::IDLE_ACTIVATION_SECS)
            {
                state.promote(&mut inner.store).map_err(AgentError::Om)?;
            }
            state.plan(&unobserved)
        };
        loop {
            // The activity the status bar's gauge shows while this run is in
            // flight; `Done` closes the run (the hook is the app's om_status
            // emitter, absent in tests and on children).
            if let Some(hook) = &hook {
                let kind = match &action {
                    crate::om_integration::TurnEndAction::Done => "idle",
                    crate::om_integration::TurnEndAction::Observe { .. }
                    | crate::om_integration::TurnEndAction::Buffer { .. } => "observing",
                    crate::om_integration::TurnEndAction::Reflect { .. } => "reflecting",
                };
                hook(kind);
            }
            let result = match &action {
                crate::om_integration::TurnEndAction::Done => break,
                crate::om_integration::TurnEndAction::Observe { transcript }
                | crate::om_integration::TurnEndAction::Buffer { transcript } => {
                    let system = crate::om::observer_system_prompt();
                    let request = ResponseRequest::new(
                        self.om_model(),
                        Some(system.as_str()),
                        vec![InputEntry::Message(InputMessage {
                            role: "user".into(),
                            content: transcript.clone(),
                        })],
                    );
                    let mut sink = crate::om_integration::NoopSink;
                    self.provider
                        .call(&request, &mut sink)
                        .await
                        .map_err(AgentError::Provider)?
                }
                crate::om_integration::TurnEndAction::Reflect { level } => {
                    let prompt = state.reflector_prompt(*level);
                    let system = crate::om::reflector_system_prompt();
                    let request = ResponseRequest::new(
                        self.om_model(),
                        Some(system.as_str()),
                        vec![InputEntry::Message(InputMessage {
                            role: "user".into(),
                            content: prompt,
                        })],
                    );
                    let mut sink = crate::om_integration::NoopSink;
                    self.provider
                        .call(&request, &mut sink)
                        .await
                        .map_err(AgentError::Provider)?
                }
            };
            {
                let mut inner = self.inner.lock().unwrap();
                state
                    .commit(&mut inner.store, &mut action, &result)
                    .map_err(AgentError::Om)?;
            }
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.om = Some(state.clone());
        }
        Ok(())
    }

    /// The OM model (global config, spec §4); empty = the session's model.
    fn om_model(&self) -> String {
        let inner = self.inner.lock().unwrap();
        if inner.om_model.is_empty() {
            inner.model.clone()
        } else {
            inner.om_model.clone()
        }
    }

    async fn append_user(&self, msg: Queued) -> Result<(), AgentError> {
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

    fn append_assistant(&self, result: &TurnResult) -> Result<(), AgentError> {
        // A partial cut before anything arrived has nothing to record
        // (review N10): an empty, call-less interrupted entry is noise.
        if !result.completed
            && result.text.is_empty()
            && result.reasoning.is_empty()
            && result.calls.is_empty()
        {
            return Ok(());
        }
        self.append(
            KIND_ASSISTANT,
            tau_protocol::payload::AssistantPayload {
                text: result.text.clone(),
                reasoning: result.reasoning.clone(),
                interrupted: !result.completed,
                usage: result.usage.clone(),
                calls: result.calls.clone(),
            }
            .to_value(),
        )
    }

    pub(super) fn append(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
        // A fresh session has no leaf: the first entry starts the branch.
        // Any other failure is a storage error and propagates.
        let parent = match inner.store.leaf() {
            Ok(leaf) => leaf.map(|e| e.id),
            Err(e) => return Err(AgentError::Session(e)),
        };
        let parent = parent.as_deref();
        inner.store.append(kind, payload, parent)?;
        Ok(())
    }

    fn cwd(&self) -> PathBuf {
        self.inner.lock().unwrap().cwd.clone()
    }

    fn tool_batch_on_force(&self) -> ToolBatchPolicy {
        self.inner.lock().unwrap().tool_batch_on_force
    }

    fn turn_config(&self) -> TurnConfig {
        self.inner.lock().unwrap().turn
    }
}

/// The sink the loop gives the provider: the per-call kill flag (force)
/// or the persistent stop flag (ticket #23) stops the stream (spec §7).
struct KillSink {
    kill: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl TurnSink for KillSink {
    fn event(&mut self, _: crate::provider::TurnEvent) -> bool {
        !self.kill.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst)
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
