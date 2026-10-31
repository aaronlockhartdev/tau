use super::route::title_on_disk;
use super::{
    AgentSession, Arc, AtomicBool, AtomicUsize, Child, ChildState, ContextMode, Mutex, Ordering,
    SessionStore, SpawnNotice, Spawned, Supervisor, TurnOrigin, Weak,
};
use crate::agent::Lane;
use crate::harness::SessionRole;
use crate::om::OmRecord;
use crate::om_integration::{self, OmState};
use crate::tools;
use serde_json::json;
use tau_protocol::EntryKind;

impl Supervisor {
    /// Spawn a child (spec §5.1): async — this only sets things up; the
    /// child's loop runs on its own task and reports through `parent_notify`.
    #[allow(clippy::too_many_lines)] // one spawn setup; splitting is refactoring
    pub fn spawn(
        self: &Arc<Self>,
        agent_type: &str,
        brief: &str,
        context_mode: Option<ContextMode>,
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
        // The fleet's hard cap on total handles (the backstop the soft
        // bound exists to keep under, #250) — checked before any work.
        self.check_fleet_cap()?;
        let session_id = SessionStore::new_session_id();
        let parent_agent = self
            .parent
            .lock()
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "supervisor has no parent attached".to_owned())?;
        let parent_record = parent_agent
            .om_state()
            .map(|s| s.record)
            .unwrap_or_default();

        // The child's session file: a fresh file (fresh/compacted) or a
        // branched copy of the parent's entry tree (fork, spec §5.1).
        let mut store = if context_mode == ContextMode::Fork {
            let mut source = SessionStore::for_workspace(&self.cwd, &self.parent_session);
            source
                .open()
                .map_err(|e| format!("subagent_spawn: cannot fork: {e}"))?;
            SessionStore::fork_from(&source, &session_id)
                .map_err(|e| format!("subagent_spawn: fork failed: {e}"))?
        } else {
            let mut store = SessionStore::for_workspace(&self.cwd, &session_id);
            store.create().map_err(|e| format!("subagent_spawn: {e}"))?;
            store
        };
        // The header carries the parent link too, so the nesting survives a
        // restart (the spawn record below is the entry-level provenance).
        store
            .set_parent(&self.parent_session)
            .map_err(|e| format!("subagent_spawn: {e}"))?;

