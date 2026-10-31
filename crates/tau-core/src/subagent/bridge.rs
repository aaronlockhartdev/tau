use super::states::{ChildState, ContextMode, WaitingOn};
use crate::agent::AgentSession;
use crate::provider::TurnProviderRef;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tau_protocol::payload::ResumeContract;

/// The provider a child's loop runs against, created per child session id
/// (the dispatch implements this to wrap the production provider in its
/// stream-forwarding seam; tests return canned providers). `model` is the
/// child's model id, so the factory can route the child to the provider that
/// id names (#336) instead of the one its parent happens to address.
pub trait ChildProviderFactory: Send + Sync {
    fn create(&self, child_session_id: &str, model: &str) -> TurnProviderRef;

    /// Where a child's memory passes run (#348): `om_model` is the inherited
    /// `[om].om_model` and `model` the child's own id, so a bare `om_model` that
    /// several providers declare resolves against the provider the child's id
    /// names (#336). `Session` — the default, and what the test seams run — has
    /// the child ride its own model.
    fn om_route(&self, _om_model: &str, _model: &str) -> crate::agent::OmRoute {
        crate::agent::OmRoute::Session
    }
}

/// The parent-side notice for a child state transition: the GUI badge and
/// snapshot refresh. Idempotent-cumulative — the full state of one handle.
#[derive(Clone, Debug, PartialEq)]
pub struct StateNotice {
    pub parent: String,
    pub handle: String,
    pub child: String,
    pub state: ChildState,
    pub note: Option<String>,
    /// A stopped child's resume contract (the plan's next action, spec
    /// §5.2); None for every other transition.
    pub resume_contract: Option<ResumeContract>,
}

/// A spawn notice (the protocol's `subagent_spawned` event).
#[derive(Clone, Debug, PartialEq)]
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

/// A child handle was reclaimed (spawn-triggered terminal reclamation,
/// #250): the implementation drops the matching `child_handles` entry.
/// The session file and the child's live session are untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct ReclaimNotice {
    pub parent: String,
    pub handle: String,
    pub child: String,
}

/// A child handle was re-adopted (its `resolve` miss fell back to the disk
/// record, #250): the implementation re-records the handle's mint (the
/// reclamation dropped it). The session file and the child's live session
/// are untouched, as on reclamation.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadoptNotice {
    pub parent: String,
    pub handle: String,
    pub child: String,
}

/// A wake (ADR-0001 wake rules): the implementation appends the
/// notification to the parent's active branch (child provenance) and starts
/// a turn there.
#[derive(Clone, Debug, PartialEq)]
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
    /// A child handle was reclaimed (its entry left the supervisor's
    /// children map, #250): the dispatch drops the matching `child_handles`
    /// entry. The session file and the child's live session are untouched.
    fn reclaimed(&self, notice: &ReclaimNotice);
    /// A child handle was re-adopted from disk, #250): the dispatch
    /// re-records the handle's mint in `child_handles` (the reclamation
    /// dropped it).
    fn re_adopted(&self, notice: &ReadoptNotice);
    fn wake(&self, notice: &WakeNotice);
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
