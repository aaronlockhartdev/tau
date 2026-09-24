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
use tau_protocol::payload::{ResumeContract, Task};

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
    pub resume_contract: Option<ResumeContract>,
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
    /// The child's displayed name (its session header title): the name
    /// the agent and the GUI address it by; the session id stays the
    /// machine key.
    pub name: String,
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

    fn set_state(&self, state: ChildState) -> Result<(), String> {
        *self.state.lock().unwrap() = state.clone();
        // The record carries the discriminant plus the non-output variant
        // fields only: done's output lives in the notify record (the
        // report), not twice.
        let payload = match &state {
            ChildState::Running => json!({ "event": "state", "state": "running" }),
            ChildState::Idle { waiting_on } => json!({
                "event": "state",
                "state": "idle",
                "waiting_on": waiting_on.as_str(),
            }),
            ChildState::Done { .. } => json!({ "event": "state", "state": "done" }),
            ChildState::Failed { reason } => json!({
                "event": "state",
                "state": "failed",
                "reason": reason,
            }),
            ChildState::Stopped { by } => json!({
                "event": "state",
                "state": "stopped",
                "by": by.as_str(),
            }),
        };
        self.agent
            .append_entry(KIND_SUBAGENT, payload)
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
    /// The child's displayed name (spawn addresses it by this; the
    /// session id is the machine key).
    pub name: String,
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

    /// Addressing (name-based, the session id stays the machine key):
    /// the child's name (its displayed title, exact match) first, then
    /// the session id, then the internal handle (the GUI's protocol
    /// commands pass it).
    fn resolve(&self, name_or_id: &str) -> Option<Arc<Child>> {
        let children = self.children.lock().unwrap();
        children
            .values()
            .find(|c| c.name == name_or_id)
            .or_else(|| children.values().find(|c| c.session_id == name_or_id))
            .or_else(|| children.get(name_or_id))
            .cloned()
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
    pub task: Option<Task>,
    pub resume_contract: Option<ResumeContract>,
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
                Ok(s) => {
                    // The spawn reports the child's name: the model refers
                    // to it by name in the sub-agent tools that follow
                    // (the session id stays the machine key, internal).
                    format!(
                        "spawned sub-agent {}; it reports back via parent_notify",
                        s.name
                    )
                }
                Err(e) => e,
            }
        }
        "subagent_message" => {
            let Some(name) = tc.args.get("name").and_then(Value::as_str) else {
                return "subagent_message: missing \"name\"".into();
            };
            let text = tc
                .args
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match sup.message(name, text, Lane::Steering) {
                Ok(s) => s,
                Err(e) => e,
            }
        }
        "subagent_stop" => {
            let Some(name) = tc.args.get("name").and_then(Value::as_str) else {
                return "subagent_stop: missing \"name\"".into();
            };
            match sup.stop(name, StoppedBy::Parent) {
                Ok(s) => s,
                Err(e) => e,
            }
        }
        "subagent_state" => {
            let Some(name) = tc.args.get("name").and_then(Value::as_str) else {
                return "subagent_state: missing \"name\"".into();
            };
            match sup.state_info(name) {
                Some(info) => {
                    let name = sup
                        .resolve(name)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| info.child.clone());
                    format!(
                        "{name}: {}{} — session {}",
                        info.state.kind(),
                        info.waiting_on
                            .map(|w| format!(" (waiting on {})", w.as_str()))
                            .unwrap_or_default(),
                        info.child
                    )
                }
                None => format!("subagent_state: no sub-agent {name} in this session"),
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
mod inspect;
mod lifecycle;
mod notify;
mod spawn;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_acceptance;
#[cfg(test)]
mod tests_caps;
#[cfg(test)]
mod tests_context_modes;
#[cfg(test)]
mod tests_notify;
#[cfg(test)]
mod tests_stop_resume;
#[cfg(test)]
mod tests_wake;
