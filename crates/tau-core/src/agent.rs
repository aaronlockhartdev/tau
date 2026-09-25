//! The agent loop and the message lanes (spec §2, §7): one turn = system
//! prompt + history → provider call → tool dispatch → repeat until the model
//! ends the turn. Lanes: **force** (kill the in-flight stream, partial kept
//! as an `interrupted` entry, tool batch completed per config, message at
//! the head of the queue), **steering** (delivered at the next LLM call),
//! **follow-up** (delivered after the turn completes).

use crate::config::ToolBatchPolicy;
use crate::provider::{
    CallOutput, FunctionCall, FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage,
    ReasoningEffort, ResponseRequest, ToolSpec, TurnProviderRef, TurnResult, TurnSink,
};
use crate::session::{Entry, SessionStore};
use crate::tools;
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A message lane (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Force,
    Steering,
    FollowUp,
}

/// The session entry kinds the loop defines on top of the storage (spec §3).
pub const KIND_USER: &str = "user";
pub const KIND_ASSISTANT: &str = "assistant";
pub const KIND_TOOL: &str = "tool";
pub const KIND_SYSTEM: &str = "system";

/// Runaway guard: a model that never stops calling tools.
const MAX_ROUNDS: usize = 32;

/// A loop-level failure: session storage or the provider.
#[derive(Debug)]
pub enum AgentError {
    Session(crate::session::Error),
    Provider(crate::provider::ProviderError),
    Om(crate::om_integration::OmError),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(e) => write!(f, "session: {e}"),
            Self::Provider(e) => write!(f, "provider: {e}"),
            Self::Om(e) => write!(f, "om: {e}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<crate::session::Error> for AgentError {
    fn from(e: crate::session::Error) -> Self {
        Self::Session(e)
    }
}

impl From<crate::provider::ProviderError> for AgentError {
    fn from(e: crate::provider::ProviderError) -> Self {
        Self::Provider(e)
    }
}

/// Per-turn provider options, resolved from the merged config and the
/// session's model facts at session build (spec §12, #35); re-resolved when
/// the session's model changes.
#[derive(Debug, Clone, Default)]
pub struct TurnConfig {
    pub max_output_tokens: Option<u64>,
    pub reasoning: Option<ReasoningEffort>,
    pub reasoning_summary: bool,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    /// A stable cache key (the session id) plus the `cache.retention`.
    pub prompt_cache: Option<(String, crate::config::CacheRetention)>,
    /// The model's declared context window (its facts); the clamp's ceiling.
    pub context_window: Option<u32>,
}

#[derive(Clone)]
struct Queued {
    text: String,
    lane: Lane,
    /// Provenance: the child session that produced this wake message
    /// (ticket #23) — a child notification is a user entry with a `source`.
    source: Option<String>,
    /// A skill invocation (ticket #28): the entry's payload carries
    /// `(name, location)` so the GUI renders its green block.
    skill: Option<(String, String)>,
}

fn lane_name(lane: Lane) -> &'static str {
    match lane {
        Lane::Force => "force",
        Lane::Steering => "steering",
        Lane::FollowUp => "follow-up",
    }
}

/// The OM run's status callback: the activity kind as a string
/// ("observing" / "reflecting" / "idle") — the app shapes it into the
/// protocol event.
type OmStatusHook = Arc<dyn Fn(&str) + Send + Sync>;

struct Inner {
    store: SessionStore,
    system_prompt: String,
    model: String,
    tools: Vec<ToolSpec>,
    cwd: PathBuf,
    tool_batch_on_force: ToolBatchPolicy,
    turn: TurnConfig,
    queue: VecDeque<Queued>,
    om: Option<crate::om_integration::OmState>,
    om_model: String,
    /// The OM-run observer (the app emits the protocol's om_status event
    /// from it): core is transport-free, so the hook takes the kind string,
    /// not the event. `None` = no observer (tests, children).
    om_status_hook: Option<OmStatusHook>,
    /// The parent-side supervisor (ticket #23): present on non-child
    /// sessions only — the depth cap (a child cannot spawn) is structural.
    subagents: Option<Arc<crate::subagent::Supervisor>>,
    /// The child-side link (ticket #23): present on child sessions only;
    /// routes `parent_notify` to the child's supervisor.
    child: Option<Arc<crate::subagent::ChildLink>>,
}

