//! Sub-agent machinery (spec §5, ADR-0001): in-process concurrent
//! agent-loop instances, each working its own session file linked to the
//! parent. The 5-state lifecycle (running / idle / done / failed /
//! stopped — all non-running states deliberately resumable, nothing
//! auto-resumes), the parent tool surface, the child's `parent_notify`,
//! the one-shot nudge with its three-way fork, and the wake rules.
//!
//! Every transition is a `subagent` entry in the child's session file —
//! the child's file is the record; the in-memory state is its live view.

use crate::agent::{AgentSession, Lane, SessionParams, TurnConfig};
use crate::config::{Om, SubAgents, ToolBatchPolicy};
use crate::om::OmRecord;
use crate::om_integration::{self, OmState};
use crate::provider::{TurnProviderRef, Usage};
use crate::session::SessionStore;
use crate::tools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};


/// The session entry kind for sub-agent lifecycle records (spawn, state
/// transitions, notifies) — the child's session file is the record.
pub const KIND_SUBAGENT: &str = "subagent";

/// The one-shot nudge (ADR-0001): a child whose turn ends without
/// `parent_notify` gets exactly one injected system turn.
pub const NUDGE: &str = "Your turn ended without parent_notify. If you are done, call \
     parent_notify with done:true and your structured output now. If you are parking, \
     call parent_notify stating what you are waiting for (waiting_on: parent | user | \
     subagent).";

/// A parked child's declared wait target (ADR-0001: an explicit
/// `waiting_on` declaration, not a free-text inference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitingOn {
    Parent,
    User,
    Subagent,
}

impl WaitingOn {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::User => "user",
            Self::Subagent => "subagent",
        }
    }
}

/// Who stopped the child (the only asymmetry between stops — ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoppedBy {
    User,
    Parent,
}

impl StoppedBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Parent => "parent",
        }
    }
}

/// The 5-state lifecycle (spec §5; ADR-0001 supplement: all non-running
/// states are deliberately resumable — nothing auto-resumes anything).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    Running,
    Idle {
        waiting_on: WaitingOn,
    },
    Done {
        output: Option<Value>,
    },
    Failed {
        reason: String,
    },
    Stopped {
        by: StoppedBy,
    },
}

impl ChildState {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle { .. } => "idle",
            Self::Done { .. } => "done",
            Self::Failed { .. } => "failed",
            Self::Stopped { .. } => "stopped",
        }
    }
}

/// A spawn's context mode (spec §5.1; the protocol carries its own mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    Fresh,
    Compacted,
    Fork,
}

/// The provider a child's loop runs against, created per child session id
/// (the dispatch implements this to wrap the production provider in its
/// stream-forwarding seam; tests return canned providers).
pub trait ChildProviderFactory: Send + Sync {
    fn create(&self, child_session_id: &str) -> TurnProviderRef;
}

/// The parent-side notice for a child state transition: the GUI badge and
/// snapshot refresh. Idempotent-cumulative — the full state of one handle.
#[derive(Clone)]
pub struct StateNotice {
    pub parent: String,
    pub handle: String,
    pub child: String,
    pub state: ChildState,
    pub note: Option<String>,
}

/// A spawn notice (the protocol's `subagent_spawned` event).
#[derive(Clone)]
pub struct SpawnNotice {
    pub parent: String,
    pub handle: String,
    pub child: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
}

/// A wake (ADR-0001 wake rules): the implementation appends the
/// notification to the parent's active branch (child provenance) and starts
/// a turn there.
#[derive(Clone)]
pub struct WakeNotice {
    pub parent: String,
    pub child: String,
    pub kind: WakeKind,
    pub text: String,
    pub waiting_on: Option<WaitingOn>,
    pub output: Option<Value>,
}

/// What woke the parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeKind {
    /// done — the parent is woken always.
    Done,
    /// failed — the parent is notified always (to investigate).
    Failed,
    /// idle with `waiting_on: parent` — a parked parent is woken with the
    /// child's stated need.
    Waiting,
}

/// The parent-side seam the supervisor calls across (the dispatch
/// implements it: protocol events + the wake).
pub trait SubagentBridge: Send + Sync {
    fn spawned(&self, notice: &SpawnNotice);
    fn state(&self, notice: &StateNotice);
    fn wake(&self, notice: &WakeNotice);
}

/// One child's live record.
pub struct Child {
    pub handle: String,
    pub session_id: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
    pub agent: Arc<AgentSession>,
    state: Mutex<ChildState>,
    nudge_sent: AtomicBool,
    /// The drive's in-flight flag: one drive per child at a time (the
    /// single-writer rule at the sub-agent level).
    turn: AtomicBool,
    last_message: Mutex<Option<String>>,
}

impl Child {
    fn state(&self) -> ChildState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, state: ChildState, note: Option<String>) {
        *self.state.lock().unwrap() = state.clone();
        if let Err(e) = self.agent.append_entry(
            KIND_SUBAGENT,
            json!({ "event": "state", "state": state, "note": note }),
        ) {
            eprintln!("subagent state entry failed: {e}");
        }
    }

    fn set_last_message(&self, text: &str) {
        *self.last_message.lock().unwrap() = Some(text.to_owned());
    }
}

/// The child-side link a child's loop carries: routes `parent_notify` to
/// the child's supervisor.
pub struct ChildLink {
    supervisor: Arc<Supervisor>,
    handle: String,
}

impl ChildLink {
    /// The `parent_notify` tool handler (ticket #23): `done:true` requires
    /// a structured output and ends the child; a note parks it. The
    /// declared `waiting_on` (default: parent — it just messaged the
    /// parent) is the nudge fork's "valid wait declaration".
    pub fn notify(&self, args: &Value) -> String {
        self.supervisor.notify(&self.handle, args)
    }
}

