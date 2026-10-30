//! The streaming seam: the forwarding provider that mirrors stream events onto the protocol channel, the protocol mapping, the session provider factory, and the sub-agent factory/driver/bridge.

use super::{
    AgentSession, Arc, AtomicBool, AtomicU64, BoxedDrive, ChildDriver, ChildProviderFactory,
    Config, ContextMode, Core, Duration, Event, HashMap, LiveSession, Mutex, Ordering,
    PipeCounters, ProviderTurn, ReadoptNotice, ReclaimNotice, ResponseRequest, SessionMeta,
    SessionStore, Skeleton, SkillInfo, SpawnNotice, StateNotice, SubagentBridge, SubagentEventKind,
    SubagentInfo, Supervisor, TurnEvent, TurnProvider, TurnProviderRef, TurnSink, Usage, WakeKind,
    WakeNotice, Weak, json, mpsc, pipe_send, provider, run_turn,
};
use crate::config::ProviderRef;
use std::future::Future;
use std::pin::Pin;
/// A `TurnProvider` wrapper that mirrors stream events onto the protocol
/// channel (spec §8 streaming) before the loop's own sink sees them.
pub(crate) struct ForwardingProvider {
    /// The endpoint this session's calls go to (#336). The constructor's
    /// provider is the first of them; a model switch to another provider's
    /// model rebinds it, so the next turn's call answers to the model the
    /// session carries, not the provider the config's map sorts first.
    route: Mutex<TurnProviderRef>,
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
    /// Calls whose `Completed` frame was seen: the post-turn diff uses this
    /// to tell a cut empty partial (interrupted end) from a completed empty
    /// call (clean end).
    pub(crate) completed: Arc<Mutex<HashMap<String, bool>>>,
}

impl ForwardingProvider {
    /// The six live dependencies, named; the per-provider call bookkeeping
    /// (seq, call ids, completion map) is always fresh.
    pub(crate) fn new(
        inner: TurnProviderRef,
        tx: mpsc::Sender<Event>,
        pipe: PipeCounters,
        workspace: String,
        session: String,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            route: Mutex::new(inner),
            tx,
            pipe,
            workspace,
            session,
            stop,
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Point this session's calls at `inner` — the endpoint the model its id
    /// names is addressed at (the `session_set_model` command's live half).
    /// It takes effect at the next call: an in-flight stream keeps the endpoint
    /// it started on, so the switch lands at the turn boundary.
    pub(crate) fn set_route(&self, inner: TurnProviderRef) {
        *self.route.lock() = inner;
    }
}

impl TurnProvider for ForwardingProvider {
    fn forwarding_inner(&self) -> Option<TurnProviderRef> {
        Some(self.route.lock().clone())
    }
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
            inner: sink,
        };
        // Record the call at start, not first event (#67): a call aborted
        // before it emits anything is still a call, and must finalize like
        // any other — a StreamStart now, a StreamEnd at turn settlement.
        forward.calls.lock().push(forward.call_id.clone());
        forward.send(Event::StreamStart {
            workspace: forward.workspace.clone(),
            session: forward.session.clone(),
            call_id: forward.call_id.clone(),
        });
        let inner = self.route.lock().clone();
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
    inner: &'a mut dyn TurnSink,
}