/// One session's agent loop. Single-writer per session (spec §2): the GUI
/// (or a parent agent) drives it with `send` + `process`; the kill flag is
/// the only cross-thread surface (a force from the GUI's thread).
/// Everything an agent session needs to run, grouped so the constructor
/// stays a single parameter as the ticket series grows the loop (spec §5).
pub struct SessionParams {
    pub store: SessionStore,
    /// The fully assembled system prompt: the caller builds it (agent-type
    /// prompt + context files via `context::assemble`) — the loop takes the
    /// result and performs no discovery of its own.
    pub system_prompt: String,
    pub model: String,
    pub tools: Vec<ToolSpec>,
    pub cwd: PathBuf,
    pub provider: TurnProviderRef,
    pub tool_batch_on_force: ToolBatchPolicy,
    pub turn: TurnConfig,
    /// The OM integration state (ticket #22); `None` = OM disabled and the
    /// loop behaves as before.
    pub om: Option<crate::om_integration::OmState>,
    /// The OM model (global config, spec §4); empty = the session's model.
    pub om_model: String,
    /// The parent-side sub-agent supervisor (ticket #23); `None` for child
    /// sessions (a child cannot spawn — the depth cap is structural).
    pub subagents: Option<Arc<crate::subagent::Supervisor>>,
    /// The child-side link (ticket #23); `None` for non-child sessions.
    pub child: Option<Arc<crate::subagent::ChildLink>>,
}

pub struct AgentSession {
    inner: Mutex<Inner>,
    provider: TurnProviderRef,
    kill: Arc<AtomicBool>,
    /// Persistent stop (ticket #23): unlike the per-call kill flag (reset
    /// at each call), it stays set until the next send — a sub-agent soft
    /// stop cuts the stream this way.
    stop: Arc<AtomicBool>,
}