/// A drive's result: `Err` = the child's loop failed (provider/storage) →
/// the child fails and the parent is notified.
pub type BoxedDrive = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// The child turn driver (the dispatch implements it): run the child's
/// queue to completion (the turns) plus the post-turn work, and return
/// when the queue drains.
pub trait ChildDriver: Send + Sync {
    fn drive(&self, session: &str, agent: &Arc<AgentSession>) -> BoxedDrive;
}

/// The supervisor: one per non-child session; owns that session's
/// children. A child session carries no supervisor — the depth cap (a
/// child cannot spawn) is structural, not a check.
pub struct Supervisor {
    parent_session: String,
    /// Set after the parent's own loop is constructed (the constructor
    /// cannot close over its owner).
    parent: Mutex<Option<Arc<AgentSession>>>,
    children: Mutex<HashMap<String, Arc<Child>>>,
    next: AtomicUsize,
    caps: SubAgents,
    cwd: std::path::PathBuf,
    provider: Arc<dyn ChildProviderFactory>,
    model: String,
    system_prompt: String,
    om: Om,
    om_model: String,
    tool_batch_on_force: ToolBatchPolicy,
    turn: TurnConfig,
    bridge: Arc<dyn SubagentBridge>,
    driver: Arc<dyn ChildDriver>,
}

/// The supervisor's construction inputs (the dispatch assembles them from
/// the session's config and the parent's binding).
pub struct SupervisorParams {
    pub parent_session: String,
    pub cwd: std::path::PathBuf,
    pub provider: Arc<dyn ChildProviderFactory>,
    pub model: String,
    pub system_prompt: String,
    pub om: Om,
    pub om_model: String,
    pub tool_batch_on_force: ToolBatchPolicy,
    pub turn: TurnConfig,
    pub caps: SubAgents,
    pub bridge: Arc<dyn SubagentBridge>,
    pub driver: Arc<dyn ChildDriver>,
}

/// A completed spawn: everything the dispatch needs to register the child
/// as an ordinary live session (a child is an ordinary session — ADR-0006)
/// plus the drive's join handle.
impl std::fmt::Debug for Spawned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spawned")
            .field("handle", &self.handle)
            .field("session_id", &self.session_id)
            .finish()
    }
}

pub struct Spawned {
    pub handle: String,
    pub session_id: String,
    pub agent: Arc<AgentSession>,
    pub provider: TurnProviderRef,
    pub stop: Arc<AtomicBool>,
    pub cwd: std::path::PathBuf,
    pub model: String,
    pub created: u64,
    pub drive: tokio::task::JoinHandle<()>,
}

impl Supervisor {
    pub fn new(p: SupervisorParams) -> Arc<Self> {
        Arc::new(Self {
            parent_session: p.parent_session,
            parent: Mutex::new(None),
            children: Mutex::new(HashMap::new()),
            next: AtomicUsize::new(0),
            caps: p.caps,
            cwd: p.cwd,
            provider: p.provider,
            model: p.model,
            system_prompt: p.system_prompt,
            om: p.om,
            om_model: p.om_model,
            tool_batch_on_force: p.tool_batch_on_force,
            turn: p.turn,
            bridge: p.bridge,
            driver: p.driver,
        })
    }

    /// The parent's loop (attached once by the caller after the parent's
    /// `AgentSession` exists).
    pub fn attach_parent(&self, agent: Arc<AgentSession>) {
        *self.parent.lock().unwrap() = Some(agent);
    }

    /// A child's concurrency slot: every child that is not done holds one
    /// (a done child is quiescent; resuming it re-acquires a slot).
    fn live_children(&self) -> Vec<Arc<Child>> {
        self.children
            .lock()
            .unwrap()
            .values()
            .filter(|c| !matches!(c.state(), ChildState::Done { .. }))
            .cloned()
            .collect()
    }

    fn check_cap(&self, extra: bool) -> Result<(), String> {
        if let Some(max) = self.caps.max_concurrent {
            if self.live_children().len() as u32 + extra as u32 > max {
                return Err(format!(
                    "subagent_spawn: concurrency cap ({max}) reached"
                ));
            }
        }
        Ok(())
    }

