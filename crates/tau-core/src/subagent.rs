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
use std::path::Path;
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
    Idle { waiting_on: WaitingOn },
    Done { output: Option<Value> },
    Failed { reason: String },
    Stopped { by: StoppedBy },
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
    /// A stopped child's assigned task rides its resume contract (spec
    /// §5.2); None for every other transition.
    pub resume_contract: Option<Value>,
}

/// A spawn notice (the protocol's `subagent_spawned` event).
#[derive(Clone)]
pub struct SpawnNotice {
    pub parent: String,
    pub handle: String,
    pub child: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
    /// The child's model (the dispatch registers the child's live session
    /// with it).
    pub model: String,
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
    /// stopped — the parent is told the child was stopped (the note names
    /// who), so its model sees the child as stopped, not running.
    Stopped,
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
    last_message: Mutex<Option<String>>,
    /// Drive generation: each drive spawn bumps it; a superseded drive's
    /// loop sees the mismatch and bails before running another round (the
    /// stop-then-resume window between `drive()` returning and the loop's
    /// state check — two drives must never run the child concurrently).
    drive_gen: AtomicUsize,
    /// The generation of the last drive whose loop exited: the archive
    /// path quiesces a stopped child on this before moving its file (the
    /// stop's interrupted entry must land in the file, not a recreated
    /// fragment).
    last_ended_gen: AtomicUsize,
    /// The idle drive sleeps on this (review N8): a message or a stop
    /// wakes it, so an idle child costs no polling. `Notify`'s permit
    /// makes a wake that lands before the drive arms its future unlost.
    wake: tokio::sync::Notify,
}