        // The spawn record: the parent link (session id + originating tool
        // call id) is the child's durable provenance (ADR-0001).
        let leaf = store.leaf().map_or(None, |l| l.map(|e| e.id));
        store
            .append(
                EntryKind::Subagent,
                json!({
                    "event": "spawn",
                    "type": agent_type,
                    "brief": brief,
                    "context_mode": context_mode,
                    "parent": self.parent_session,
                    "call": call_id,
                }),
                leaf.as_deref(),
            )
            .map_err(|e| format!("subagent_spawn: {e}"))?;

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
        // The type configures the child (spec §5.5): `general` inherits
        // the session's prompt, tools, and model; a `.md` type brings its
        // own prompt and, optionally, a tool subset and model.
        let child_prompt = if ty.name == "general" {
            self.system_prompt.clone()
        } else {
            // A `.md` type's prompt is its own body, so the gate section —
            // stated in the prompt when a gate is configured (spec §5.3) —
            // is appended here; the inherited supervisor prompt reaches
            // only `general` children.
            let mut body = ty.body.clone();
            crate::gate::with_section(&mut body, &self.gates);
            body
        };
        // The child's model is resolved before its provider, which the
        // factory routes by that id (#336): a `.md` type naming another
        // provider's model is addressed there, not at its parent's endpoint.
        let inherited = self.inherited();
        let child_model = ty.model.clone().unwrap_or(inherited.model);
        let provider = self.provider.create(&session_id, &child_model);
        let child_tools = tools::surface::child_specs(ty.tools.as_deref());
        // The role carries the child's wiring (R2); the spawn keeps the
        // context-mode and agent-type resolution.
        let agent = AgentSession::launch(
            store,
            SessionRole::Child {
                supervisor: Arc::clone(self),
                provider: provider.clone(),
                system_prompt: child_prompt,
                model: child_model.clone(),
                tools: child_tools,
                record,
                handle: handle.clone(),
                turn: inherited.turn,
            },
        )
        // The child role has no fallible step (the provider, the record,
        // and the values are the spawn's resolved ones); a failure here is
        // reported like any other spawn failure, not a process panic.
        .map_err(|e| format!("subagent_spawn: {e}"))?;
        let created = agent.store_created();
        let stop = agent.stop_flag();
        let child = Arc::new(Child {
            handle: handle.clone(),
            name: name.clone(),
            session_id: session_id.clone(),
            agent_type: agent_type.to_owned(),
            context_mode,
            agent: agent.clone(),
            created,
            state: Mutex::new(ChildState::Running),
            // The spawn's brief is autonomous work: its rest wakes the
            // delegating parent (spec §5.2 turn-origin wake routing).
            origin: Mutex::new(TurnOrigin::Spawn),
            rest_requested: AtomicBool::new(false),
            declared_output: Mutex::new(None),
            declared_waiting: Mutex::new(None),
            last_message: Mutex::new(Some(brief.to_owned())),
            drive_gen: AtomicUsize::new(0),
            last_ended_gen: AtomicUsize::new(0),
            wake: tokio::sync::Notify::new(),
        });
        self.children.lock().insert(handle.clone(), child.clone());
        // The spawn's child is in: reclaim the oldest terminal children if
        // the fleet now exceeds its soft bound (spawn-triggered, #250) —
        // the new child is running, so it can never be the eviction target.
        self.reclaim_terminal();
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
    /// sleeps while the child rests (`idle`/`awaiting`), so a resume — a
    /// message that flips the child back to `Running` — is picked up by the
    /// same task; there is no window in which a resume can race the drive's
    /// exit (ADR-0001: everything non-running is resumable, nothing
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
                // `idle` and `awaiting` are resting states: the drive sleeps
                // until a resume (a message, a nudge-free world — spec §5.2
                // deletes the one-shot nudge) or a stop.
                ChildState::Idle { .. } | ChildState::Awaiting => {
                    if child.agent.has_pending() {
                        // A message landed while the child rested: a
                        // message-origin resume (spec §5.2 turn origin).
                        child.set_origin(TurnOrigin::Message);
                        child.reset_hints();
                        if child.set_state(&ChildState::Running).is_err() {
                            sup.mark_failed(&child, "state entry failed".into());
                            break;
                        }
                        sup.emit_state(
                            &child,
                            ChildState::Running,
                            Some("resumed by a queued message".into()),
                            None,
                        );
                        continue;
                    }
                    // No polling (review N8): a message, or a stop, notifies
                    // the drive; the permit keeps an out-of-order wake from
                    // being lost.
                    child.wake.notified().await;
                    continue;
                }
                // done/stopped/failed end the drive: a done child is
                // quiescent, and a resume starts a fresh drive (the message path).
                _ => break,
            }
            match sup.driver.drive(&child.session_id, &child.agent).await {
                Ok(()) => {}
                Err(reason) => {
                    sup.mark_failed(&child, reason);
                    break;
                }
            }
            // A superseded drive bails before settling: the fresh drive owns
            // the rest (two drives must never settle one child).
            if child.drive_gen.load(Ordering::SeqCst) != drive_gen {
                break;
            }
            // `parent_notify` (or a stop) already settled the child: the
            // loop head sleeps until a resume.
            if !matches!(child.state(), ChildState::Running) {
                continue;
            }
            // Observed rest (spec §5.2): the turn ended and the child is
            // still `Running` — the state is read from the observed signals
            // (gate_failed, the `QUESTION:` marker, the plan file), never
            // nudged. An undeclared stop is `idle`, never force-`failed`.
            sup.settle_rest(&child);
            match child.state() {
                // done/failed are terminal: the drive ends (the resume path
                // starts a fresh one); idle/awaiting sleep at the head.
                ChildState::Done { .. } | ChildState::Failed { .. } => break,
                _ => {}
            }
        }
        // The loop exited by any arm: record this generation's end (a
        // superseded drive reporting after the fresh one is a no-op — the
        // quiescence check compares against the newest generation).
        child.last_ended_gen.store(drive_gen, Ordering::SeqCst);
    }
}