impl TurnSink for ForwardSink<'_> {
    fn event(&mut self, event: TurnEvent) -> bool {
        // Text/reasoning deltas are the loop's sink's: they become the
        // growing entry's snapshots (ADR-0008), not channel events.
        // No `StreamEnd` here: the Completed frame ends the call's stream,
        // not the turn — the turn's om_turn_end pass still runs after it.
        // The end arrives with the post-turn reconciliation, at turn
        // settlement (issue #82). The `completed` note is the
        // reconciliation's cut-vs-clean signal for entry-less calls.
        // A stop cuts the stream at the next delta (the loop records the
        // partial as interrupted, spec §7). Snapshot it before the
        // completed note: a stop landing with the Completed frame means
        // the call was cut, so an entry-less one must read interrupted,
        // not completed, in the reconciliation.
        let stopped = self.stop.load(Ordering::SeqCst);
        if let TurnEvent::Completed(_, _) = &event
            && !stopped
        {
            self.completed.lock().insert(self.call_id.clone(), true);
        }
        if stopped {
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
pub(crate) const fn mode_to_protocol(m: crate::subagent::ContextMode) -> ContextMode {
    match m {
        crate::subagent::ContextMode::Fresh => ContextMode::Fresh,
        crate::subagent::ContextMode::Compacted => ContextMode::Compacted,
        crate::subagent::ContextMode::Fork => ContextMode::Fork,
    }
}

/// The core's child record as the protocol's (the protocol crate is
/// independent of the core — ADR-0002 — so the mapping lives here).
pub(crate) fn info_to_protocol(i: &crate::subagent::SubagentInfo) -> SubagentInfo {
    let state = i.state.kind();
    SubagentInfo {
        handle: i.handle.clone(),
        child: i.child.clone(),
        agent_type: i.agent_type.clone(),
        context_mode: mode_to_protocol(i.context_mode),
        state: state.to_owned(),
        waiting_on: i.waiting_on.map(|w| w.as_str().to_owned()),
        last_message: i.last_message.clone(),
        created: i.created,
        usage: i.usage.as_ref().map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.total_tokens(),
            cached_prompt_tokens: u.cached_input_tokens.unwrap_or(0),
        }),
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

/// The child provider factory (ticket #23 N3): wraps the provider the parent's
/// model is addressed at in the forwarding seam, per child session id.
pub(crate) struct ForwardingChildFactory {
    pub(crate) client: reqwest::Client,
    /// The workspace config: a `.md` agent type names a model of its own, and
    /// that id routes to its own provider (#336) — the parent's entry is only
    /// the fallback a bare key resolves against.
    pub(crate) config: Config,
    pub(crate) provider: crate::config::Provider,
    pub(crate) requests: crate::config::Requests,
    pub(crate) tx: mpsc::Sender<Event>,
    pub(crate) pipe: PipeCounters,
    pub(crate) workspace: String,
    pub(crate) providers: Mutex<HashMap<String, Arc<ForwardingProvider>>>,
}
impl ChildProviderFactory for ForwardingChildFactory {
    fn create(&self, child: &str, model: &str) -> TurnProviderRef {
        // #336: a child is addressed at the provider its model's id names (a
        // `.md` agent type's model, or the parent's), not at the provider the
        // parent happened to open on. A prefix naming no provider is no route
        // — the child keeps the parent's provider, whose id the spawn already
        // resolved.
        let owner = self.config.route_child(
            model,
            ProviderRef {
                name: "",
                provider: &self.provider,
            },
        );
        let provider = Arc::new(ForwardingProvider::new(
            session_inner(&self.client, owner, &self.requests),
            self.tx.clone(),
            self.pipe.clone(),
            self.workspace.clone(),
            child.to_owned(),
            Arc::new(AtomicBool::new(false)),
        ));
        // Stash it so the bridge's spawned reuses this exact provider for the
        // child's LiveSession (one per child, as the root has).
        self.providers
            .lock()
            .insert(child.to_owned(), provider.clone());
        provider
    }
}

impl ForwardingChildFactory {
    /// The concrete provider `create` made for a child, so the bridge's
    /// `LiveSession` shares it with the child's agent (one provider, #175).
    fn take_provider(&self, child: &str) -> Option<Arc<ForwardingProvider>> {
        self.providers.lock().remove(child)
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
            .and_then(|c| c.sessions.lock().get(session).cloned());
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
    pub(crate) factory: Option<Arc<ForwardingChildFactory>>,
}
impl SubagentBridge for SessionSubagentBridge {
    fn spawned(&self, n: &SpawnNotice) {
        // Register the child as a live session so the GUI can open it and
        // the driver can feed it turns.
        let Some(agent) = self
            .sup
            .lock()
            .as_ref()
            .and_then(Weak::upgrade)
            .and_then(|s| s.child_agent(&n.handle))
        else {
            return;
        };
        // The handle is opaque (R3): record which parent minted it — the
        // GUI's handle-carrying commands resolve the parent from the
        // registry, never from the handle's format.
        self.core.register_child_handle(&n.handle, &n.parent);
        let Some(parent_live) = self.core.sessions.lock().get(&n.parent).cloned() else {
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
        // Reuse the child's own provider (the one its agent runs through) so
        // the post-turn reconciliation emits StreamEnd (#175); a non-forwarding
        // test factory falls back to a fresh one.
        let provider = self
            .factory
            .as_ref()
            .and_then(|f| f.take_provider(&n.child))
            .unwrap_or_else(|| {
                Arc::new(ForwardingProvider::new(
                    session_inner(&self.client, &self.provider, &self.requests),
                    self.core.events_tx.clone(),
                    self.core.pipe.clone(),
                    self.workspace.clone(),
                    n.child.clone(),
                    agent.stop_flag(),
                ))
            });
        // Wire the child's event hooks (#173): the child launch path only wires
        // the plan hook, so its streaming entries never reached the GUI.
        let blob_root = std::path::Path::new(&parent_live.cwd).join(".tau");
        super::launch::wire_events(&self.core, &self.workspace, &n.child, &agent, &blob_root);
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
            turn: AtomicBool::new(false),
            provider,
            cwd: parent_live.cwd.clone(),
            skeleton: Mutex::new(Skeleton::default()),
        });
        let id = live.meta.lock().id.clone();
        self.core.sessions.lock().insert(id, live);
    }

    fn reclaimed(&self, n: &ReclaimNotice) {
        // The handle is gone from the supervisor's map: drop the mint record
        // (the session file and the child's live session stay, #250).
        self.core.child_handles.lock().remove(&n.handle);
    }

    fn re_adopted(&self, n: &ReadoptNotice) {
        // The re-adopt re-minted an existing handle: re-record the parent
        // (the reclamation dropped it), so handle-carrying commands resolve
        // it again.
        self.core.register_child_handle(&n.handle, &n.parent);
    }

    fn state(&self, n: &StateNotice) {
        let detail = match &n.state {
            crate::subagent::ChildState::Done { output } => Some(json!({ "output": output })),
            crate::subagent::ChildState::Failed { reason } => Some(json!({ "reason": reason })),
            crate::subagent::ChildState::Idle { waiting_on } => {
                Some(json!({ "waiting_on": waiting_on.as_str() }))
            }
            // `awaiting` is self-describing in its state string.
            crate::subagent::ChildState::Awaiting | crate::subagent::ChildState::Running => None,
            crate::subagent::ChildState::Stopped { by } => {
                let mut d = json!({ "by": by });
                if let Some(rc) = &n.resume_contract {
                    d["resume_contract"] =
                        serde_json::to_value(rc).unwrap_or(serde_json::Value::Null);
                }
                Some(d)
            }
        };
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::State {
                handle: n.handle.clone(),
                child: n.child.clone(),
                state: n.state.kind().to_owned(),
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
        // The report rides the steering lane — a child that finishes mid
        // parent-turn is delivered at this turn's next LLM call, not
        // spliced into a later, unrelated one.
        let Some(live) = self.core.sessions.lock().get(&n.parent).cloned() else {
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
        live.agent.send_notified_steer(text, n.child.clone());
        // The Queue event is a projection of the agent's lane queue (the
        // single source of truth), taken after the wake lands in it.
        self.core.emit(Event::Queue {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            items: live.agent.queued_items(),
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
