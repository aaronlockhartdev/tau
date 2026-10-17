//! Sub-agent machinery (spec §5, ADR-0001): in-process concurrent
//! agent-loop instances, each working its own session file linked to the
//! parent. The 6-state lifecycle is machine-observed (spec §5.2): the
//! states are *read* from observed signals — a turn that ended with a
//! `QUESTION:` marker is `awaiting`, a complete plan (gate passed) is
//! `done`, `gate_failed` is `failed` — with `parent_notify` a secondary
//! hint the observation confirms or overrides. The one-shot nudge is
//! gone: an undeclared stop is `idle`. A child holds no lock on its
//! parent: every child→parent effect is a value the parent ingests.
//!
//! Every transition is a `subagent` entry in the child's session file —
//! the child's file is the record; the in-memory state is its live view.

use crate::agent::{AgentSession, TurnConfig};
use crate::config::{Gates, Om, SubAgents, ToolBatchPolicy};
use crate::provider::{TurnProviderRef, Usage};
use crate::session::SessionStore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tau_protocol::payload::{ResumeContract, Task};

pub use bridge::{
    BoxedDrive, ChildDriver, ChildProviderFactory, SpawnNotice, StateNotice, SubagentBridge,
    WakeKind, WakeNotice,
};
pub use child::Child;
pub use link::ChildLink;
pub use route::route_parent;
pub use states::{ChildState, ContextMode, KIND_SUBAGENT, StoppedBy, TurnOrigin, WaitingOn};
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
    /// cannot close over its owner). `Weak`: the supervisor is owned by the
    /// parent session, so a strong capture would be a cycle (ticket #43).
    parent: Mutex<Option<Weak<AgentSession>>>,
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
    gates: Option<Gates>,
    bridge: Arc<dyn SubagentBridge>,
    driver: Arc<dyn ChildDriver>,
    core: Option<Weak<crate::harness::Core>>,
}

/// The values a session built around this supervisor inherits (the
/// constructor's `Root`/`Child` roles): the parent's cwd, model, prompt,
/// turn config, and OM wiring.
pub(crate) struct Inherited {
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) model: String,
    pub(crate) system_prompt: String,
    pub(crate) turn: TurnConfig,
    pub(crate) om: Om,
    pub(crate) om_model: String,
    pub(crate) tool_batch_on_force: ToolBatchPolicy,
    pub(crate) gates: Option<Gates>,
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
    pub gates: Option<Gates>,
    pub caps: SubAgents,
    /// The session's depth (top-level = 0); a spawn fails when the child's
    /// depth (depth + 1) would exceed `max_depth`.
    pub depth: u32,
    /// The agent-type registry (spec §5.5): `general` first, then the
    /// discovered `.md` types.
    pub types: Vec<crate::agent_type::AgentType>,
    pub bridge: Arc<dyn SubagentBridge>,
    pub driver: Arc<dyn ChildDriver>,
    /// The core the parent session runs under (production): the plan-file
    /// wiring (seed, the prompt's path, the state re-injection) routes
    /// through it. `None` on the test seams (no plan wiring).
    pub core: Option<Weak<crate::harness::Core>>,
}

/// A completed spawn: everything the dispatch needs to register the child
/// as an ordinary live session (a child is an ordinary session — ADR-0006)
/// plus the drive's join handle.
#[allow(clippy::missing_fields_in_debug)] // identity-only debug; agent and provider are not Debug
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
    #[must_use]
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
            gates: p.gates,
            bridge: p.bridge,
            driver: p.driver,
            core: p.core,
        })
    }

    /// The inherited bundle (see `Inherited`).
    pub(crate) fn inherited(&self) -> Inherited {
        Inherited {
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            turn: self.turn.clone(),
            om: self.om.clone(),
            om_model: self.om_model.clone(),
            tool_batch_on_force: self.tool_batch_on_force,
            gates: self.gates.clone(),
        }
    }

    /// The core link (production); `None` on the test seams (no plan
    /// wiring).
    #[must_use]
    pub fn core(&self) -> Option<Weak<crate::harness::Core>> {
        self.core.clone()
    }

    /// The parent's session id (the supervisor's own — the child's
    /// pointer to it; the handle stays opaque, R3).
    pub fn parent_session(&self) -> &str {
        &self.parent_session
    }

    /// The parent's loop (attached once by the caller after the parent's
    /// `AgentSession` exists); stored as a `Weak` (ticket #43).
    pub fn attach_parent(&self, agent: &Arc<AgentSession>) {
        *self
            .parent
            .lock()
            .expect("supervisor parent: no panic while the lock is held") =
            Some(Arc::downgrade(agent));
    }

    /// Whether a child is still `Running` (the child-side loop's exit
    /// check, ADR-0001).
    pub fn child_running(&self, handle: &str) -> bool {
        self.children
            .lock()
            .expect("children map: no panic while the lock is held")
            .get(handle)
            .is_some_and(|c| matches!(c.state(), ChildState::Running))
    }

    /// A child's concurrency slot: every child with a live drive holds
    /// one; a quiescent child (done/stopped/failed — its drive is gone)
    /// holds none, and a resume of it re-acquires a slot.
    fn live_children(&self) -> Vec<Arc<Child>> {
        self.children
            .lock()
            .expect("children map: no panic while the lock is held")
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
            Some(max)
                if u32::try_from(self.live_children().len())
                    .expect("live-child counts are far below u32::MAX")
                    + u32::from(extra)
                    > max =>
            {
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
        let children = self
            .children
            .lock()
            .expect("children map: no panic while the lock is held");
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
            .expect("children map: no panic while the lock is held")
            .get(handle)
            .map(|c| c.agent.clone())
    }

    /// The child's session id for a handle (the children map is the
    /// record; the handle stays opaque, R3).
    pub fn child_session_id(&self, handle: &str) -> Option<String> {
        self.children
            .lock()
            .expect("children map: no panic while the lock is held")
            .get(handle)
            .map(|c| c.session_id.clone())
    }

    /// Every child's session id (the child-targeted `TaskChanged` routing).
    pub fn child_sessions(&self) -> Vec<String> {
        self.children
            .lock()
            .expect("children map: no panic while the lock is held")
            .values()
            .map(|c| c.session_id.clone())
            .collect()
    }

    /// The child's assigned task from the single source of truth (the
    /// parent's file — the task never leaves it; the child's file carries
    /// no task entries).
    pub(super) fn child_task(&self, child: &Child) -> Option<Task> {
        let mut store = SessionStore::for_workspace(&self.cwd, &self.parent_session);
        let Ok(()) = store.open() else {
            return None;
        };
        let entries = store.entries_range(0, usize::MAX).ok()?;
        crate::task::fold_entries(&entries).into_iter().find(|t| {
            t.worker
                .as_ref()
                .is_some_and(|w| w.session == child.session_id)
        })
    }
}

/// One child's structured state (the protocol carries its own mirror; the
/// state is the observed lifecycle, spec §5.2).
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

mod bridge;
mod child;
mod inspect;
mod lifecycle;
mod link;
mod notify;
mod route;
mod spawn;
mod states;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests_acceptance;
#[cfg(test)]
mod tests_caps;
#[cfg(test)]
mod tests_context_modes;
#[cfg(test)]
mod tests_gate;
#[cfg(test)]
mod tests_link;
#[cfg(test)]
mod tests_notify;
#[cfg(test)]
mod tests_stop_resume;
#[cfg(test)]
mod tests_wake;
