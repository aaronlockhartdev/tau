//! The streaming seam: the forwarding provider that mirrors stream events onto the protocol channel, the protocol mapping, the session provider factory, and the sub-agent factory/driver/bridge.

use super::*;
use std::future::Future;
use std::pin::Pin;
/// A `TurnProvider` wrapper that mirrors stream events onto the protocol
/// channel (spec §8 streaming) before the loop's own sink sees them.
pub(crate) struct ForwardingProvider {
    pub(crate) inner: TurnProviderRef,
    pub(crate) tx: mpsc::Sender<Event>,
    /// The core's pipe drop bookkeeping (shared): an overflow in a child
    /// stream is counted and surfaced exactly like the core's own pipe.
    pub(crate) pipe: PipeCounters,
    pub(crate) workspace: String,
    pub(crate) session: String,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) call_seq: AtomicU64,
    /// Call ids in start order; a turn diffs its assistant entries against
    /// this (the kth entry in a turn is the kth call).
    pub(crate) calls: Arc<Mutex<Vec<String>>>,
    /// Calls whose `Completed` (usage) event was seen; the post-turn diff
    /// emits `StreamEnd` for the rest.
    pub(crate) completed: Arc<Mutex<HashMap<String, bool>>>,
}

impl TurnProvider for ForwardingProvider {
    fn call<'a>(&self, request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let call_id = format!(
            "{}-{:08}",
            self.session,
            self.call_seq.fetch_add(1, Ordering::Relaxed) + 1
        );
        // The sink is fully owned (clones of the shared state), so the
        // future captures no lifetimes of its own beyond the loop's sink.
        let mut forward = ForwardSink {
            tx: self.tx.clone(),
            pipe: self.pipe.clone(),
            workspace: self.workspace.clone(),
            session: self.session.clone(),
            stop: self.stop.clone(),
            calls: self.calls.clone(),
            completed: self.completed.clone(),
            call_id,
            started: false,
            inner: sink,
        };
        let inner = self.inner.clone();
        let request = request.clone();
        Box::pin(async move { inner.call(&request, &mut forward).await })
    }
}

/// One stream's mirror: forwards each delta onto the protocol channel and
/// delegates to the loop's own sink (the kill point stays the loop's).
pub(crate) struct ForwardSink<'a> {
    pub(crate) tx: mpsc::Sender<Event>,
    pipe: PipeCounters,
    pub(crate) workspace: String,
    session: String,
    stop: Arc<AtomicBool>,
    calls: Arc<Mutex<Vec<String>>>,
    completed: Arc<Mutex<HashMap<String, bool>>>,
    call_id: String,
    started: bool,
    inner: &'a mut dyn TurnSink,
}

impl TurnSink for ForwardSink<'_> {
    fn event(&mut self, event: TurnEvent) -> bool {
        if !self.started {
            self.started = true;
            self.calls.lock().unwrap().push(self.call_id.clone());
            self.send(Event::StreamStart {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                call_id: self.call_id.clone(),
            });
        }
        match &event {
            TurnEvent::Text(t) => self.send(Event::StreamDelta {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                call_id: self.call_id.clone(),
                text: t.clone(),
                reasoning: None,
            }),
            TurnEvent::Reasoning(t) => self.send(Event::StreamDelta {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                call_id: self.call_id.clone(),
                text: String::new(),
                reasoning: Some(t.clone()),
            }),
            TurnEvent::Completed(u) => {
                self.completed
                    .lock()
                    .unwrap()
                    .insert(self.call_id.clone(), true);
                self.send(Event::StreamEnd {
                    workspace: self.workspace.clone(),
                    session: self.session.clone(),
                    call_id: self.call_id.clone(),
                    interrupted: false,
                    usage: Some(Usage {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        total_tokens: u.total_tokens,
                        cached_prompt_tokens: u
                            .prompt_tokens_details
                            .as_ref()
                            .map(|d| d.cached_tokens)
                            .unwrap_or(0),
                    }),
                });
            }
        }
        // A stop cuts the stream at the next delta (the loop records the
        // partial as interrupted, spec §7).
        if self.stop.load(Ordering::SeqCst) {
            return false;
        }
        self.inner.event(event)
    }
    fn stop_signal(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(stop_flag(&self.stop))
    }
}

/// The prefill half of a stop (spec §7): a poll over the flag so the
/// provider tears the in-flight request down before the first stream event
/// arrives. 50 ms: a human clicking stop, not a tight race.
async fn stop_flag(stop: &AtomicBool) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl ForwardSink<'_> {
    fn send(&self, event: Event) {
        // A drop is never silent (the shared counter + a one-shot summary error)
        pipe_send(
            &self.tx,
            &self.pipe,
            event,
            self.workspace.clone(),
            Some(self.session.clone()),
        );
    }
}