    /// Spawn a child (spec §5.1): async — this only sets things up; the
    /// child's loop runs on its own task and reports through `parent_notify`.
    pub fn spawn(
        self: &Arc<Self>,
        agent_type: &str,
        brief: &str,
        context_mode: ContextMode,
        task: Option<&str>,
        call_id: &str,
    ) -> Result<Spawned, String> {
        // v0 has exactly one agent type (the built-in `general`); the
        // `.md`-file registry lands with ticket #24.
        if agent_type != "general" {
            return Err(format!(
                "subagent_spawn: unknown agent type {agent_type:?} (v0 has the built-in \"general\")"
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
                let mut source =
                    SessionStore::for_workspace(&self.cwd, &self.parent_session);
                source
                    .open()
                    .map_err(|e| format!("subagent_spawn: cannot fork: {e}"))?;
                SessionStore::fork_from(&source, &session_id)
                    .map_err(|e| format!("subagent_spawn: fork failed: {e}"))?
            }
            _ => {
                let mut store = SessionStore::for_workspace(&self.cwd, &session_id);
                store
                    .create()
                    .map_err(|e| format!("subagent_spawn: {e}"))?;
                store
            }
        };

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

        let handle = format!("{}-{}", self.parent_session, self.next.fetch_add(1, Ordering::SeqCst) + 1);
        let provider = self.provider.create(&session_id);
        let agent = Arc::new(AgentSession::new(SessionParams {
            store,
            // The built-in `general` type inherits the session's prompt,
            // tools, and model (spec §5.5); type-specific prompts land
            // with ticket #24.
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            tools: tools::child_tool_specs(),
            cwd: self.cwd.clone(),
            provider: provider.clone(),
            tool_batch_on_force: self.tool_batch_on_force,
            turn: self.turn,
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
            session_id: session_id.clone(),
            agent_type: agent_type.to_owned(),
            context_mode,
            agent: agent.clone(),
            state: Mutex::new(ChildState::Running),
            nudge_sent: AtomicBool::new(false),
            turn: AtomicBool::new(false),
            last_message: Mutex::new(Some(brief.to_owned())),
        });
        self.children.lock().unwrap().insert(handle.clone(), child.clone());
        self.bridge.spawned(&SpawnNotice {
            parent: self.parent_session.clone(),
            handle: handle.clone(),
            child: session_id.clone(),
            agent_type: agent_type.to_owned(),
            context_mode,
        });

        // The brief is the child's first turn (the handoff-in, ADR-0001).
        child.agent.send(brief, Lane::FollowUp);
        let drive = tokio::spawn(Self::drive_loop(Arc::clone(self), child));
        Ok(Spawned {
            handle,
            session_id,
            agent,
            provider,
            stop,
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            created,
            drive,
        })
    }

    /// The child's drive: run turns until the child leaves the running
    /// state, with the one-shot nudge and the three-way fork (ADR-0001).
    async fn drive_loop(sup: Arc<Supervisor>, child: Arc<Child>) {
        child.turn.store(true, Ordering::SeqCst);
        loop {
            match sup
                .driver
                .drive(&child.session_id, &child.agent)
                .await
            {
                Ok(()) => {}
                Err(reason) => {
                    sup.mark_failed(&child, reason);
                    break;
                }
            }
            // The drive drained the queue: whatever the child's state is
            // now, only `Running` keeps the loop (done / idle / failed /
            // stopped are all deliberate rest states — the child is
            // resumable, and nothing here resumes it).
            if !matches!(child.state(), ChildState::Running) {
                break;
            }
            // A turn ended without `parent_notify`: the one-shot nudge.
            if !child.nudge_sent.swap(true, Ordering::SeqCst) {
                let _ = child.agent.append_entry(
                    crate::agent::KIND_SYSTEM,
                    json!({ "note": "nudge: state what you are waiting for, or finish" }),
                );
                child.agent.send(NUDGE, Lane::FollowUp);
                continue;
            }
            // Nudge exhausted: the three-way fork's remaining arm.
            sup.mark_failed(
                &child,
                "nudge exhausted: the child ended a turn without done or a valid wait declaration".to_owned(),
            );
            break;
        }
        child.turn.store(false, Ordering::SeqCst);
    }

    fn mark_failed(&self, child: &Arc<Child>, reason: String) {
        child.set_state(ChildState::Failed { reason: reason.clone() }, None);
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state: ChildState::Failed { reason: reason.clone() },
            note: Some(reason.clone()),
        });
        // A failed child auto-notifies the parent to investigate (ADR-0001
        // wake rules: failed is always woken).
        self.bridge.wake(&WakeNotice {
            parent: self.parent_session.clone(),
            child: child.session_id.clone(),
            kind: WakeKind::Failed,
            text: reason,
            waiting_on: None,
            output: None,
        });
    }

    /// The child-side `parent_notify` (routed here from the child's loop).
    fn notify(&self, handle: &str, args: &Value) -> String {
        let Some(child) = self.children.lock().unwrap().get(handle).cloned() else {
            return format!("parent_notify: unknown child {handle}");
        };
        let Some(text) = args.get("text").and_then(Value::as_str) else {
            return "parent_notify: missing \"text\"".into();
        };
        let done = args.get("done").and_then(Value::as_bool).unwrap_or(false);
        let output = args.get("output").cloned();
        let waiting_on = match args.get("waiting_on").and_then(Value::as_str) {
            None => None,
            Some("parent") => Some(WaitingOn::Parent),
            Some("user") => Some(WaitingOn::User),
            Some("subagent") => Some(WaitingOn::Subagent),
            Some(other) => {
                return format!("parent_notify: waiting_on must be parent | user | subagent, got {other:?}");
            }
        };

        if done && !matches!(output, Some(Value::Object(_))) {
            // done:true requires a structured output (the handoff-out,
            // ADR-0001: the child can never finish without one).
            return "parent_notify: done:true requires an object output".into();
        }
        if !done && output.is_some() {
            return "parent_notify: output requires done:true".into();
        }

        let mut payload = json!({ "event": "notify", "text": text, "done": done });
        if let Some(output) = &output {
            payload["output"] = output.clone();
        }
        if let Some(w) = waiting_on {
            payload["waiting_on"] = json!(w.as_str());
        }
        if let Err(e) = child.agent.append_entry(KIND_SUBAGENT, payload) {
            return format!("parent_notify: recording the note failed: {e}");
        }
        child.set_last_message(text);

        if done {
            // Quiescence, not death (ADR-0001): the loop ends, the
            // concurrency slot frees, and the parent is woken always.
            child.set_state(ChildState::Done { output: output.clone() }, None);
            self.bridge.state(&StateNotice {
                parent: self.parent_session.clone(),
                handle: child.handle.clone(),
                child: child.session_id.clone(),
                state: ChildState::Done { output: output.clone() },
                note: None,
            });
            self.bridge.wake(&WakeNotice {
                parent: self.parent_session.clone(),
                child: child.session_id.clone(),
                kind: WakeKind::Done,
                text: text.to_owned(),
                waiting_on: None,
                output,
            });
            "done: the parent has your result".into()
        } else {
            // A note parks the child. The declared (or default) wait
            // target decides the wake (ADR-0001 wake rules).
            let waiting_on = waiting_on.unwrap_or(WaitingOn::Parent);
            child.set_state(ChildState::Idle { waiting_on }, Some(text.to_owned()));
            self.bridge.state(&StateNotice {
                parent: self.parent_session.clone(),
                handle: child.handle.clone(),
                child: child.session_id.clone(),
                state: ChildState::Idle { waiting_on },
                note: Some(text.to_owned()),
            });
            if waiting_on == WaitingOn::Parent {
                // A parked parent (including a user-stopped one) is woken
                // with the child's stated need; user/subagent waits are
                // GUI-badge only.
                self.bridge.wake(&WakeNotice {
                    parent: self.parent_session.clone(),
                    child: child.session_id.clone(),
                    kind: WakeKind::Waiting,
                    text: text.to_owned(),
                    waiting_on: Some(WaitingOn::Parent),
                    output: None,
                });
            }
            "noted: you are parked".into()
        }
    }

