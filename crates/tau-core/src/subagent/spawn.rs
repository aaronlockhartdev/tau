use super::*;

impl Supervisor {
    /// Spawn a child (spec §5.1): async — this only sets things up; the
    /// child's loop runs on its own task and reports through `parent_notify`.
    pub fn spawn(
        self: &Arc<Self>,
        agent_type: &str,
        brief: &str,
        context_mode: Option<ContextMode>,
        task: Option<&str>,
        call_id: &str,
    ) -> Result<Spawned, String> {
        // Resolve the type (spec §5.5): an unknown name is an error that
        // lists what the session can spawn.
        let ty = self
            .types
            .iter()
            .find(|t| t.name == agent_type)
            .ok_or_else(|| {
                let names: Vec<&str> = self.types.iter().map(|t| t.name.as_str()).collect();
                format!(
                    "subagent_spawn: unknown agent type {agent_type:?} (available: {})",
                    names.join(", ")
                )
            })?;
        let context_mode = context_mode.unwrap_or(ty.context_mode);
        if self.depth + 1 > self.caps.max_depth {
            return Err(format!(
                "subagent_spawn: depth cap reached (max_depth {}; this session is at depth {})",
                self.caps.max_depth, self.depth
            ));
        }
        self.check_cap(true)?;

        let session_id = SessionStore::new_session_id();
        let parent_agent = self
            .parent
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "supervisor has no parent attached".to_owned())?;
        let parent_record = parent_agent
            .om_state()
            .map(|s| s.record)
            .unwrap_or_default();

        // The child's session file: a fresh file (fresh/compacted) or a
        // branched copy of the parent's entry tree (fork, spec §5.1).
        let mut store = match context_mode {
            ContextMode::Fork => {
                let mut source = SessionStore::for_workspace(&self.cwd, &self.parent_session);
                source
                    .open()
                    .map_err(|e| format!("subagent_spawn: cannot fork: {e}"))?;
                SessionStore::fork_from(&source, &session_id)
                    .map_err(|e| format!("subagent_spawn: fork failed: {e}"))?
            }
            _ => {
                let mut store = SessionStore::for_workspace(&self.cwd, &session_id);
                store.create().map_err(|e| format!("subagent_spawn: {e}"))?;
                store
            }
        };
        // The header carries the parent link too, so the nesting survives a
        // restart (the spawn record below is the entry-level provenance).
        store
            .set_parent(&self.parent_session)
            .map_err(|e| format!("subagent_spawn: {e}"))?;

        // The spawn record: the parent link (session id + originating tool
        // call id) is the child's durable provenance (ADR-0001).
        let leaf = store.leaf().map(|l| l.map(|e| e.id)).unwrap_or(None);
        store
            .append(
                KIND_SUBAGENT,
                json!({
                    "event": "spawn",
                    "type": agent_type,
                    "brief": brief,
                    "context_mode": context_mode,
                    "parent": self.parent_session,
                    "call": call_id,
                    "task": task,
                }),
                leaf.as_deref(),
            )
            .map_err(|e| format!("subagent_spawn: {e}"))?;

        // A spawn with a task is an assignment (spec §5.3): the record
        // copies into the child's session before its first turn, so the
        // child works a task that already exists in its own file — the
        // child's session is the live record, the parent's the pointer.
        // Both sides run on their own stores (single writer per session).
        if let Some(task) = task
            && let Some(parent) = self.parent.lock().unwrap().clone()
        {
            let _ = parent.with_task_store(|cstore| {
                crate::task::assign(cstore, &mut store, task, &session_id, &self.parent_session)
            });
        }
        // The child's OM record (spec §5.1, ADR-0004): fresh = an empty
        // prefix; compacted = the parent's log verbatim as the frozen
        // prefix; fork = the parent's record copied wholesale (a fork owns
        // its copy outright).
        let record = match context_mode {
            ContextMode::Compacted => om_integration::seed_compacted_child(
                &parent_record,
                &self.parent_session,
                &mut store,
            )
            .map_err(|e| format!("subagent_spawn: {e}"))?,
            ContextMode::Fork => om_integration::fork_record(&parent_record),
            ContextMode::Fresh => OmRecord::default(),
        };