/// The context modes are structurally identical (both three-lowercase);
/// the mapping is the ADR-0002 crate boundary.
pub(crate) fn mode_to_protocol(m: crate::subagent::ContextMode) -> ContextMode {
    match m {
        crate::subagent::ContextMode::Fresh => ContextMode::Fresh,
        crate::subagent::ContextMode::Compacted => ContextMode::Compacted,
        crate::subagent::ContextMode::Fork => ContextMode::Fork,
    }
}

/// The core's child record as the protocol's (the protocol crate is
/// independent of the core — ADR-0002 — so the mapping lives here).
pub(crate) fn info_to_protocol(i: &crate::subagent::SubagentInfo) -> SubagentInfo {
    let state = match &i.state {
        crate::subagent::ChildState::Running => "running",
        crate::subagent::ChildState::Idle { .. } => "idle",
        crate::subagent::ChildState::Done { .. } => "done",
        crate::subagent::ChildState::Failed { .. } => "failed",
        crate::subagent::ChildState::Stopped { .. } => "stopped",
    };
    SubagentInfo {
        handle: i.handle.clone(),
        child: i.child.clone(),
        agent_type: i.agent_type.clone(),
        context_mode: mode_to_protocol(i.context_mode),
        state: state.to_owned(),
        waiting_on: i.waiting_on.map(|w| w.as_str().to_owned()),
        last_message: i.last_message.clone(),
        usage: i.usage.as_ref().map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
            cached_prompt_tokens: u
                .prompt_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or(0),
        }),
        task: i.task.clone(),
        resume_contract: i.resume_contract.clone(),
    }
}

/// A session's provider: a `canned://` entry (the dev-gated test hook) takes
/// its scripted replay; everything else is the live endpoint. A `canned://`
/// entry in a release build has no scripts, so it falls through to the live
/// endpoint, where the unresolvable URL fails the turn.
pub(crate) fn session_inner(
    client: &reqwest::Client,
    p: &crate::config::Provider,
    requests: &crate::config::Requests,
) -> TurnProviderRef {
    provider::canned_dev(p).unwrap_or_else(|| provider::production(client, p, requests))
}