    /// `subagent_message` (and the GUI's send to a child): running → the
    /// text queues on the child's lane; non-running → resume (text omitted
    /// = a pure resume). One tool, state decides (ADR-0006).
    pub fn message(self: &Arc<Self>, handle: &str, text: Option<String>, lane: Lane) -> Result<String, String> {
        let child = self.children.lock().unwrap().get(handle).cloned().ok_or_else(|| {
            format!("subagent_message: no sub-agent {handle} in this session")
        })?;
        match child.state() {
            ChildState::Running => match &text {
                Some(text) => {
                    child.agent.send(text.clone(), lane);
                    child.set_last_message(text);
                    Ok(format!("delivered to {handle}"))
                }
                None => Ok(format!("sub-agent {handle} is already running")),
            },
            _ => {
                // Resume from any non-running state (ADR-0001 supplement:
                // all non-running states are deliberately resumable; the
                // only asymmetry is the provenance the stop recorded).
                if matches!(child.state(), ChildState::Done { .. }) {
                    self.check_cap(true)?;
                }
                let text = text.unwrap_or_else(|| "Continue from where you stopped.".to_owned());
                child.set_state(ChildState::Running, Some(text.clone()));
                self.bridge.state(&StateNotice {
                    parent: self.parent_session.clone(),
                    handle: child.handle.clone(),
                    child: child.session_id.clone(),
                    state: ChildState::Running,
                    note: Some("resumed".into()),
                });
                child.agent.send(text, Lane::FollowUp);
                // One drive at a time: a second concurrent resume just
                // queues — the in-flight drive absorbs the message.
                if child.turn.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
                {
                    tokio::spawn(Self::drive_loop(Arc::clone(self), child));
                }
                Ok(format!("resumed sub-agent {handle}"))
            }
        }
    }

    /// `subagent_stop` (and the GUI's stop, and a parent's
    /// close/delete of its children): soft pause — the in-flight stream is
    /// cut, the partial kept, provenance recorded, all states resumable.
    pub fn stop(self: &Arc<Self>, handle: &str, by: StoppedBy) -> Result<String, String> {
        let child = self
            .children
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or_else(|| format!("subagent_stop: no sub-agent {handle} in this session"))?;
        if matches!(child.state(), ChildState::Running) {
            // Cuts the child's stream at the next delta; the loop records
            // the partial as an interrupted entry.
            child.agent.stop();
        }
        let note = format!("stopped by {}", by.as_str());
        child.set_state(ChildState::Stopped { by }, Some(note.clone()));
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state: ChildState::Stopped { by },
            note: Some(note),
        });
        Ok(format!("stopped sub-agent {handle}"))
    }

    /// Soft-stop every live child (the parent session was closed or
    /// deleted: no work runs for a session that no longer exists). Done
    /// children are left as-is — their record is complete and they stay
    /// resumable as standalone sessions.
    pub fn stop_all(self: &Arc<Self>, by: StoppedBy) {
        for child in self.live_children() {
            let _ = self.stop(&child.handle, by);
        }
    }

    /// All registered children (the snapshot's live-state handles).
    pub fn handles(&self) -> Vec<String> {
        self.children.lock().unwrap().keys().cloned().collect()
    }

    /// A child's structured inspection (the `subagent_state` tool and the
    /// protocol's `subagent_state` command).
    pub fn state_info(&self, handle: &str) -> Option<SubagentInfo> {
        let child = self.children.lock().unwrap().get(handle)?.clone();
        let (state, waiting_on) = match child.state() {
            ChildState::Idle { waiting_on } => (ChildState::Idle { waiting_on }, Some(waiting_on)),
            other => (other, None),
        };
        let usage = {
            let mut store = SessionStore::for_workspace(&self.cwd, &child.session_id);
            store
                .open()
                .ok()
                .and_then(|_| store.leaf().ok().flatten())
                .and_then(|leaf| {
                    let entries = store.entries_range(0, usize::MAX).ok()?;
                    let branch = om_integration::branch_entries(&entries, Some(&leaf.id));
                    branch
                        .iter()
                        .rev()
                        .find(|e| e.kind == crate::agent::KIND_ASSISTANT)
                        .and_then(|e| e.payload.get("usage"))
                        .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok())
                })
        };
        Some(SubagentInfo {
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            agent_type: child.agent_type.clone(),
            context_mode: child.context_mode,
            state,
            waiting_on,
            last_message: child.last_message.lock().unwrap().clone(),
            usage,
            // Ticket #24 fills the task linkage (the per-session task
            // record and its resume contract); v0 carries null.
            task: None,
            resume_contract: None,
        })
    }
}

/// One child's structured state (the protocol carries its own mirror).
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentInfo {
    pub handle: String,
    /// The child's session id — a child is an ordinary session.
    pub child: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
    pub state: ChildState,
    pub waiting_on: Option<WaitingOn>,
    pub last_message: Option<String>,
    pub usage: Option<Usage>,
    pub task: Option<Value>,
    pub resume_contract: Option<Value>,
}