impl AgentSession {
    pub fn new(p: SessionParams) -> Self {
        Self {
            inner: Mutex::new(Inner {
                store: p.store,
                system_prompt: p.system_prompt,
                model: p.model,
                tools: p.tools,
                cwd: p.cwd,
                tool_batch_on_force: p.tool_batch_on_force,
                turn: p.turn,
                queue: VecDeque::new(),
                om: p.om,
                om_model: p.om_model,
                om_status_hook: None,
                subagents: p.subagents,
                child: p.child,
            }),
            provider: p.provider,
            kill: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Queue a user message on its lane (spec §7). A force kills the
    /// in-flight stream and jumps to the head of the queue; everything else
    /// keeps its position.
    pub fn send(&self, text: impl Into<String>, lane: Lane) {
        let mut inner = self.inner.lock().unwrap();
        // A new send clears a previous stop (ticket #23): a stop means
        // "interrupt current work", not "never work again".
        self.stop.store(false, Ordering::SeqCst);
        let msg = Queued {
            text: text.into(),
            lane,
            source: None,
            skill: None,
        };
        if lane == Lane::Force {
            self.kill.store(true, Ordering::SeqCst);
            inner.queue.push_front(msg);
        } else {
            inner.queue.push_back(msg);
        }
    }

    /// A `/skill:` invocation expanded at the app's `message_send` boundary
    /// (ticket #28): `text` is already the expansion template; the payload
    /// carries the skill's identity for the GUI's block.
    pub fn send_skill(&self, text: impl Into<String>, lane: Lane, name: &str, location: &str) {
        let mut inner = self.inner.lock().unwrap();
        self.stop.store(false, Ordering::SeqCst);
        let msg = Queued {
            text: text.into(),
            lane,
            source: None,
            skill: Some((name.to_owned(), location.to_owned())),
        };
        if lane == Lane::Force {
            self.kill.store(true, Ordering::SeqCst);
            inner.queue.push_front(msg);
        } else {
            inner.queue.push_back(msg);
        }
    }

    /// A wake message with provenance (ticket #23): the child's result
    /// lands on the follow-up lane tagged with the child's session id, so
    /// the notification entry carries child provenance (ADR-0001).
    pub fn send_notified(&self, text: impl Into<String>, source: String) {
        let mut inner = self.inner.lock().unwrap();
        self.stop.store(false, Ordering::SeqCst);
        inner.queue.push_back(Queued {
            text: text.into(),
            lane: Lane::FollowUp,
            source: Some(source),
            skill: None,
        });
    }

    /// A persistent stop (ticket #23): cuts the in-flight stream at the
    /// next delta and stays until the next send.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// The persistent stop flag (a forwarding seam can share it so one
    /// flag cuts at both the transport mirror and the loop sink).
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Whether a queued message is waiting (a parked sub-agent with a
    /// queued message is resumed by it, ADR-0001).
    pub fn has_pending(&self) -> bool {
        !self.inner.lock().unwrap().queue.is_empty()
    }

    /// The queued message at the head of the lane queue — for a
    /// turn-starting send, the one the in-flight turn will deliver first.
    /// The harness re-queues it in the GUI's queue when the turn cannot
    /// even start (a failed store open), so the accepted send is not lost.
    pub fn first_pending(&self) -> Option<(String, Lane)> {
        self.inner
            .lock()
            .unwrap()
            .queue
            .front()
            .map(|q| (q.text.clone(), q.lane))
    }

    /// The child-side link (a child session's `parent_notify` routing).
    pub fn child_link(&self) -> Option<Arc<crate::subagent::ChildLink>> {
        self.inner.lock().unwrap().child.clone()
    }

    /// This session's supervisor (None for a child session — children
    /// cannot spawn, so the supervisor is parent-side only).
    pub fn subagents(&self) -> Option<Arc<crate::subagent::Supervisor>> {
        self.inner.lock().unwrap().subagents.clone()
    }

    /// The `recall` tool (spec §4): a child's `scope: "parent"` browses
    /// the parent session's raw history (a compacted child's frozen prefix
    /// points there); everything else is this session's own log.
    pub fn recall_scoped(&self, args: &Value) -> String {
        if args.get("scope").and_then(Value::as_str) == Some("parent") {
            let (link, cwd) = {
                let inner = self.inner.lock().unwrap();
                (inner.child.clone(), inner.cwd.clone())
            };
            match link {
                Some(link) => {
                    let parent_id = link
                        .handle()
                        .rsplit_once('-')
                        .map(|(s, _)| s.to_owned())
                        .unwrap_or_default();
                    let mut store = SessionStore::for_workspace(&cwd, &parent_id);
                    match store.open() {
                        Ok(()) => match crate::om_integration::OmState::load_record(&mut store) {
                            Ok(record) => crate::om_integration::recall(&mut store, &record, args),
                            Err(e) => format!("recall: parent record: {e}"),
                        },
                        Err(e) => format!("recall: parent session: {e}"),
                    }
                }
                None => "recall: scope \"parent\" needs a parent link (compacted child)".into(),
            }
        } else {
            let mut inner = self.inner.lock().unwrap();
            let record = inner
                .om
                .as_ref()
                .map(|om| om.record.clone())
                .unwrap_or_default();
            crate::om_integration::recall(&mut inner.store, &record, args)
        }
    }

    /// The task tools (spec §5.3/§5.4): run against this session's own
    /// store — the session-scoped task store a parent and a child share.
    fn task_tool(&self, tc: &tools::ToolCall) -> String {
        let mut inner = self.inner.lock().unwrap();
        if let Some(r) = Self::task_child_refusal(inner.child.is_some(), &tc.name) {
            return r;
        }
        crate::task::tool_call(&mut inner.store, &tc.name, &tc.args)
    }
    /// The session store's header timestamp (epoch ms).
    pub fn store_created(&self) -> u64 {
        self.inner.lock().unwrap().store.created()
    }

    /// Run one operation against this session's own store — the
    /// single-writer surface for other components (the sub-agent task
    /// gate, the app's task commands; review B3).
    pub fn with_task_store<R>(&self, f: impl FnOnce(&mut SessionStore) -> R) -> R {
        let mut inner = self.inner.lock().unwrap();
        f(&mut inner.store)
    }

    /// The seven task tools against this session's own store (the app's
    /// task commands; the model's dispatch uses `task_tool`).
    pub fn task_tool_call(&self, name: &str, args: &Value) -> String {
        let mut inner = self.inner.lock().unwrap();
        if let Some(r) = Self::task_child_refusal(inner.child.is_some(), name) {
            return r;
        }
        crate::task::tool_call(&mut inner.store, name, args)
    }

    /// A child is a leaf: create/assign/cancel act on a parent's planning
    /// state, never on the assigned record (spec §5.3); the refusal is the
    /// caller's only view. Shared by the model's dispatch and the app's
    /// task-command surface — the same rule on both paths.
    fn task_child_refusal(is_child: bool, name: &str) -> Option<String> {
        if is_child && matches!(name, "task_create" | "task_assign" | "task_cancel") {
            Some(format!(
                "{name}: not available in a child session — work the task your parent assigned"
            ))
        } else {
            None
        }
    }

    #[cfg(test)]
    /// Test-only OM state injection (the sub-agent module's tests).
    pub fn set_om(&self, om: Option<crate::om_integration::OmState>) {
        self.inner.lock().unwrap().om = om;
    }

    /// The OM-run observer (the app's om_status emitter); `None` clears it.
    pub fn set_om_status_hook(&self, hook: Option<OmStatusHook>) {
        self.inner.lock().unwrap().om_status_hook = hook;
    }

    /// The session's model for the next turn's calls (the `session_set_model`
    /// command's live half; provider resolution happens at the call).
    pub fn set_model(&self, model: String) {
        self.inner.lock().unwrap().model = model;
    }

    /// The turn's provider options (spec §12, #35): re-derived when the
    /// session's model changes, so clamping and reasoning levels track the
    /// active model.
    pub fn set_turn_config(&self, turn: TurnConfig) {
        self.inner.lock().unwrap().turn = turn;
    }

    /// Append a record entry against the active leaf (the sub-agent
    /// lifecycle records, ticket #23).
    pub fn append_entry(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        self.append(kind, payload)
    }

    pub fn model(&self) -> String {
        self.inner.lock().unwrap().model.clone()
    }

    pub fn tools(&self) -> Vec<ToolSpec> {
        self.inner.lock().unwrap().tools.clone()
    }

    pub fn system_prompt(&self) -> String {
        self.inner.lock().unwrap().system_prompt.clone()
    }

    /// The session's OM state (ticket #22); `None` = OM disabled.
    pub fn om_state(&self) -> Option<crate::om_integration::OmState> {
        self.inner.lock().unwrap().om.clone()
    }
}
mod turn;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_om;
#[cfg(test)]
mod tests_turns;