/// The child provider factory (ticket #23 N3): wraps the session's
/// production provider in the forwarding seam, per child session id.
pub(crate) struct ForwardingChildFactory {
    pub(crate) client: reqwest::Client,
    pub(crate) provider: crate::config::Provider,
    pub(crate) requests: crate::config::Requests,
    pub(crate) tx: mpsc::Sender<Event>,
    pub(crate) pipe: PipeCounters,
    pub(crate) workspace: String,
}
impl ChildProviderFactory for ForwardingChildFactory {
    fn create(&self, child: &str) -> TurnProviderRef {
        Arc::new(ForwardingProvider {
            inner: session_inner(&self.client, &self.provider, &self.requests),
            tx: self.tx.clone(),
            pipe: self.pipe.clone(),
            workspace: self.workspace.clone(),
            session: child.to_owned(),
            stop: Arc::new(AtomicBool::new(false)),
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

/// The child driver (N3): a child's in-flight round is a plain session
/// turn — the child is an ordinary session, so its drive is `run_turn` on
/// its live session (registered by the bridge at spawn).
pub(crate) struct TurnChildDriver {
    pub(crate) core: Weak<Core>,
}
impl ChildDriver for TurnChildDriver {
    fn drive(&self, session: &str, _agent: &Arc<AgentSession>) -> BoxedDrive {
        let live = self
            .core
            .upgrade()
            .and_then(|c| c.sessions.lock().unwrap().get(session).cloned());
        let Some((core, live)) = self.core.upgrade().zip(live) else {
            return Box::pin(async { Ok(()) });
        };
        Box::pin(async move {
            run_turn(core, live).await;
            Ok(())
        })
    }
}

/// The dispatch-side sub-agent bridge (N3): events, parent wakes, and
/// child-session registration — a child is an ordinary live session
/// (ADR-0006), so the GUI can open it the same way as any session.
pub(crate) struct SessionSubagentBridge {
    pub(crate) core: Arc<Core>,
    pub(crate) workspace: String,
    pub(crate) client: reqwest::Client,
    pub(crate) provider: crate::config::Provider,
    pub(crate) requests: crate::config::Requests,
    pub(crate) sup: Mutex<Option<Weak<Supervisor>>>,
}
impl SubagentBridge for SessionSubagentBridge {
    fn spawned(&self, n: &SpawnNotice) {
        // Register the child as a live session so the GUI can open it and
        // the driver can feed it turns.
        let Some(agent) = self
            .sup
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
            .and_then(|s| s.child_agent(&n.handle))
        else {
            return;
        };
        let Some(parent_live) = self.core.sessions.lock().unwrap().get(&n.parent).cloned() else {
            return;
        };
        let mut store = SessionStore::for_workspace(&parent_live.cwd, &n.child);
        if store.open().is_err() {
            return; // the spawn failed after the notice; nothing to register
        }
        // The core set the child's header title at spawn (type + adjective-noun);
        // the event carries it so the GUI's stub gets the real name.
        let title = store.title().unwrap_or("sub-agent").to_string();
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::Spawned {
                handle: n.handle.clone(),
                child: n.child.clone(),
                agent_type: n.agent_type.clone(),
                context_mode: mode_to_protocol(n.context_mode),
                title,
            },
        });
        let provider = Arc::new(ForwardingProvider {
            inner: session_inner(&self.client, &self.provider, &self.requests),
            tx: self.core.events_tx.clone(),
            pipe: self.core.pipe.clone(),
            workspace: self.workspace.clone(),
            session: n.child.clone(),
            stop: agent.stop_flag(),
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        });
        let live = Arc::new(LiveSession {
            meta: Mutex::new(SessionMeta {
                id: n.child.clone(),
                workspace: self.workspace.clone(),
                title: store.title().map(str::to_string),
                parent: Some(n.parent.clone()),
                created: store.created(),
                leaf: None,
                model: Some(n.model.clone()),
                usage: None,
                archived: false,
            }),
            agent: agent.clone(),
            stop: agent.stop_flag(),
            queue: Mutex::new(Vec::new()),
            turn: AtomicBool::new(false),
            provider,
            cwd: parent_live.cwd.clone(),
        });
        let id = live.meta.lock().unwrap().id.clone();
        self.core.sessions.lock().unwrap().insert(id, live);
    }

    fn state(&self, n: &StateNotice) {
        let detail = match &n.state {
            crate::subagent::ChildState::Done { output } => Some(json!({ "output": output })),
            crate::subagent::ChildState::Failed { reason } => Some(json!({ "reason": reason })),
            crate::subagent::ChildState::Idle { waiting_on } => {
                Some(json!({ "waiting_on": waiting_on.as_str() }))
            }
            crate::subagent::ChildState::Stopped { by } => {
                let mut d = json!({ "by": by });
                if let Some(rc) = &n.resume_contract {
                    d["resume_contract"] =
                        serde_json::to_value(rc).unwrap_or(serde_json::Value::Null);
                }
                Some(d)
            }
            crate::subagent::ChildState::Running => None,
        };
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::State {
                handle: n.handle.clone(),
                child: n.child.clone(),
                state: match &n.state {
                    crate::subagent::ChildState::Running => "running",
                    crate::subagent::ChildState::Idle { .. } => "idle",
                    crate::subagent::ChildState::Done { .. } => "done",
                    crate::subagent::ChildState::Failed { .. } => "failed",
                    crate::subagent::ChildState::Stopped { .. } => "stopped",
                }
                .to_owned(),
                detail,
                note: n.note.clone(),
            },
        });
    }

    fn wake(&self, n: &WakeNotice) {
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::Notified {
                child: n.child.clone(),
                wake: match n.kind {
                    WakeKind::Done => "done",
                    WakeKind::Failed => "failed",
                    WakeKind::Waiting => "waiting",
                    WakeKind::Stopped => "stopped",
                }
                .into(),
                text: n.text.clone(),
                output: n.output.clone(),
            },
        });
        // Wake the parent (ADR-0001 wake rules): the notification is already
        // on its active branch; deliver it to the loop and start a turn.
        let Some(live) = self.core.sessions.lock().unwrap().get(&n.parent).cloned() else {
            return;
        };
        let text = match &n.output {
            Some(o) => format!(
                "{} — {}",
                n.text,
                serde_json::to_string(o).unwrap_or_default()
            ),
            None => n.text.clone(),
        };
        live.agent.send_notified(text.clone(), n.child.clone());
        {
            let mut q = live.queue.lock().unwrap();
            q.push(QueuedItem {
                text: text.clone(),
                lane: MessageLane::FollowUp,
            });
        }
        self.core.emit(Event::Queue {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            items: live.queue.lock().unwrap().clone(),
        });
        live.stop.store(false, Ordering::SeqCst);
        if live
            .turn
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            && let Some(core) = self.core.self_arc()
        {
            tokio::spawn(run_turn(core, live));
        }
    }
}

/// The protocol's mirror of a discovered skill (the location as a string,
/// the GUI never sees host paths as `Path`).
pub(crate) fn skill_info(s: &crate::skills::Skill) -> SkillInfo {
    SkillInfo {
        name: s.name.clone(),
        description: s.description.clone(),
        location: s.location.to_string_lossy().into_owned(),
        model_invocation: s.model_invocation,
    }
}