        // Persist the record into the child's file so it survives the
        // session close/open boundary (load_record reads the newest `om`
        // entry; the seeded prefix would be lost without it).
        OmState::from_config(&self.om, record.clone())
            .save(&mut store)
            .map_err(|e| format!("subagent_spawn: {e}"))?;
        let handle = format!(
            "{}-{}",
            self.parent_session,
            self.next.fetch_add(1, Ordering::SeqCst) + 1
        );
        // A child session keeps a readable, unique name in its header (the
        // adjective-noun pair, like a top-level session), so it survives a
        // restart; retry while it collides with a title already on disk in
        // this workspace.
        let draw = || {
            names::Generator::default()
                .next()
                .expect("the generator yields a name")
        };
        let mut name = draw();
        for _ in 0..8 {
            if !title_on_disk(&self.cwd, &name) {
                break;
            }
            name = draw();
        }
        store
            .set_title(&name)
            .map_err(|e| format!("subagent_spawn: {e}"))?;
        let provider = self.provider.create(&session_id);
        // The type configures the child (spec §5.5): `general` inherits
        // the session's prompt, tools, and model; a `.md` type brings its
        // own prompt and, optionally, a tool subset and model.
        let child_prompt = if ty.name == "general" {
            self.system_prompt.clone()
        } else {
            ty.body.clone()
        };
        let child_model = ty.model.clone().unwrap_or_else(|| self.model.clone());
        let child_tools = match &ty.tools {
            Some(allowed) => tools::child_tool_specs()
                .into_iter()
                .filter(|s| allowed.iter().any(|a| a == &s.name))
                .collect(),
            None => tools::child_tool_specs(),
        };
        let agent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: child_prompt,
            model: child_model.clone(),
            tools: child_tools,
            cwd: self.cwd.clone(),
            provider: provider.clone(),
            tool_batch_on_force: self.tool_batch_on_force,
            turn: self.turn.clone(),
            om: Some(OmState::from_config(&self.om, record)),
            om_model: self.om_model.clone(),
            subagents: None,
            child: Some(Arc::new(ChildLink {
                supervisor: Arc::clone(self),
                handle: handle.clone(),
            })),
        }));

        let created = agent.store_created();
        let stop = agent.stop_flag();
        let child = Arc::new(Child {
            handle: handle.clone(),
            name: name.clone(),
            session_id: session_id.clone(),
            agent_type: agent_type.to_owned(),
            context_mode,
            agent: agent.clone(),
            state: Mutex::new(ChildState::Running),
            nudge_sent: AtomicBool::new(false),
            last_message: Mutex::new(Some(brief.to_owned())),
            drive_gen: AtomicUsize::new(0),
            last_ended_gen: AtomicUsize::new(0),
            wake: tokio::sync::Notify::new(),
        });
        self.children
            .lock()
            .unwrap()
            .insert(handle.clone(), child.clone());
        self.bridge.spawned(&SpawnNotice {
            parent: self.parent_session.clone(),
            handle: handle.clone(),
            child: session_id.clone(),
            agent_type: agent_type.to_owned(),
            context_mode,
            model: child_model.clone(),
        });

        // The brief is the child's first turn (the handoff-in, ADR-0001).
        child.agent.send(brief, Lane::FollowUp);
        let drive_gen = child.drive_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let drive = tokio::spawn(Self::drive_loop(Arc::clone(self), child, drive_gen));
        Ok(Spawned {
            handle,
            name,
            session_id,
            agent,
            provider,
            stop,
            cwd: self.cwd.clone(),
            model: child_model.clone(),
            created,
            drive,
        })
    }

    /// The child's drive: it runs turns while the child is `Running` and
    /// sleeps while the child rests in `idle`, so a resume — a message that
    /// flips the child back to `Running` — is picked up by the same task;
    /// there is no window in which a resume can race the drive's exit
    /// (ADR-0001: everything non-running is resumable, nothing
    /// auto-resumes). `done`, `stopped`, and `failed` end the drive — a
    /// done child is quiescent (no polling task held for the session's
    /// life), and the resume path starts a fresh drive for each.
    pub(super) async fn drive_loop(sup: Arc<Supervisor>, child: Arc<Child>, drive_gen: usize) {
        loop {
            // A newer drive was spawned while this one was between rounds
            // (stop + resume in the window after `drive()` returned): the
            // old loop bails before touching the child again.
            if child.drive_gen.load(Ordering::SeqCst) != drive_gen {
                break;
            }
            match child.state() {
                ChildState::Running => {}
                ChildState::Idle { .. } => {
                    if child.agent.has_pending() {
                        if child.set_state(ChildState::Running).is_err() {
                            sup.mark_failed(&child, "state entry failed".into());
                            break;
                        }
                        sup.bridge.state(&StateNotice {
                            parent: sup.parent_session.clone(),
                            handle: child.handle.clone(),
                            child: child.session_id.clone(),
                            state: ChildState::Running,
                            note: Some("resumed by a queued message".into()),
                            resume_contract: None,
                        });
                        continue;
                    }
                    // No polling (review N8): a message, a nudge, or a
                    // stop notifies the drive; the permit keeps an
                    // out-of-order wake from being lost.
                    child.wake.notified().await;
                    continue;
                }
                // done ends the drive: a done child is quiescent, and a
                // resume of it starts a fresh drive (the message path).
                ChildState::Done { .. } => break,
                _ => break,
            }
            match sup.driver.drive(&child.session_id, &child.agent).await {
                Ok(()) => {}
                Err(reason) => {
                    sup.mark_failed(&child, reason);
                    break;
                }
            }
            // The drive drained the queue and the child moved into a rest
            // state: the loop's head sleeps until a resume.
            if !matches!(child.state(), ChildState::Running) {
                continue;
            }
            // The nudge sits inside this iteration (after `drive()` returns,
            // before the loop's head): a superseded drive must not burn the
            // budget the fresh drive's work period just reset.
            if child.drive_gen.load(Ordering::SeqCst) != drive_gen {
                break;
            }
            // A turn ended without `parent_notify`: the one-shot nudge.
            if !child.nudge_sent.swap(true, Ordering::SeqCst) {
                let _ = child.agent.append_entry(
                    crate::agent::KIND_SYSTEM,
                    json!({ "note": "Nudge: state what you are waiting for, or finish" }),
                );
                child.agent.send(NUDGE, Lane::FollowUp);
                continue;
            }
            // Nudge exhausted: the three-way fork's remaining arm.
            sup.mark_failed(
                &child,
                "nudge exhausted: the child ended a turn without done or a valid wait declaration"
                    .to_owned(),
            );
            break;
        }
        // The loop exited by any arm: record this generation's end (a
        // superseded drive reporting after the fresh one is a no-op — the
        // quiescence check compares against the newest generation).
        child.last_ended_gen.store(drive_gen, Ordering::SeqCst);
    }
}