impl Child {
    fn state(&self) -> ChildState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, state: ChildState, note: Option<String>) -> Result<(), String> {
        *self.state.lock().unwrap() = state.clone();
        self.agent
            .append_entry(
                KIND_SUBAGENT,
                json!({ "event": "state", "state": state, "note": note }),
            )
            .map_err(|e| format!("subagent state entry failed: {e}"))
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
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// The `parent_notify` tool handler (ticket #23): `done:true` requires
    /// a structured output and ends the child; a note parks it. The
    /// declared `waiting_on` (default: parent — it just messaged the
    /// parent) is the nudge fork's "valid wait declaration".
    pub fn notify(&self, args: &Value) -> String {
        self.supervisor.notify(&self.handle, args)
    }

    /// Whether the child is still `Running` (the loop's quiescence check,
    /// ADR-0001: the core auto-terminates a child's loop on done).
    pub fn is_running(&self) -> bool {
        self.supervisor.child_running(&self.handle)
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
    /// This session's depth (top-level = 0): a child's depth is depth + 1,
    /// capped by `max_depth` (spawn fails when it would exceed it). The
    /// structural cap (a child carries no supervisor) already bounds v0
    /// depth to 1; this knob additionally disables spawning at 0.
    depth: u32,
    /// The agent-type registry (spec §5.5).
    types: Vec<crate::agent_type::AgentType>,
    /// Set after the parent's own loop is constructed (the constructor
    /// cannot close over its owner).
    parent: Mutex<Option<Arc<AgentSession>>>,
    /// The children map (review N8): a terminal child (done/stopped/failed)
    /// stays in it until the parent session closes — every non-running
    /// state is resumable, so the record must outlive its drive. Bounded
    /// by the spawn count for the session's life; a long-lived-session
    /// prune would need resume to re-adopt from the session file.
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
    /// The session's depth (top-level = 0); a spawn fails when the child's
    /// depth (depth + 1) would exceed `max_depth`.
    pub depth: u32,
    /// The agent-type registry (spec §5.5): `general` first, then the
    /// discovered `.md` types.
    pub types: Vec<crate::agent_type::AgentType>,
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
            types: p.types,
            depth: p.depth,
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

    /// Whether a child is still `Running` (the child-side loop's exit
    /// check, ADR-0001).
    pub fn child_running(&self, handle: &str) -> bool {
        self.children
            .lock()
            .unwrap()
            .get(handle)
            .map(|c| matches!(c.state(), ChildState::Running))
            .unwrap_or(false)
    }

    /// A child's concurrency slot: every child that is not done holds one
    /// A child's concurrency slot: every child with a live drive holds
    /// one; a quiescent child (done/stopped/failed — its drive is gone)
    /// holds none, and a resume of it re-acquires a slot.
    fn live_children(&self) -> Vec<Arc<Child>> {
        self.children
            .lock()
            .unwrap()
            .values()
            .filter(|c| {
                !matches!(
                    c.state(),
                    ChildState::Done { .. }
                        | ChildState::Stopped { .. }
                        | ChildState::Failed { .. }
                )
            })
            .cloned()
            .collect()
    }

    fn check_cap(&self, extra: bool) -> Result<(), String> {
        match self.caps.max_concurrent {
            Some(max) if self.live_children().len() as u32 + extra as u32 > max => {
                Err(format!("subagent_spawn: concurrency cap ({max}) reached"))
            }
            _ => Ok(()),
        }
    }

    /// The child's live agent (the dispatch registers it as an ordinary
    /// live session — a child is a session, ADR-0006).
    pub fn child_agent(&self, handle: &str) -> Option<Arc<AgentSession>> {
        self.children
            .lock()
            .unwrap()
            .get(handle)
            .map(|c| c.agent.clone())
    }

    /// Assign one of this session's tasks to a child (spec §5.3): the
    /// record copies into the child's session, which becomes the live
    /// record; the creator's copy becomes the status pointer. Both sides
    /// run through the sessions' own stores — one writer per session,
    /// never a second store on a live file (review B3). The copy is then
    /// delivered through the child's message path: a running child takes
    /// it as a steering round on its next call, a non-running child
    /// resumes with it (ADR-0001) — a bare copy races a running loop.
    pub fn assign_task(
        self: &Arc<Self>,
        task_id: &str,
        worker_session: &str,
    ) -> Result<(), String> {
        if task_id.is_empty() {
            return Err("task_assign: missing \"task\"".into());
        }
        let child = self
            .children
            .lock()
            .unwrap()
            .values()
            .find(|c| c.session_id == worker_session)
            .cloned()
            .ok_or_else(|| {
                format!("task_assign: {worker_session} is not a sub-agent of this session")
            })?;
        let parent = self
            .parent
            .lock()
            .unwrap()
            .clone()
            .ok_or("task_assign: no parent attached".to_owned())?;
        parent
            .with_task_store(|cstore| {
                child.agent.with_task_store(|wstore| {
                    crate::task::assign(
                        cstore,
                        wstore,
                        task_id,
                        worker_session,
                        &self.parent_session,
                    )
                })
            })
            .map(|_| ())?;
        let title = parent
            .with_task_store(|store| {
                crate::task::fold_entries(&store.entries_range(0, usize::MAX).unwrap_or_default())
                    .into_iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.title)
            })
            .unwrap_or_default();
        let note = if title.is_empty() {
            format!("Assigned task {task_id}")
        } else {
            format!("Assigned {task_id}: {title}")
        };
        self.message(&child.handle, Some(note), Lane::Steering)
            .map(|_| ())
    }

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
    async fn drive_loop(sup: Arc<Supervisor>, child: Arc<Child>, drive_gen: usize) {
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
                        if child
                            .set_state(
                                ChildState::Running,
                                Some("resumed by a queued message".into()),
                            )
                            .is_err()
                        {
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

    fn mark_failed(&self, child: &Arc<Child>, reason: String) {
        // The state entry itself can fail (storage down): nothing further
        // to do — the in-memory state already says failed.
        if let Err(e) = child.set_state(
            ChildState::Failed {
                reason: reason.clone(),
            },
            None,
        ) {
            eprintln!("subagent state entry failed: {e}");
        }
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state: ChildState::Failed {
                reason: reason.clone(),
            },
            note: Some(reason.clone()),
            resume_contract: None,
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
        // Shape-only validation (v0, review N9): the keys and their types
        // are checked, not a full schema — a semantic schema would be
        // speculative until the task gate lands. A malformed call is
        // rejected before the child lookup, even for an unknown handle.
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
                return format!(
                    "parent_notify: waiting_on must be parent | user | subagent, got {other:?}"
                );
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

        let Some(child) = self.children.lock().unwrap().get(handle).cloned() else {
            return format!("parent_notify: unknown child {handle}");
        };
        // A repeated `done` is a no-op (ADR-0001 quiescence): the child has
        // already terminated — no second state entry, no second wake.
        if matches!(child.state(), ChildState::Done { .. }) {
            return "already done: the parent has your result".into();
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
            // The task gate (spec §5.3): a done child cannot leave its
            // assigned task dangling in_progress — the resolution is
            // forced (completed / handed_off / blocked), and the
            // creator's pointer is mirrored. Runs before the state set so
            // the Done record and the wake see the final task state.
            self.resolve_assigned_task(&child, &output);
            // Quiescence, not death (ADR-0001): the loop ends, the
            // concurrency slot frees, and the parent is woken always.
            if let Err(e) = child.set_state(
                ChildState::Done {
                    output: output.clone(),
                },
                None,
            ) {
                return e;
            }
            self.bridge.state(&StateNotice {
                parent: self.parent_session.clone(),
                handle: child.handle.clone(),
                child: child.session_id.clone(),
                state: ChildState::Done {
                    output: output.clone(),
                },
                note: None,
                resume_contract: None,
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
            if let Err(e) = child.set_state(ChildState::Idle { waiting_on }, Some(text.to_owned()))
            {
                return e;
            }
            self.bridge.state(&StateNotice {
                parent: self.parent_session.clone(),
                handle: child.handle.clone(),
                child: child.session_id.clone(),
                state: ChildState::Idle { waiting_on },
                note: Some(text.to_owned()),
                resume_contract: None,
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

    /// The child's assigned task, resolved on done (spec §5.3): the
    /// completion gate runs on the child's own store (the live record),
    /// and the creator's pointer is mirrored on the parent's own store —
    /// one store per session, never a second store on a live file.
    ///
    /// - all criteria satisfied → `done`
    /// - not satisfied, not blocked → `handed_off` (stays in_progress;
    ///   the output becomes the resume contract, the parent decides)
    /// - already blocked (the child called `task_block`) → stays blocked
    fn resolve_assigned_task(&self, child: &Child, output: &Option<Value>) {
        let resolved = child.agent.with_task_store(|store| {
            let entries = store.entries_range(0, usize::MAX).ok()?;
            let tasks = crate::task::fold_entries(&entries);
            // The live record is the task with a creator link (in v0 a
            // child carries at most one assigned task).
            let task = tasks.into_iter().find(|t| t.created_in.is_some())?;
            let output = output.as_ref().unwrap_or(&Value::Null);
            let status = if task.status == crate::task::STATUS_IN_PROGRESS {
                let gate = task
                    .criteria
                    .iter()
                    .all(|c| c.status == crate::task::CriterionStatus::Satisfied);
                if gate {
                    crate::task::finish(store, &task.id, false, None)
                } else {
                    crate::task::handoff(store, &task.id, output)
                }
                .ok()?
                .status
            } else {
                task.status.clone()
            };
            Some((task.id, status))
        });
        if let Some((id, status)) = resolved
            && let Some(parent) = self.parent.lock().unwrap().clone()
        {
            parent.with_task_store(|store| {
                // The creator's copy tracks the worker's state; a creator
                // without the copy is a no-op (mirror_status handles it).
                let _ = crate::task::mirror_status(store, &id, &status);
            });
        }
    }
    /// `subagent_message` (and the GUI's send to a child): running → the
    /// text queues on the child's lane; non-running → resume (text omitted
    /// = a pure resume). One tool, state decides (ADR-0006).
    pub fn message(
        self: &Arc<Self>,
        handle: &str,
        text: Option<String>,
        lane: Lane,
    ) -> Result<String, String> {
        let child = self
            .children
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or_else(|| format!("subagent_message: no sub-agent {handle} in this session"))?;
        match child.state() {
            ChildState::Running => match &text {
                Some(text) => {
                    child.agent.send(text.clone(), lane);
                    child.set_last_message(text);
                    child.wake.notify_one();
                    Ok(format!("delivered to {handle}"))
                }
                None => Ok(format!("sub-agent {handle} is already running")),
            },
            _ => {
                // Resume from any non-running state (ADR-0001 supplement:
                // all non-running states are deliberately resumable; the
                // only asymmetry is the provenance the stop recorded).
                let was_quiescent = matches!(
                    *child.state.lock().unwrap(),
                    ChildState::Stopped { .. }
                        | ChildState::Failed { .. }
                        | ChildState::Done { .. }
                );
                // A quiescent child holds no slot; the resume re-acquires
                // one against the cap.
                if was_quiescent {
                    self.check_cap(true)?;
                }
                let text = text.unwrap_or_else(|| "Continue from where you stopped.".to_owned());
                child.set_state(ChildState::Running, Some(text.clone()))?;
                child.wake.notify_one();
                self.bridge.state(&StateNotice {
                    parent: self.parent_session.clone(),
                    handle: child.handle.clone(),
                    child: child.session_id.clone(),
                    state: ChildState::Running,
                    note: Some("resumed".into()),
                    resume_contract: None,
                });
                child.agent.send(text, Lane::FollowUp);
                // A drive ended by stop/failure/done is gone: this resume
                // starts a fresh one (only an idle drive persists and
                // wakes on its own). The nudge budget resets with the new
                // work period.
                if was_quiescent {
                    let drive_gen = child.drive_gen.fetch_add(1, Ordering::SeqCst) + 1;
                    tokio::spawn(Self::drive_loop(Arc::clone(self), child.clone(), drive_gen));
                }
                child.nudge_sent.store(false, Ordering::SeqCst);
                Ok(format!("resumed sub-agent {handle}"))
            }
        }
    }

    /// `subagent_stop` (and the GUI's stop): a running child is stopped —
    /// the in-flight stream is cut, the partial kept, a terminal `Stopped{by}`
    /// recorded (ADR-0001: deliberately resumable), its concurrency slot
    /// freed, and the parent woken. A non-running child is a no-op with a
    /// clear message — it already rests in the state that records how it
    /// got there, and a second stop would overwrite it.
    pub fn stop(self: &Arc<Self>, handle: &str, by: StoppedBy) -> Result<String, String> {
        let child = self
            .children
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or_else(|| format!("subagent_stop: no sub-agent {handle} in this session"))?;
        if !matches!(child.state(), ChildState::Running) {
            return match child.state() {
                ChildState::Idle { .. } => Ok(format!(
                    "sub-agent {handle} is parked — a message resumes it"
                )),
                other => Ok(format!("sub-agent {handle} is already {}", other.kind())),
            };
        }
        // Cuts the child's stream at the next delta; the loop records
        // the partial as an interrupted entry.
        child.agent.stop();
        self.finish_stop(&child, by)
    }

    /// The stop's shared tail (the user stop and the parent's close/archive
    /// both end a drive this way): the terminal Stopped record, the
    /// drive's wake, the state event (with the task's resume contract),
    /// and the parent's wake.
    fn finish_stop(&self, child: &Arc<Child>, by: StoppedBy) -> Result<String, String> {
        let handle = child.handle.clone();
        let note = format!("stopped by {}", by.as_str());
        child.set_state(ChildState::Stopped { by }, Some(note.clone()))?;
        // The drive ends at its loop head (running: after the interrupted
        // entry lands; parked: it wakes from its sleep and exits).
        child.wake.notify_one();
        // A stopped child with an assigned task carries the task's resume
        // contract in the stop event (spec §5.2).
        let resume_contract = child.agent.with_task_store(|store| {
            let entries = store.entries_range(0, usize::MAX).ok()?;
            crate::task::fold_entries(&entries)
                .into_iter()
                .find(|t| t.created_in.is_some())
                .filter(|t| {
                    t.status == crate::task::STATUS_IN_PROGRESS
                        || t.status == crate::task::STATUS_BLOCKED
                })
                .map(|t| crate::task::resume_contract(&t))
        });
        self.bridge.state(&StateNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
            state: ChildState::Stopped { by },
            note: Some(note.clone()),
            resume_contract: resume_contract.clone(),
        });
        // The parent is told the child is stopped: its model's view of the
        // delegation must not keep the child running.
        self.bridge.wake(&WakeNotice {
            parent: self.parent_session.clone(),
            child: child.session_id.clone(),
            kind: WakeKind::Stopped,
            text: note,
            waiting_on: None,
            output: None,
        });
        match resume_contract {
            Some(rc) => Ok(format!(
                "stopped sub-agent {handle}; its assigned task stays live (resume contract: {rc})"
            )),
            None => Ok(format!("stopped sub-agent {handle}")),
        }
    }

    /// Soft-stop every child that still holds a drive (the parent session
    /// was closed, deleted, or archived: no work runs for a session that
    /// no longer exists): a running child is stopped as `stop` does, and
    /// a parked (idle) child's sleeping drive is ended — `stop` leaves a
    /// parked child alone, but a dead parent must not leave a drive behind.
    /// Quiescent children (done/stopped/failed) are left as-is — their
    /// record is complete and they stay resumable as standalone sessions.
    pub fn stop_all(self: &Arc<Self>, by: StoppedBy) {
        for child in self.live_children() {
            if matches!(child.state(), ChildState::Running) {
                child.agent.stop();
            }
            let _ = self.finish_stop(&child, by);
        }
    }

    /// Whether the child's newest drive has exited (the archive's settle
    /// check: a stopped child's drive must be gone before its file moves).
    pub fn drive_quiescent(&self, handle: &str) -> bool {
        let children = self.children.lock().unwrap();
        let Some(child) = children.get(handle) else {
            return true;
        };
        child.last_ended_gen.load(Ordering::SeqCst) == child.drive_gen.load(Ordering::SeqCst)
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
        // The child's assigned task + its resume contract (spec §5.1):
        // read from the child's own entries (read-only — the writer stays
        // the child's session).
        let (task, resume_contract) = {
            let mut store = SessionStore::for_workspace(&self.cwd, &child.session_id);
            store
                .open()
                .ok()
                .and_then(|_| store.entries_range(0, usize::MAX).ok())
                .and_then(|entries| {
                    let tasks = crate::task::fold_entries(&entries);
                    let t = tasks.into_iter().find(|t| t.created_in.is_some())?;
                    Some((
                        serde_json::to_value(&t).ok(),
                        Some(crate::task::resume_contract(&t)),
                    ))
                })
                .unwrap_or((None, None))
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
            task,
            resume_contract,
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
            let context_mode =
                tc.args
                    .get("context_mode")
                    .and_then(Value::as_str)
                    .map(|m| match m {
                        "compacted" => ContextMode::Compacted,
                        "fork" => ContextMode::Fork,
                        _ => ContextMode::Fresh,
                    });
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
            let text = tc
                .args
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned);
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

/// True when a session file in the workspace already carries this title
/// (the header is the record, so the check reads the disk).
fn title_on_disk(cwd: &Path, title: &str) -> bool {
    let dir = cwd.join(".tau").join("sessions");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return false;
    };
    rd.filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .any(|e| {
            let id = e
                .file_name()
                .to_string_lossy()
                .to_string()
                .trim_end_matches(".jsonl")
                .to_string();
            let mut s = SessionStore::for_workspace(cwd, &id);
            s.open().is_ok() && s.title() == Some(title)
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{self, ResponseRequest, TurnSink};
    use crate::session::SessionStore;
    use std::time::Duration;

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
    /// (falls back to the last script); delays likewise (default: none).
    struct CannedFactory {
        scripts: Vec<Vec<String>>,
        delays: Vec<Duration>,
        created: AtomicUsize,
        /// Provider calls across every child (the quiescence test's oracle).
        calls: Arc<AtomicUsize>,
    }

    impl ChildProviderFactory for CannedFactory {
        fn create(&self, _id: &str) -> TurnProviderRef {
            let slot = self.created.fetch_add(1, Ordering::SeqCst);
            let scripts = self
                .scripts
                .get(slot)
                .or(self.scripts.last())
                .cloned()
                .unwrap_or_default();
            let pre_delay = self.delays.get(slot).copied().unwrap_or(Duration::ZERO);
            Arc::new(CannedChildProvider {
                scripts,
                index: AtomicUsize::new(0),
                pre_delay,
                calls: self.calls.clone(),
            })
        }
    }

    struct CannedChildProvider {
        scripts: Vec<String>,
        index: AtomicUsize,
        pre_delay: Duration,
        calls: Arc<AtomicUsize>,
    }
    impl CannedChildProvider {
        fn next(&self) -> String {
            // No modulo: an exhausted script ends in bare turns, so a
            // child's scripted turn always terminates.
            let i = self.index.fetch_add(1, Ordering::SeqCst);
            self.scripts.get(i).cloned().unwrap_or_else(|| sse("", &[]))
        }
    }
    impl provider::TurnProvider for CannedChildProvider {
        fn call<'a>(
            &self,
            _req: &ResponseRequest,
            sink: &'a mut dyn TurnSink,
        ) -> provider::ProviderTurn<'a> {
            let body = self.next();
            self.calls.fetch_add(1, Ordering::SeqCst);
            let delay = self.pre_delay;
            let (events, calls) = provider::decode_stream(&body).unwrap();
            Box::pin(async move {
                let mut result = provider::TurnResult::default();
                let mut accepted = 0usize;
                for (i, event) in events.iter().cloned().enumerate() {
                    // A mid-stream pause: the first event lands, then the
                    // stream stalls long enough for a stop to cut it.
                    if i == 1 && delay > Duration::ZERO {
                        tokio::time::sleep(delay).await;
                    }
                    if !sink.event(event.clone()) {
                        break;
                    }
                    provider::fold_event(&event, &mut result);
                    accepted += 1;
                }
                if accepted == events.len() {
                    result.completed = events
                        .iter()
                        .any(|e| matches!(e, provider::TurnEvent::Completed(_)));
                }
                result.calls = calls
                    .iter()
                    .filter(|(i, _)| *i <= accepted)
                    .map(|(_, c)| c.clone())
                    .collect();
                Ok(result)
            })
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

    fn notify_call(
        call_id: &str,
        text: &str,
        done: bool,
        output: Option<Value>,
        waiting: Option<&str>,
    ) -> (String, String, String) {
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
        ("parent_notify".into(), call_id.into(), args.to_string())
    }

    /// A parent session + supervisor wired to the test seams. The parent
    /// agent is scripted (its provider); the children use the factory.
    fn harness(
        dir: &std::path::Path,
        parent_bodies: Vec<String>,
        child_scripts: Vec<Vec<String>>,
        caps: SubAgents,
    ) -> (Arc<Supervisor>, Arc<TestBridge>) {
        let (sup, bridge, _) = harness_full(dir, parent_bodies, child_scripts, vec![], caps);
        (sup, bridge)
    }

    fn harness_full(
        dir: &std::path::Path,
        parent_bodies: Vec<String>,
        child_scripts: Vec<Vec<String>>,
        delays: Vec<Duration>,
        caps: SubAgents,
    ) -> (Arc<Supervisor>, Arc<TestBridge>, Arc<CannedFactory>) {
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: child_scripts,
            delays,
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let provider: Arc<dyn ChildProviderFactory> = factory.clone();
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.to_path_buf(),
            provider,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps,
            depth: 0,
            types: vec![crate::agent_type::builtin_general()],
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
        (sup, bridge, factory)
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
        fn call<'a>(
            &self,
            req: &ResponseRequest,
            sink: &'a mut dyn TurnSink,
        ) -> provider::ProviderTurn<'a> {
            let i = self.index.fetch_add(1, Ordering::SeqCst);
            let body = self
                .bodies
                .get(i % self.bodies.len())
                .cloned()
                .unwrap_or_else(|| sse("", &[]));
            provider::canned(&body).call(req, sink)
        }
    }

    fn spawn_args(brief: &str, mode: &str) -> Value {
        json!({ "type": "general", "brief": brief, "context_mode": mode })
    }

    fn entry_by_event(
        dir: &std::path::Path,
        session: &str,
        event: &str,
    ) -> Option<crate::session::Entry> {
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
                &[(
                    "subagent_spawn".into(),
                    "c1".into(),
                    spawn_args("do A", "fresh").to_string(),
                )],
            ),
            sse(
                "",
                &[(
                    "subagent_spawn".into(),
                    "c2".into(),
                    spawn_args("do B", "fresh").to_string(),
                )],
            ),
            sse(
                "",
                &[(
                    "subagent_spawn".into(),
                    "c3".into(),
                    spawn_args("do C", "fresh").to_string(),
                )],
            ),
            sse("all spawned", &[]),
        ];
        // Per-child scripts: A → done; B → a note (park), then a resumed
        // turn that finishes; C → two bare turn-ends (nudge, then failed).
        let child_scripts = vec![
            vec![
                sse(
                    "",
                    &[notify_call(
                        "n1",
                        "A is done",
                        true,
                        Some(json!({"answer": 42})),
                        None,
                    )],
                ),
                sse("", &[]),
            ],
            vec![
                sse("", &[notify_call("n2", "B needs input", false, None, None)]),
                sse("", &[]),
                sse(
                    "",
                    &[notify_call(
                        "n3",
                        "B finished after resume",
                        true,
                        Some(json!({"ok": true})),
                        None,
                    )],
                ),
                sse("", &[]),
            ],
            vec![sse("", &[]), sse("", &[])],
        ];
        let (sup, bridge) = harness(
            dir.path(),
            parent_bodies,
            child_scripts,
            SubAgents::default(),
        );

        // The parent's loop runs the spawns (tools dispatch in-loop).
        sup_spawn_from_parent(&sup).await;

        let handle_a = "parent-1".to_owned();
        let handle_b = "parent-2".to_owned();
        let handle_c = "parent-3".to_owned();

        // A: done — the parent is woken with the schema-validated output.
        wait_for(|| {
            matches!(
                sup.state_info(&handle_a).unwrap().state,
                ChildState::Done { .. }
            )
        });
        let wake = bridge
            .wakes
            .lock()
            .unwrap()
            .iter()
            .find(|w| w.kind == WakeKind::Done)
            .cloned()
            .expect("done wakes the parent");
        assert_eq!(wake.output, Some(json!({"answer": 42})));
        // The transition is a session entry in the child's file.
        let entry = entry_by_event(
            dir.path(),
            &sup.state_info(&handle_a).unwrap().child,
            "state",
        );
        assert!(entry.is_some(), "the done transition is a session entry");

        // B: parked by a note (default waiting_on = parent → a wake).
        wait_for(|| {
            matches!(
                sup.state_info(&handle_b).unwrap().state,
                ChildState::Idle { .. }
            )
        });
        assert!(
            bridge
                .wakes
                .lock()
                .unwrap()
                .iter()
                .any(|w| w.kind == WakeKind::Waiting)
        );
        // Resumed by a message (the tool's non-running branch).
        sup.message(&handle_b, Some("proceed".into()), Lane::Steering)
            .unwrap();
        wait_for(|| {
            matches!(
                sup.state_info(&handle_b).unwrap().state,
                ChildState::Done { .. }
            )
        });

        // C: nudge-exhausted → failed, and the parent is notified.
        wait_for(|| {
            matches!(
                sup.state_info(&handle_c).unwrap().state,
                ChildState::Failed { .. }
            )
        });
        assert!(
            bridge
                .wakes
                .lock()
                .unwrap()
                .iter()
                .any(|w| w.kind == WakeKind::Failed),
            "a failed child auto-notifies the parent"
        );
        // The nudge is a session entry (one-shot).
        let c_info = sup.state_info(&handle_c).unwrap();
        let mut cstore = SessionStore::for_workspace(dir.path(), &c_info.child);
        cstore.open().unwrap();
        let centries = cstore.entries_range(0, usize::MAX).unwrap();
        let nudges: Vec<_> = centries
            .iter()
            .filter(|e| {
                e.kind == crate::agent::KIND_SYSTEM
                    && e.payload["note"].as_str()
                        == Some("Nudge: state what you are waiting for, or finish")
            })
            .collect();
        assert_eq!(nudges.len(), 1, "one nudge per parked state, no loops");

        // Every spawn produced a bridge event; the state stream saw the
        // full transition set.
        assert_eq!(bridge.spawns.lock().unwrap().len(), 3);
        let states: Vec<&str> = bridge
            .states
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.state.kind())
            .collect();
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
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting"
            );
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
            vec![
                sse(
                    "",
                    &[notify_call(
                        "n1",
                        "waiting for the user",
                        false,
                        None,
                        Some("user"),
                    )],
                ),
                sse("", &[]),
            ],
            vec![
                sse(
                    "",
                    &[notify_call(
                        "n2",
                        "waiting for my child",
                        false,
                        None,
                        Some("subagent"),
                    )],
                ),
                sse("", &[]),
            ],
            vec![
                sse(
                    "",
                    &[notify_call(
                        "n3",
                        "waiting for the parent",
                        false,
                        None,
                        Some("parent"),
                    )],
                ),
                sse("", &[]),
            ],
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
                Some(ContextMode::Fresh),
                None,
                "c0",
            );
            r.unwrap();
        }
        wait_for(|| {
            (0..3).all(|i| {
                matches!(
                    sup.state_info(&format!("parent-{}", i + 1)).unwrap().state,
                    ChildState::Idle { .. }
                )
            })
        });
        let wakes = bridge.wakes.lock().unwrap();
        // Exactly one wake: the waiting_on=parent child.
        assert_eq!(
            wakes.len(),
            1,
            "only the parent-declared child wakes the parent"
        );
        assert_eq!(wakes[0].kind, WakeKind::Waiting);
        assert_eq!(wakes[0].waiting_on, Some(WaitingOn::Parent));
    }

    /// A child whose script is only `parent_notify{done}` — no trailing
    /// entries — must quiesce with exactly one provider call, one Done
    /// state entry, and one parent wake (review B2: the loop used to
    /// re-call the model forever, re-appending the Done entry and
    /// re-issuing the wake every round).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_done_child_quiesces_with_exactly_one_call() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, bridge, factory) = harness_full(
            dir.path(),
            vec![sse("spawning", &[])],
            vec![vec![sse(
                "",
                &[notify_call(
                    "n1",
                    "done",
                    true,
                    Some(json!({ "ok": true })),
                    None,
                )],
            )]],
            vec![],
            SubAgents::default(),
        );
        let s = sup
            .spawn("general", "one call", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
        s.drive.await.unwrap();
        assert_eq!(
            factory.calls.load(Ordering::SeqCst),
            1,
            "a done child makes exactly one provider call"
        );
        let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
        store.open().unwrap();
        let done_entries: Vec<_> = store
            .entries_range(0, usize::MAX)
            .unwrap()
            .into_iter()
            .filter(|e| {
                e.kind == KIND_SUBAGENT
                    && e.payload["event"] == json!("state")
                    && e.payload["state"] == json!({ "state": "done", "output": { "ok": true } })
            })
            .collect();
        assert_eq!(done_entries.len(), 1, "exactly one Done state entry");
        assert_eq!(
            bridge
                .wakes
                .lock()
                .unwrap()
                .iter()
                .filter(|w| w.kind == WakeKind::Done)
                .count(),
            1,
            "exactly one Done wake"
        );
    }

    /// The done-display invariant (the user's 'done shows idle' report):
    /// the Done transition is the child's final state event — the wake
    /// that follows it never resets the record, and `state_info` (the
    /// snapshot's and the GUI badge's source) keeps saying done.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_done_child_stays_done_after_its_wake() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, bridge) = harness(
            dir.path(),
            vec![
                sse("parent keeps working", &[]),
                sse("second parent turn", &[]),
            ],
            vec![vec![sse(
                "",
                &[notify_call(
                    "n1",
                    "done",
                    true,
                    Some(json!({ "ok": true })),
                    None,
                )],
            )]],
            SubAgents::default(),
        );
        let s = sup
            .spawn("general", "finish", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
        s.drive.await.unwrap();
        assert!(
            matches!(
                sup.state_info(&s.handle).unwrap().state,
                ChildState::Done { .. }
            ),
            "the record must keep saying done"
        );
        // The state stream for the handle ends at Done: nothing the wake
        // triggers may overwrite it.
        let states: Vec<&str> = bridge
            .states
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n.handle == s.handle)
            .map(|n| n.state.kind())
            .collect();
        assert_eq!(
            states.last().copied(),
            Some("done"),
            "the child's last state event is the done one: {states:?}"
        );
        assert!(
            bridge
                .wakes
                .lock()
                .unwrap()
                .iter()
                .any(|w| w.kind == WakeKind::Done),
            "done wakes the parent"
        );
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
        let (sup, bridge, _) = harness_full(
            dir.path(),
            vec![sse("spawning", &[])],
            vec![vec![
                slow.to_owned(),
                sse(
                    "",
                    &[notify_call("n1", "done", true, Some(json!({"r": 1})), None)],
                ),
                sse("", &[]),
            ]],
            vec![Duration::from_millis(200)],
            SubAgents::default(),
        );
        let s = sup
            .spawn("general", "slow work", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
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
            .find(|e| {
                e.kind == crate::agent::KIND_ASSISTANT && e.payload["interrupted"] == json!(true)
            })
            .expect("the partial is kept as interrupted");
        assert!(
            interrupted.payload["text"]
                .as_str()
                .unwrap()
                .contains("partial")
        );
        assert!(matches!(
            sup.state_info(&s.handle).unwrap().state,
            ChildState::Stopped {
                by: StoppedBy::User
            }
        ));
        assert!(bridge.states.lock().unwrap().iter().any(|n| n.state
            == ChildState::Stopped {
                by: StoppedBy::User
            }));

        // Resume from the stopped state: the message starts the next turn.
        sup.message(&s.handle, Some("continue".into()), Lane::Steering)
            .unwrap();
        wait_for(|| {
            matches!(
                sup.state_info(&s.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        // The resume is a session entry (the state transition is durable).
        let mut cstore = SessionStore::for_workspace(dir.path(), &s.session_id);
        cstore.open().unwrap();
        let entries = cstore.entries_range(0, usize::MAX).unwrap();
        assert!(
            entries.iter().any(|e| e.kind == KIND_SUBAGENT
                && e.payload["event"] == json!("state")
                && e.payload["state"] == json!({"state": "running"})),
            "the resume transition is recorded"
        );
    }

    /// A running child is stopped: the partial kept, the terminal Stopped
    /// recorded, the parent woken, and the concurrency slot freed — a
    /// second spawn at the cap succeeds while the stopped child rests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_running_child_is_stopped_and_releases_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        let slow = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n"
        );
        let (sup, bridge, _) = harness_full(
            dir.path(),
            vec![sse("spawning", &[])],
            vec![
                vec![slow.to_owned(), sse("", &[])],
                vec![
                    sse(
                        "",
                        &[notify_call(
                            "n1",
                            "done",
                            true,
                            Some(json!({ "ok": true })),
                            None,
                        )],
                    ),
                    sse("", &[]),
                ],
            ],
            vec![Duration::from_millis(200), Duration::ZERO],
            SubAgents {
                max_depth: 1,
                max_concurrent: Some(1),
            },
        );
        let first = sup
            .spawn("general", "slow", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
        let drive = first.drive;
        // Let the slow stream start, then stop it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let out = sup.stop(&first.handle, StoppedBy::User).unwrap();
        assert!(out.contains("stopped"), "{out}");
        drive.await.unwrap();
        assert!(
            matches!(
                sup.state_info(&first.handle).unwrap().state,
                ChildState::Stopped {
                    by: StoppedBy::User
                }
            ),
            "the stopped state is recorded with its provenance"
        );
        assert!(bridge.states.lock().unwrap().iter().any(|n| n.state
            == ChildState::Stopped {
                by: StoppedBy::User
            }));
        assert!(
            bridge
                .wakes
                .lock()
                .unwrap()
                .iter()
                .any(|w| w.kind == WakeKind::Stopped),
            "the stop wakes the parent"
        );
        // A second stop of the stopped child is a no-op.
        let out = sup.stop(&first.handle, StoppedBy::User).unwrap();
        assert!(out.contains("already stopped"), "{out}");
        // The slot is freed: a second child spawns at the cap and finishes.
        let second = sup
            .spawn("general", "second", Some(ContextMode::Fresh), None, "c1")
            .unwrap();
        second.drive.await.unwrap();
        assert!(
            matches!(
                sup.state_info(&second.handle).unwrap().state,
                ChildState::Done { .. }
            ),
            "the freed slot carried a real spawn"
        );
    }

    /// A stop of a non-running child is a no-op with a clear message:
    /// the state that records how it got there (done's output, the stop's
    /// provenance) is never overwritten.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_non_running_child_refuses_a_second_stop() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, bridge) = harness(
            dir.path(),
            vec![sse("spawning", &[])],
            vec![
                vec![sse(
                    "",
                    &[notify_call(
                        "n1",
                        "done",
                        true,
                        Some(json!({ "ok": true })),
                        None,
                    )],
                )],
                vec![
                    sse(
                        "",
                        &[notify_call("n1", "parked", false, None, Some("user"))],
                    ),
                    sse(
                        "",
                        &[notify_call(
                            "n2",
                            "done",
                            true,
                            Some(json!({ "ok": true })),
                            None,
                        )],
                    ),
                ],
            ],
            SubAgents::default(),
        );
        let done_child = sup
            .spawn("general", "one", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
        wait_for(|| {
            matches!(
                sup.state_info(&done_child.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        let out = sup.stop(&done_child.handle, StoppedBy::User).unwrap();
        assert!(out.contains("already done"), "{out}");
        let parked = sup
            .spawn("general", "two", Some(ContextMode::Fresh), None, "c1")
            .unwrap();
        wait_for(|| {
            matches!(
                sup.state_info(&parked.handle).unwrap().state,
                ChildState::Idle { .. }
            )
        });
        // A parked child is not running: the stop is a no-op and the park
        // stands.
        let out = sup.stop(&parked.handle, StoppedBy::User).unwrap();
        assert!(out.contains("parked"), "{out}");
        assert!(matches!(
            sup.state_info(&parked.handle).unwrap().state,
            ChildState::Idle { .. }
        ));
        // A message resumes it; its next scripted turn finishes the child.
        sup.message(&parked.handle, Some("go".into()), Lane::Steering)
            .unwrap();
        wait_for(|| {
            matches!(
                sup.state_info(&parked.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        let out = sup.stop(&parked.handle, StoppedBy::User).unwrap();
        assert!(out.contains("already done"), "{out}");
        // The done child's record is untouched: exactly one state event.
        let done_states: Vec<&str> = bridge
            .states
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n.handle == done_child.handle)
            .map(|n| n.state.kind())
            .collect();
        assert_eq!(done_states, vec!["done"], "{done_states:?}");
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
            vec![
                vec![
                    sse("", &[notify_call("n1", "note", false, None, Some("user"))]),
                    sse("", &[]),
                    sse(
                        "",
                        &[notify_call(
                            "n2",
                            "done",
                            true,
                            Some(json!({"ok": true})),
                            None,
                        )],
                    ),
                    sse("", &[]),
                ],
                vec![
                    sse(
                        "",
                        &[notify_call(
                            "n3",
                            "done",
                            true,
                            Some(json!({"ok": true})),
                            None,
                        )],
                    ),
                    sse("", &[]),
                ],
            ],
            SubAgents {
                max_depth: 1,
                max_concurrent: Some(1),
            },
        );
        let first = sup
            .spawn("general", "one", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
        // Second spawn while the first holds its slot: refused.
        let err = sup
            .spawn("general", "two", Some(ContextMode::Fresh), None, "c1")
            .expect_err("second spawn while the first holds its slot");
        assert!(err.contains("concurrency cap"), "{err}");
        // The child's tool set: parent_notify in, subagent_spawn out.
        let tools = tools::child_tool_specs()
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>();
        assert!(tools.contains(&"parent_notify".to_owned()));
        assert!(!tools.contains(&"subagent_spawn".to_owned()));
        // Free the slot: the done child no longer counts. Wait for the
        // park first so the resume is deterministic (a message delivered
        // mid-brief-turn would be a steering, consumed by that turn).
        wait_for(|| {
            matches!(
                sup.state_info(&first.handle).unwrap().state,
                ChildState::Idle { .. }
            )
        });
        sup.message(&first.handle, Some("finish".into()), Lane::Steering)
            .unwrap();
        wait_for(|| {
            matches!(
                sup.state_info(&first.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        let second = sup.spawn("general", "two", Some(ContextMode::Fresh), None, "c1");
        assert!(
            second.is_ok(),
            "a done child frees its slot: {:?}",
            second.err()
        );
        // Unknown agent types are refused, naming what is available.
        let err = sup
            .spawn("planner", "x", Some(ContextMode::Fresh), None, "c2")
            .expect_err("an unknown type is refused");
        assert!(err.contains("unknown agent type \"planner\""), "{err}");
        assert!(err.contains("general"), "{err}");
    }

    /// A `.md` type (spec §5.5) configures the child: its body is the
    /// system prompt, its model the child's model, its tools a subset of
    /// the child's default set.
    #[tokio::test]
    async fn a_md_type_configures_the_child_prompt_model_and_tools() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join(".tau").join("agents")).unwrap();
        std::fs::write(
            project.join(".tau/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: reviews\ntools: read\nmodel: review-model\n---\nYou are a strict reviewer.\n",
        )
        .unwrap();
        let types = crate::agent_type::discover(None, &project);
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![vec![sse("done", &[])]],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents::default(),
            depth: 0,
            types,
            bridge: bridge as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        });
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.create().unwrap();
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: crate::provider::canned(sse("ok", &[]).as_str()),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        sup.attach_parent(parent);
        let spawned = sup
            .spawn("reviewer", "look", None, None, "c0")
            .expect("the discovered type spawns");
        // The type's body, not the session's prompt; the type's model, not
        // the session's; the subset, not the full child set.
        assert_eq!(spawned.agent.system_prompt(), "You are a strict reviewer.");
        assert_eq!(spawned.model, "review-model");
        assert_eq!(spawned.agent.tools().len(), 1);
        assert_eq!(spawned.agent.tools()[0].name, "read");
        // An omitted context_mode falls back to the type's default (fresh
        let children = sup.children.lock().unwrap();
        let child = children.get(&spawned.handle).unwrap();
        assert_eq!(child.context_mode, ContextMode::Fresh);
    }

    /// A `general` child inherits the session's prompt (spec §5.5) —
    /// including the skill catalog that `build_live` appended as the last
    /// layer (ticket #28).
    #[tokio::test]
    async fn a_general_child_inherits_the_catalog_carrying_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let skill = crate::skills::Skill {
            name: "alpha".into(),
            description: "does alpha".into(),
            location: dir.path().join(".agents/skills/alpha/SKILL.md"),
            model_invocation: true,
        };
        let catalog = crate::skills::catalog(&[skill]).unwrap();
        let prompt = format!("You are Tau, a coding agent.\n\n{catalog}");
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![vec![sse("done", &[])]],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: prompt.clone(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents::default(),
            depth: 0,
            types: crate::agent_type::discover(None, dir.path()),
            bridge: bridge as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        });
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.create().unwrap();
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: prompt.clone(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: crate::provider::canned(sse("ok", &[]).as_str()),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        sup.attach_parent(parent);
        let spawned = sup
            .spawn("general", "look", None, None, "c0")
            .expect("general always spawns");
        // The session's prompt verbatim — the catalog rides along with it.
        assert_eq!(spawned.agent.system_prompt(), prompt);
    }
    /// `max_depth` is enforced at spawn: a session at the cap refuses to
    /// spawn, and `max_depth: 0` disables spawning outright (the
    /// structural child-carries-no-supervisor rule already bounds v0 depth
    /// to 1, so depth 0 is the only reachable check in a live tree).
    #[tokio::test]
    async fn the_depth_cap() {
        let dir = tempfile::tempdir().unwrap();
        let sup = |depth: u32, max_depth: u32| -> Arc<Supervisor> {
            let bridge = Arc::new(TestBridge::default());
            let factory = Arc::new(CannedFactory {
                scripts: vec![],
                delays: vec![],
                created: AtomicUsize::new(0),
                calls: Arc::new(AtomicUsize::new(0)),
            });
            Supervisor::new(SupervisorParams {
                parent_session: "parent".into(),
                cwd: dir.path().to_path_buf(),
                provider: factory,
                model: "test-model".into(),
                system_prompt: "be terse".into(),
                om: Om::default(),
                om_model: String::new(),
                tool_batch_on_force: ToolBatchPolicy::Complete,
                turn: TurnConfig::default(),
                caps: SubAgents {
                    max_depth,
                    max_concurrent: None,
                },
                types: vec![crate::agent_type::builtin_general()],
                depth,
                bridge: bridge as Arc<dyn SubagentBridge>,
                driver: Arc::new(TestDriver),
            })
        };
        // At the cap: refused with a clear diagnostic.
        let at_cap = sup(1, 1);
        let err = at_cap
            .spawn("general", "x", Some(ContextMode::Fresh), None, "c0")
            .expect_err("a session at the depth cap cannot spawn");
        assert!(err.contains("depth cap"), "{err}");
        // max_depth 0 disables spawning for a top-level session.
        let zero = sup(0, 0);
        let err = zero
            .spawn("general", "x", Some(ContextMode::Fresh), None, "c0")
            .expect_err("max_depth 0 disables spawning");
        assert!(err.contains("depth cap"), "{err}");
        // Below the cap the depth check passes (spawn then fails only on
        // the missing parent attachment, not on depth).
        let below = sup(0, 1);
        let err = below
            .spawn("general", "x", Some(ContextMode::Fresh), None, "c0")
            .expect_err("no parent attached in this bare harness");
        assert!(!err.contains("depth cap"), "{err}");
    }

    /// The drive-generation guard (review N1): stop + resume landing in the
    /// window after the in-flight `drive()` returns must not let the old
    /// drive consume the child's nudge budget and run rounds alongside the
    /// fresh drive. A notify-gated driver makes the window deterministic:
    /// both drives are in flight when the test releases them at once — the
    /// fresh drive legitimately consumes the nudge budget (its own turn);
    /// without the guard the superseded drive would race it for the budget
    /// and its loser marks the child `failed` (nudge exhausted).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stop_resume_in_the_drive_window_does_not_double_drive() {
        /// Each `drive()` blocks until the test's notify fires.
        struct NotifyingDriver {
            notify: Arc<tokio::sync::Notify>,
            calls: AtomicUsize,
        }
        impl ChildDriver for NotifyingDriver {
            fn drive(&self, _session: &str, _agent: &Arc<AgentSession>) -> BoxedDrive {
                let notify = Arc::clone(&self.notify);
                self.calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    notify.notified().await;
                    Ok(())
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let driver = Arc::new(NotifyingDriver {
            notify: Arc::new(tokio::sync::Notify::new()),
            calls: AtomicUsize::new(0),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory as Arc<dyn ChildProviderFactory>,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents::default(),
            depth: 0,
            types: vec![crate::agent_type::builtin_general()],
            bridge: bridge as Arc<dyn SubagentBridge>,
            driver: Arc::clone(&driver) as Arc<dyn ChildDriver>,
        });
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.create().unwrap();
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: Arc::new(ScriptedProvider::new(vec![sse("", &[])])),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        sup.attach_parent(parent);

        // Drive #1 is in flight; stop + resume then spawn drive #2, which
        // is in flight too — the window opens when the notify releases
        // both rounds at once.
        let spawned = sup
            .spawn("general", "work", Some(ContextMode::Fresh), None, "c0")
            .unwrap();
        wait_for(|| driver.calls.load(Ordering::SeqCst) == 1);
        sup.stop(&spawned.handle, StoppedBy::User).unwrap();
        sup.message(&spawned.handle, Some("go".into()), Lane::Steering)
            .unwrap();
        wait_for(|| driver.calls.load(Ordering::SeqCst) == 2);
        driver.notify.notify_waiters();
        // The superseded drive ends here, on the generation mismatch.
        spawned.drive.await.unwrap();
        assert!(
            !matches!(
                sup.state_info(&spawned.handle).unwrap().state,
                ChildState::Failed { .. }
            ),
            "the superseded drive must not exhaust the nudge budget"
        );
        let mut store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
        store.open().unwrap();
        let entries = store.entries_range(0, usize::MAX).unwrap();
        let nudges = entries
            .iter()
            .filter(|e| {
                e.kind == "system"
                    && e.payload
                        .get("note")
                        .and_then(|n| n.as_str())
                        .map(|n| n.starts_with("Nudge"))
                        .unwrap_or(false)
            })
            .count();
        assert_eq!(
            nudges, 1,
            "exactly the fresh drive's nudge; the superseded drive added none"
        );
        // Cleanup: the fresh drive's next round is gated; stop the child.
        sup.stop(&spawned.handle, StoppedBy::User).unwrap();
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
        let s = sup
            .spawn(
                "general",
                "compact me",
                Some(ContextMode::Compacted),
                None,
                "c0",
            )
            .unwrap();
        // Quiesce the child before reading its file from a second store:
        // its drive ends when the nudge exhausts, after which the file is
        // stable (review B1: a reader racing the drive saw a torn branch).
        s.drive.await.unwrap();
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
        assert!(
            snapshot.payload["log"]
                .as_str()
                .unwrap()
                .contains("the parent's log")
        );
        // The child's own record: the prefix verbatim, empty suffix.
        let record = OmState::load_record(&mut store).unwrap();
        assert_eq!(
            record.frozen_prefix,
            "<observations>the parent's log</observations>"
        );
        assert!(record.active_observations.is_empty());
    }

    /// Parent-scoped recall (review N4): a compacted child's
    /// `scope: "parent"` browses the parent session's raw history — the
    /// target of its frozen prefix; a session without a parent link gets a
    /// refusal, not a guess.
    #[tokio::test]
    async fn a_compacted_child_recalls_the_parent_session() {
        let dir = tempfile::tempdir().unwrap();
        let (sup, _) = harness(
            dir.path(),
            vec![sse("obs turn", &[])],
            vec![vec![sse("", &[])]],
            SubAgents::default(),
        );
        let parent = sup.parent.lock().unwrap().clone().unwrap();
        // The parent's raw history, plus an observation group covering it.
        parent
            .append_entry(
                crate::agent::KIND_USER,
                json!({ "text": "parent raw one", "lane": "follow-up" }),
            )
            .unwrap();
        parent
            .append_entry(
                crate::agent::KIND_USER,
                json!({ "text": "parent raw two", "lane": "follow-up" }),
            )
            .unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.open().unwrap();
        let entries = store.entries_range(0, usize::MAX).unwrap();
        let first = entries[0].id.as_str();
        let last = entries.last().unwrap().id.as_str();
        let record = OmRecord {
            frozen_prefix: String::new(),
            active_observations: crate::om::wrap_in_observation_group(
                "parent obs",
                &format!("{first}:{last}"),
                "pg",
                None,
            ),
            ..Default::default()
        };
        // Saved to the file: the parent-scoped recall reads the record the
        // way it always does (a file read, not the parent's memory).
        OmState::from_config(&Om::default(), record.clone())
            .save(&mut store)
            .unwrap();
        parent.set_om(Some(OmState::from_config(&Om::default(), record)));
        let s = sup
            .spawn(
                "general",
                "compact me",
                Some(ContextMode::Compacted),
                None,
                "c0",
            )
            .unwrap();
        s.drive.await.unwrap();
        let child = sup.child_agent(&s.handle).unwrap();
        // Parent scope: the parent's raw entries come back.
        let out = child.recall_scoped(&json!({ "group": "pg", "scope": "parent" }));
        assert!(out.contains("parent raw one"), "{out}");
        assert!(out.contains("parent raw two"), "{out}");
        // The parent itself has no parent link.
        let out = parent.recall_scoped(&json!({ "group": "pg", "scope": "parent" }));
        assert!(out.contains("needs a parent link"), "{out}");
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
        let s = sup
            .spawn("general", "fork me", Some(ContextMode::Fork), None, "c0")
            .unwrap();
        // Quiesce the child before reading its file from a second store
        // (review B1, same as the compacted test).
        s.drive.await.unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), &s.session_id);
        store.open().unwrap();
        let child_entries = store.entries_range(0, usize::MAX).unwrap();
        // The parent's user entry is present with its stable id.
        let mut pstore = SessionStore::for_workspace(dir.path(), "parent");
        pstore.open().unwrap();
        let parent_entries = pstore.entries_range(0, usize::MAX).unwrap();
        let parent_user = parent_entries
            .iter()
            .find(|e| e.kind == "user")
            .expect("the parent has a user entry");
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
        let (sup, _) = harness(
            dir.path(),
            vec![sse("x", &[])],
            vec![vec![sse("", &[])]],
            SubAgents::default(),
        );
        // A child session file + link: notify validation is checked before
        // any state change.
        let mut store = SessionStore::for_workspace(dir.path(), "child");
        store.create().unwrap();
        let link = Arc::new(ChildLink {
            supervisor: sup,
            handle: "child-1".into(),
        });
        assert!(
            link.notify(&json!({ "text": "t", "output": json!({}) }))
                .contains("output requires done:true")
        );
        assert!(
            link.notify(&json!({ "text": "t", "done": true }))
                .contains("done:true requires an object output")
        );
        assert!(
            link.notify(&json!({ "text": "t", "done": true, "output": json!([1]) }))
                .contains("object output")
        );
        assert!(
            link.notify(&json!({ "text": "t", "waiting_on": "the void" }))
                .contains("waiting_on must be")
        );
    }

    /// The acceptance flow (ticket #24): the parent creates a task and
    /// assigns it to a compacted child; the child works it — evidence,
    /// then a gated finish — and ends via parent_notify; the child's
    /// session is the live record (done, with the evidence) and the
    /// parent's copy is the status pointer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_assigned_task_is_worked_by_the_child_and_resolves_through_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.create().unwrap();
        crate::task::create(
            &mut store,
            "task-1",
            "write the docs",
            vec![],
            vec![crate::task::Criterion {
                text: "docs exist".into(),
                status: crate::task::CriterionStatus::Pending,
            }],
        )
        .unwrap();
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![vec![sse(
                "",
                &[
                    (
                        "task_evidence".into(),
                        "e1".into(),
                        r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                    ),
                    (
                        "task_finish".into(),
                        "f1".into(),
                        r#"{"task":"task-1"}"#.into(),
                    ),
                    (
                        "parent_notify".into(),
                        "n1".into(),
                        r#"{"text":"finished","done":true,"output":{"result":"the docs"}}"#.into(),
                    ),
                ],
            )]],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents::default(),
            depth: 0,
            types: vec![crate::agent_type::builtin_general()],
            bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        });
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::agent_tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: crate::provider::canned(sse("", &[]).as_str()),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        sup.attach_parent(parent.clone());
        let spawned = sup
            .spawn(
                "general",
                "work task-1",
                Some(ContextMode::Compacted),
                Some("task-1"),
                "c0",
            )
            .expect("the compacted spawn");
        // The spawn with a task IS the assignment (spec §5.3): the record
        // is in the child's session before its first turn — no second
        // store, no timing cushion (review N3/B3). The child's session is
        // the live record; the parent's copy is the status pointer.
        wait_for(|| {
            matches!(
                sup.state_info(&spawned.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        // The child's session is the live record: done, with the evidence.
        let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
        child_store.open().unwrap();
        let child_tasks =
            crate::task::fold_entries(&child_store.entries_range(0, usize::MAX).unwrap());
        let t = &child_tasks[0];
        assert_eq!(t.id, "task-1");
        assert_eq!(t.status, crate::task::STATUS_DONE);
        assert_eq!(
            t.criteria[0].status,
            crate::task::CriterionStatus::Satisfied
        );
        // The parent's copy is the status pointer to the child.
        let mut parent_store = SessionStore::for_workspace(dir.path(), "parent");
        parent_store.open().unwrap();
        let parent_tasks =
            crate::task::fold_entries(&parent_store.entries_range(0, usize::MAX).unwrap());
        let p = &parent_tasks[0];
        assert_eq!(p.worker.as_ref().unwrap().session, spawned.session_id);
        // The pointer reflects the worker's final state (review B2): the
        // done resolution mirrored through the gate, not just the link.
        assert_eq!(p.worker.as_ref().unwrap().status, crate::task::STATUS_DONE);
        // The parent was woken by the notify.
        let wakes = bridge.wakes.lock().unwrap();
        assert!(
            wakes.iter().any(|w| w.child == spawned.session_id),
            "the parent was woken by the child's notify"
        );
    }

    /// A child is a leaf (spec §5.3): task_create/assign/cancel are not in
    /// its tool set, and a model that calls one anyway gets the guard's
    /// refusal — no phantom record lands in its session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_child_cannot_create_assign_or_cancel_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.create().unwrap();
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![vec![
                sse(
                    "",
                    &[(
                        "task_create".into(),
                        "c1".into(),
                        r#"{"title":"phantom"}"#.into(),
                    )],
                ),
                sse(
                    "",
                    &[(
                        "parent_notify".into(),
                        "n1".into(),
                        r#"{"text":"done","done":true,"output":{"result":"ok"}}"#.into(),
                    )],
                ),
            ]],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents::default(),
            depth: 0,
            types: vec![crate::agent_type::builtin_general()],
            bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        });
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::agent_tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: crate::provider::canned(sse("", &[]).as_str()),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        sup.attach_parent(parent.clone());
        let spawned = sup
            .spawn("general", "go", None, None, "c0")
            .expect("the spawn");
        wait_for(|| {
            matches!(
                sup.state_info(&spawned.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        // The refusal is the model's only view: the tool result says so,
        // and no task record of the child's own exists in its session.
        let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
        child_store.open().unwrap();
        let entries = child_store.entries_range(0, usize::MAX).unwrap();
        let refusal = entries
            .iter()
            .filter(|e| e.kind == crate::agent::KIND_TOOL)
            .filter_map(|e| e.payload.get("output"))
            .filter_map(Value::as_str)
            .find(|o| o.contains("not available in a child session"));
        assert!(
            refusal.is_some(),
            "the child's task_create was refused by the guard"
        );
        let tasks = crate::task::fold_entries(&entries);
        assert!(
            tasks.iter().all(|t| t.created_in.is_some()),
            "a child cannot create a task of its own: {tasks:?}"
        );
    }

    /// An assign is delivery, not just a copy (the spawn-race fix): the
    /// child's message path carries "Assigned {id}: {title}". The child
    /// parks after its first turn, so the assign deterministically takes
    /// the resume branch: the record copy precedes the resuming message,
    /// and the child's evidence lands on the parent's record.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_assign_to_a_parked_child_resumes_it_with_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SessionStore::for_workspace(dir.path(), "parent");
        store.create().unwrap();
        crate::task::create(
            &mut store,
            "task-1",
            "write the docs",
            vec![],
            vec![crate::task::Criterion {
                text: "docs exist".into(),
                status: crate::task::CriterionStatus::Pending,
            }],
        )
        .unwrap();
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![vec![
                // Turn 1: park — the child rests until the assign, so
                // the copy can land neither before nor after its work.
                sse(
                    "kicking off",
                    &[(
                        "parent_notify".into(),
                        "n0".into(),
                        r#"{"text":"kicking off","done":false,"waiting_on":"parent"}"#.into(),
                    )],
                ),
                // The resume turn: the record is already in the child's
                // session — the evidence lands on the parent's task.
                sse(
                    "",
                    &[(
                        "task_evidence".into(),
                        "e1".into(),
                        r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                    )],
                ),
                sse(
                    "",
                    &[(
                        "parent_notify".into(),
                        "n1".into(),
                        r#"{"text":"finished","done":true,"output":{"result":"the docs"}}"#.into(),
                    )],
                ),
            ]],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents::default(),
            depth: 0,
            types: vec![crate::agent_type::builtin_general()],
            bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        });
        let parent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::agent_tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: crate::provider::canned(sse("", &[]).as_str()),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        sup.attach_parent(parent.clone());
        // A spawn without a task, then the assign: the child parks after
        // its first turn and rests until the assign moves it — the only
        // ordering that can't race.
        let spawned = sup
            .spawn("general", "working", None, None, "c0")
            .expect("the spawn");
        wait_for(|| {
            matches!(
                sup.state_info(&spawned.handle).unwrap().state,
                ChildState::Idle { .. }
            )
        });
        sup.assign_task("task-1", &spawned.session_id)
            .expect("the assign");
        wait_for(|| {
            matches!(
                sup.state_info(&spawned.handle).unwrap().state,
                ChildState::Done { .. }
            )
        });
        // The delivery reached the child as a message, and the record it
        // worked is the parent's task (no phantom of its own).
        let mut child_store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
        child_store.open().unwrap();
        let entries = child_store.entries_range(0, usize::MAX).unwrap();
        assert!(
            entries.iter().any(|e| e.kind == crate::agent::KIND_USER
                && e.payload
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.contains("Assigned task-1: write the docs"))),
            "the assignment was delivered to the child as a message"
        );
        let tasks = crate::task::fold_entries(&entries);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "task-1");
        assert_eq!(tasks[0].status, crate::task::STATUS_DONE);
    }
}