/// Route a tool call at the parent's sub-agent surface (called from the
/// loop's tool dispatch; diagnostics are results, never panics).
pub fn route_parent(sup: &Arc<Supervisor>, tc: &tools::ToolCall) -> String {
    match tc.name.as_str() {
        "subagent_spawn" => {
            let Some(agent_type) = tc.args.get("type").and_then(Value::as_str) else {
                return "subagent_spawn: missing \"type\"".into();
            };
            let Some(brief) = tc.args.get("brief").and_then(Value::as_str) else {
                return "subagent_spawn: missing \"brief\"".into();
            };
            let context_mode = match tc.args.get("context_mode").and_then(Value::as_str) {
                Some("compacted") => ContextMode::Compacted,
                Some("fork") => ContextMode::Fork,
                _ => ContextMode::Fresh,
            };
            let task = tc.args.get("task").and_then(Value::as_str);
            match sup.spawn(agent_type, brief, context_mode, task, &tc.id) {
                Ok(s) => format!(
                    "spawned sub-agent {} (session {}); it reports back via parent_notify",
                    s.handle, s.session_id
                ),
                Err(e) => e,
            }
        }
        "subagent_message" => {
            let Some(handle) = tc.args.get("handle").and_then(Value::as_str) else {
                return "subagent_message: missing \"handle\"".into();
            };
            let text = tc.args.get("text").and_then(Value::as_str).map(str::to_owned);
            match sup.message(handle, text, Lane::Steering) {
                Ok(s) => s,
                Err(e) => e,
            }
        }
        "subagent_stop" => {
            let Some(handle) = tc.args.get("handle").and_then(Value::as_str) else {
                return "subagent_stop: missing \"handle\"".into();
            };
            match sup.stop(handle, StoppedBy::Parent) {
                Ok(s) => s,
                Err(e) => e,
            }
        }
        "subagent_state" => {
            let Some(handle) = tc.args.get("handle").and_then(Value::as_str) else {
                return "subagent_state: missing \"handle\"".into();
            };
            match sup.state_info(handle) {
                Some(info) => format!(
                    "{}: {}{} — session {}",
                    info.handle,
                    info.state.kind(),
                    info.waiting_on
                        .map(|w| format!(" (waiting on {})", w.as_str()))
                        .unwrap_or_default(),
                    info.child
                ),
                None => format!("subagent_state: no sub-agent {handle} in this session"),
            }
        }
        // The core tools route through the loop's normal dispatch.
        other => format!("unknown tool: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::provider::{self, canned, ResponseRequest, TurnSink};
    use crate::session::SessionStore;

    // ── test seams ──────────────────────────────────────────────────────

    #[derive(Default)]
    struct TestBridge {
        spawns: Mutex<Vec<SpawnNotice>>,
        states: Mutex<Vec<StateNotice>>,
        wakes: Mutex<Vec<WakeNotice>>,
    }

    impl SubagentBridge for TestBridge {
        fn spawned(&self, n: &SpawnNotice) {
            self.spawns.lock().unwrap().push(n.clone());
        }
        fn state(&self, n: &StateNotice) {
            self.states.lock().unwrap().push(n.clone());
        }
        fn wake(&self, n: &WakeNotice) {
            self.wakes.lock().unwrap().push(n.clone());
        }
    }

    struct TestDriver;

    impl ChildDriver for TestDriver {
        fn drive(&self, _session: &str, agent: &Arc<AgentSession>) -> BoxedDrive {
            let agent = Arc::clone(agent);
            Box::pin(async move { agent.process().await.map_err(|e| e.to_string()) })
        }
    }

    /// Per-child canned scripts: the k-th child created gets scripts[k]
    /// (falls back to the last script).
    struct CannedFactory {
        scripts: Vec<Vec<String>>,
        created: AtomicUsize,
    }

    impl ChildProviderFactory for CannedFactory {
        fn create(&self, _id: &str) -> TurnProviderRef {
            let slot = self.created.fetch_add(1, Ordering::SeqCst);
            let scripts = self.scripts.get(slot).or(self.scripts.last()).cloned().unwrap_or_default();
            Arc::new(CannedChildProvider {
                scripts,
                index: AtomicUsize::new(0),
            })
        }
    }

    struct CannedChildProvider {
        scripts: Vec<String>,
        index: AtomicUsize,
    }
    impl CannedChildProvider {
        fn next(&self) -> String {
            let i = self.index.fetch_add(1, Ordering::SeqCst);
            self.scripts
                .get(i % self.scripts.len())
                .cloned()
                .unwrap_or_else(|| sse("", &[]))
        }
    }
    impl provider::TurnProvider for CannedChildProvider {
        fn call<'a>(&self, req: &ResponseRequest, sink: &'a mut dyn TurnSink) -> provider::ProviderTurn<'a> {
            let body = self.next();
            provider::canned(&body).call(req, sink)
        }
    }

    /// (name, call_id, arguments-json) → a canned SSE body.
    fn sse(text: &str, calls: &[(String, String, String)]) -> String {
        let mut body = String::new();
        for (name, call_id, args) in calls {
            let item = json!({
                "id": call_id, "type": "function_call", "name": name,
                "call_id": call_id, "arguments": args,
            });
            body.push_str(&format!(
                "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\n"
            ));
        }
        if !text.is_empty() {
            let delta = json!({ "type": "response.output_text.delta", "delta": text });
            body.push_str(&format!("data: {delta}\n\n"));
        }
        body.push_str(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        );
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn notify_call(call_id: &str, text: &str, done: bool, output: Option<Value>, waiting: Option<&str>) -> (String, String, String) {
        let mut args = json!({ "text": text });
        if done {
            args["done"] = json!(true);
        }
        if let Some(o) = output {
            args["output"] = o;
        }
        if let Some(w) = waiting {
            args["waiting_on"] = json!(w);
        }
        (
            "parent_notify".into(),
            call_id.into(),
            args.to_string(),
        )
    }

    /// A parent session + supervisor wired to the test seams. The parent
    /// agent is scripted (its provider); the children use the factory.
    fn harness(
        dir: &std::path::Path,
        parent_bodies: Vec<String>,
        child_scripts: Vec<Vec<String>>,
        caps: SubAgents,
    ) -> (Arc<Supervisor>, Arc<TestBridge>) {
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: child_scripts,
            created: AtomicUsize::new(0),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps,
            bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        });
        // A scripted parent loop (the parent is a full session).
        let mut store = SessionStore::for_workspace(dir, "parent");
        store.create().unwrap();
        let parent_provider = Arc::new(ScriptedProvider::new(parent_bodies));
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::agent_tool_specs(),
            cwd: dir.to_path_buf(),
            provider: parent_provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: Some(Arc::clone(&sup)),
            child: None,
        }));
        sup.attach_parent(parent);
        (sup, bridge)
    }

    /// A scripted parent provider: one canned body per call.
    struct ScriptedProvider {
        bodies: Vec<String>,
        index: AtomicUsize,
    }
    impl ScriptedProvider {
        fn new(bodies: Vec<String>) -> Self {
            Self {
                bodies,
                index: AtomicUsize::new(0),
            }
        }
    }
    impl provider::TurnProvider for ScriptedProvider {
        fn call<'a>(&self, req: &ResponseRequest, sink: &'a mut dyn TurnSink) -> provider::ProviderTurn<'a> {
            let i = self.index.fetch_add(1, Ordering::SeqCst);
            let body = self.bodies.get(i % self.bodies.len()).cloned().unwrap_or_else(|| sse("", &[]));
            provider::canned(&body).call(req, sink)
        }
    }

    fn spawn_args(brief: &str, mode: &str) -> Value {
        json!({ "type": "general", "brief": brief, "context_mode": mode })
    }

    fn entry_by_event(dir: &std::path::Path, session: &str, event: &str) -> Option<crate::session::Entry> {
        let mut store = SessionStore::for_workspace(dir, session);
        store.open().unwrap();
        store
            .entries_range(0, usize::MAX)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == json!(event))
    }

    // ── the acceptance bar ─────────────────────────────────────────────

    /// The ticket's scripted acceptance: a parent spawns two children
    /// concurrently; one finishes (parent woken with the validated
    /// result), one is stopped then resumed by message, one
    /// nudge-exhausts to failed — and every transition is a session entry
    /// with a protocol (bridge) event.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_lifecycle_acceptance_flow() {
        let dir = tempfile::tempdir().unwrap();
        // Parent script: spawn three children (one call each), then end.
        let parent_bodies = vec![
            sse(
                "",
                &[("subagent_spawn".into(), "c1".into(), spawn_args("do A", "fresh").to_string())],
            ),
            sse(
                "",
                &[("subagent_spawn".into(), "c2".into(), spawn_args("do B", "fresh").to_string())],
            ),
            sse(
                "",
                &[("subagent_spawn".into(), "c3".into(), spawn_args("do C", "fresh").to_string())],
            ),
            sse("all spawned", &[]),
        ];
        // Per-child scripts: A → done; B → a note (park), then a resumed
        // turn that finishes; C → two bare turn-ends (nudge, then failed).
        let child_scripts = vec![
            vec![sse(
                "",
                &[notify_call("n1", "A is done", true, Some(json!({"answer": 42})), None)],
            )],
            vec![
                sse("", &[notify_call("n2", "B needs input", false, None, None)]),
                sse(
                    "",
                    &[notify_call("n3", "B finished after resume", true, Some(json!({"ok": true})), None)],
                ),
            ],
            vec![sse("", &[]), sse("", &[])],
        ];
        let (sup, bridge) = harness(dir.path(), parent_bodies, child_scripts, SubAgents::default());

        // The parent's loop runs the spawns (tools dispatch in-loop).
        sup_spawn_from_parent(&sup).await;

        let handle_a = "parent-1".to_owned();
        let handle_b = "parent-2".to_owned();
        let handle_c = "parent-3".to_owned();

        // A: done — the parent is woken with the schema-validated output.
        wait_for(|| matches!(sup.state_info(&handle_a).unwrap().state, ChildState::Done { .. }));
        let wake = bridge.wakes.lock().unwrap().iter().find(|w| w.kind == WakeKind::Done).cloned().expect("done wakes the parent");
        assert_eq!(wake.output, Some(json!({"answer": 42})));
        // The transition is a session entry in the child's file.
        let entry = entry_by_event(dir.path(), &sup.state_info(&handle_a).unwrap().child, "state");
        assert!(entry.is_some(), "the done transition is a session entry");

        // B: parked by a note (default waiting_on = parent → a wake).
        wait_for(|| matches!(sup.state_info(&handle_b).unwrap().state, ChildState::Idle { .. }));
        assert!(bridge.wakes.lock().unwrap().iter().any(|w| w.kind == WakeKind::Waiting));
        // Resumed by a message (the tool's non-running branch).
        sup.message(&handle_b, Some("proceed".into()), Lane::Steering).unwrap();
        wait_for(|| matches!(sup.state_info(&handle_b).unwrap().state, ChildState::Done { .. }));

        // C: nudge-exhausted → failed, and the parent is notified.
        wait_for(|| matches!(sup.state_info(&handle_c).unwrap().state, ChildState::Failed { .. }));
        assert!(
            bridge.wakes.lock().unwrap().iter().any(|w| w.kind == WakeKind::Failed),
            "a failed child auto-notifies the parent"
        );
        // The nudge is a session entry (one-shot).
        let c_info = sup.state_info(&handle_c).unwrap();
        let mut cstore = SessionStore::for_workspace(dir.path(), &c_info.child);
        cstore.open().unwrap();
        let centries = cstore.entries_range(0, usize::MAX).unwrap();
        let nudges: Vec<_> = centries
            .iter()
            .filter(|e| e.kind == crate::agent::KIND_SYSTEM && e.payload["note"].as_str() == Some("nudge: state what you are waiting for, or finish"))
            .collect();
        assert_eq!(nudges.len(), 1, "one nudge per parked state, no loops");

        // Every spawn produced a bridge event; the state stream saw the
        // full transition set.
        assert_eq!(bridge.spawns.lock().unwrap().len(), 3);
        let states: Vec<&str> = bridge.states.lock().unwrap().iter().map(|n| n.state.kind()).collect();
        assert!(states.contains(&"done") && states.contains(&"idle") && states.contains(&"failed"));
    }

    /// Drive the parent's loop: it calls the spawn tools in its script.
    async fn sup_spawn_from_parent(sup: &Arc<Supervisor>) {
        // The harness's parent agent: find it via the supervisor's parent.
        let parent = sup.parent.lock().unwrap().clone().unwrap();
        parent.send("go", Lane::FollowUp);
        parent.process().await.unwrap();
    }

    fn wait_for(cond: impl Fn() -> bool) {
        let start = std::time::Instant::now();
        while !cond() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // ── wake rules ──────────────────────────────────────────────────────

    /// The wake-rule matrix: done always, failed always, idle-parent
    /// wakes a parked parent, idle-user / idle-subagent are badge-only.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_wake_rule_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let child_scripts = vec![
            vec![sse("", &[notify_call("n1", "waiting for the user", false, None, Some("user"))])],
            vec![sse("", &[notify_call("n2", "waiting for my child", false, None, Some("subagent"))])],
            vec![sse("", &[notify_call("n3", "waiting for the parent", false, None, Some("parent"))])],
        ];
        let (sup, bridge) = harness(
            dir.path(),
            vec![sse("spawning", &[])],
            child_scripts,
            SubAgents::default(),
        );
        for i in 0..3 {
            let r = sup.spawn(
                "general",
                &format!("child {i}"),
                ContextMode::Fresh,
                None,
                "c0",
            );
            r.unwrap();
        }
        wait_for(|| {
            (0..3)
                .all(|i| matches!(
                    sup.state_info(&format!("parent-{}", i + 1)).unwrap().state,
                    ChildState::Idle { .. }
                ))
        });
        let wakes = bridge.wakes.lock().unwrap();
        // Exactly one wake: the waiting_on=parent child.
        assert_eq!(wakes.len(), 1, "only the parent-declared child wakes the parent");
        assert_eq!(wakes[0].kind, WakeKind::Waiting);
        assert_eq!(wakes[0].waiting_on, Some(WaitingOn::Parent));
    }

    // ── stop / resume ───────────────────────────────────────────────────

    /// A running child is soft-stopped (the stream cut, the partial kept
    /// as interrupted) and resumed by a message from a non-running state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stopped_child_keeps_its_partial_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        // Child 1: a slow stream (cut by the stop), then after resume a
        // turn that finishes.
        let slow = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
        );
        let (sup, bridge) = harness(
            dir.path(),
            vec![sse("spawning", &[])],
            vec![vec![
                slow.to_owned(),
                sse("", &[notify_call("n1", "done", true, Some(json!({"r": 1})), None)]),
            ]],
            SubAgents::default(),
        );
        let s = sup.spawn("general", "slow work", ContextMode::Fresh, None, "c0").unwrap();
        let drive = s.drive;
        // Let the slow stream start, then stop it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        sup.stop(&s.handle, StoppedBy::User).unwrap();
        drive.await.unwrap();
        // The partial stands as an interrupted entry in the child's file.
        let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
        store.open().unwrap();
        let entries = store.entries_range(0, usize::MAX).unwrap();
        let interrupted = entries
            .iter()
            .find(|e| e.kind == crate::agent::KIND_ASSISTANT && e.payload["interrupted"] == json!(true))
            .expect("the partial is kept as interrupted");
        assert!(interrupted.payload["text"].as_str().unwrap().contains("partial"));
        assert!(matches!(sup.state_info(&s.handle).unwrap().state, ChildState::Stopped { by: StoppedBy::User }));
        assert!(bridge.states.lock().unwrap().iter().any(|n| n.state == ChildState::Stopped { by: StoppedBy::User }));

        // Resume from the stopped state: the message starts the next turn.
        sup.message(&s.handle, Some("continue".into()), Lane::Steering).unwrap();
        wait_for(|| matches!(sup.state_info(&s.handle).unwrap().state, ChildState::Done { .. }));
        // The resume is a session entry (the state transition is durable).
        assert!(
            entry_by_event(dir.path(), &s.session_id, "state")
                .map(|e| e.payload["state"] == json!({"state": "running"}))
                .unwrap_or(false),
            "the resume transition is recorded"
        );
    }

    // ── caps and structural depth ───────────────────────────────────────

    /// The concurrency cap is enforced; a child's tool set carries no
    /// spawn tool (the depth cap is structural).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_caps() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, _) = harness(
            dir.path(),
            vec![sse("x", &[])],
            vec![vec![sse("", &[notify_call("n1", "note", false, None, Some("user"))])]],
            SubAgents {
                max_depth: 1,
                max_concurrent: Some(1),
            },
        );
        let first = sup.spawn("general", "one", ContextMode::Fresh, None, "c0").unwrap();
        // Second spawn while the first holds its slot: refused.
        let err = sup
            .spawn("general", "two", ContextMode::Fresh, None, "c1")
            .expect_err("second spawn while the first holds its slot");
        assert!(err.contains("concurrency cap"), "{err}");
        // The child's tool set: parent_notify in, subagent_spawn out.
        let tools = tools::child_tool_specs().iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert!(tools.contains(&"parent_notify".to_owned()));
        assert!(!tools.contains(&"subagent_spawn".to_owned()));
        // Free the slot: the done child no longer counts.
        sup.message(&first.handle, Some("finish".into()), Lane::Steering).unwrap();
        wait_for(|| matches!(sup.state_info(&first.handle).unwrap().state, ChildState::Done { .. }));
        let second = sup.spawn("general", "two", ContextMode::Fresh, None, "c1");
        assert!(second.is_ok(), "a done child frees its slot: {:?}", second.err());
        // Unknown agent types are refused (v0 has only `general`).
        assert!(sup.spawn("planner", "x", ContextMode::Fresh, None, "c2").is_err());
    }

    // ── context modes ───────────────────────────────────────────────────

    /// A compacted spawn seeds the parent's observation log verbatim as
    /// the child's frozen prefix (a `spawn-snapshot` entry with the
    /// parent pointer).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_compacted_spawn_seeds_the_frozen_prefix() {
        let dir = tempfile::tempdir().unwrap();
        // The parent carries an OM record with observations.
        let (sup, _) = harness(
            dir.path(),
            vec![sse("obs turn", &[])],
            vec![vec![sse("", &[])]],
            SubAgents::default(),
        );
        let parent = sup.parent.lock().unwrap().clone().unwrap();
        parent.set_om(Some(OmState::from_config(
                &Om::default(),
                OmRecord {
                    frozen_prefix: String::new(),
                    active_observations: "<observations>the parent's log</observations>".into(),
                    ..Default::default()
                },
            )));
        let s = sup.spawn("general", "compact me", ContextMode::Compacted, None, "c0").unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
        store.open().unwrap();
        // The spawn-snapshot entry (the record in the child's file).
        let snapshot = store
            .entries_range(0, usize::MAX)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == om_integration::KIND_SPAWN_SNAPSHOT)
            .expect("a spawn-snapshot entry links the frozen prefix to the parent");
        assert_eq!(snapshot.payload["parentSession"], "parent");
        assert!(snapshot.payload["log"].as_str().unwrap().contains("the parent's log"));
        // The child's own record: the prefix verbatim, empty suffix.
        let record = OmState::load_record(&mut store).unwrap();
        assert_eq!(record.frozen_prefix, "<observations>the parent's log</observations>");
        assert!(record.active_observations.is_empty());
    }

    /// A fork copies the parent's entry tree (stable ids) and inherits the
    /// parent's record wholesale.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_fork_copies_the_tree_and_inherits_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, _) = harness(
            dir.path(),
            vec![sse("working", &[])],
            vec![vec![sse("", &[])]],
            SubAgents::default(),
        );
        let parent = sup.parent.lock().unwrap().clone().unwrap();
        // A user entry on the parent (the fork must copy it).
        parent
            .append_entry(
                crate::agent::KIND_USER,
                json!({ "text": "work 1", "lane": "follow-up" }),
            )
            .unwrap();
        // A record on the parent.
        parent.set_om(Some(OmState::from_config(
                &Om::default(),
                OmRecord {
                    frozen_prefix: "FROZEN".into(),
                    active_observations: "SUFFIX".into(),
                    ..Default::default()
                },
            )));
        let s = sup.spawn("general", "fork me", ContextMode::Fork, None, "c0").unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
        store.open().unwrap();
        let child_entries = store.entries_range(0, usize::MAX).unwrap();
        // The parent's user entry is present with its stable id.
        let mut pstore = SessionStore::for_workspace(dir.path(), "parent");
        pstore.open().unwrap();
        let parent_entries = pstore.entries_range(0, usize::MAX).unwrap();
        let parent_user = parent_entries.iter().find(|e| e.kind == "user").expect("the parent has a user entry");
        assert!(
            child_entries.iter().any(|e| e.id == parent_user.id),
            "the fork carries the parent's entries with stable ids"
        );
        // The record: the fork owns the whole log (prefix undemoted into
        // the suffix), ADR-0004.
        let record = OmState::load_record(&mut store).unwrap();
        assert_eq!(record.frozen_prefix, "");
        assert_eq!(record.active_observations, "FROZENSUFFIX");
    }

    // ── notify validation ───────────────────────────────────────────────

    /// The output/done rules: output without done is rejected, done
    /// without an object output is rejected, a bad waiting_on is
    /// rejected.
    #[tokio::test]
    async fn notify_rejects_invalid_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, _) = harness(dir.path(), vec![sse("x", &[])], vec![vec![sse("", &[])]], SubAgents::default());
        // A child session file + link: notify validation is checked before
        // any state change.
        let mut store = SessionStore::for_workspace(dir.path(), "child");
        store.create().unwrap();
        let link = Arc::new(ChildLink {
            supervisor: sup,
            handle: "child-1".into(),
        });
        assert!(link.notify(&json!({ "text": "t", "output": json!({}) })).contains("output requires done:true"));
        assert!(link.notify(&json!({ "text": "t", "done": true })).contains("done:true requires an object output"));
        assert!(link.notify(&json!({ "text": "t", "done": true, "output": json!([1]) })).contains("object output"));
        assert!(link.notify(&json!({ "text": "t", "waiting_on": "the void" })).contains("waiting_on must be"));
    }
}
