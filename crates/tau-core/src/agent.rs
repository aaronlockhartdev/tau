//! The agent loop and the message lanes (spec §2, §7): one turn = system
//! prompt + history → provider call → tool dispatch → repeat until the model
//! ends the turn. Lanes: **force** (kill the in-flight stream, partial kept
//! as an `interrupted` entry, tool batch completed per config, message at
//! the head of the queue), **steering** (delivered at the next LLM call),
//! **follow-up** (delivered after the turn completes).

use crate::config::ToolBatchPolicy;
use crate::provider::{
    CallOutput, FunctionCall, FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage,
    ReasoningEffort, ResponseRequest, ToolSpec, TurnEvent, TurnProviderRef, TurnResult, TurnSink,
};
use crate::session::{Entry, EntryEventHook, SessionStore};
use crate::tools;
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

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
    /// The `limits.image.max_bytes` cap on a `read` of an image file (#36);
    /// `None` (the default) is uncapped.
    pub image_max_bytes: Option<u64>,
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
    /// The entry was appended to the file at queue time (a steering report
    /// lands now, so the GUI shows it immediately); the turn loop's dequeue
    /// skips the second append.
    in_file: bool,
}

const fn lane_name(lane: Lane) -> &'static str {
    match lane {
        Lane::Force => "force",
        Lane::Steering => "steering",
        Lane::FollowUp => "follow-up",
    }
}

/// The OM run's status callback: the activity kind as a string
/// ("observing" / "reflecting" / "idle") — the app shapes it into the
/// protocol event.
pub(crate) type OmStatusHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Fired when the lane queue changes mid-turn (a steering/force message is
/// consumed): the app re-emits the queue snapshot so the GUI's queue pane
/// doesn't stay stale until the turn boundary.
pub(crate) type QueueEventHook = Arc<dyn Fn() + Send + Sync>;
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
    /// The OM-run observer (the app emits the protocol's `om_status` event
    /// from it): core is transport-free, so the hook takes the kind string,
    /// not the event. `None` = no observer (tests, children).
    om_status_hook: Option<OmStatusHook>,
    /// Wire-only entry upserts (ADR-0008): a re-emitted entry before its
    /// file line lands (the streaming assistant, the tool's call phase);
    /// the harness shapes it into the protocol's `EntryUpsert`.
    entry_upsert_hook: Option<EntryEventHook>,
    /// Fired when the lane queue changes mid-turn (steering/force consumed)
    /// so the app can re-emit the queue snapshot (the GUI's queue pane).
    queue_event_hook: Option<QueueEventHook>,
    /// The parent-side supervisor (ticket #23): present on non-child
    /// sessions only — the depth cap (a child cannot spawn) is structural.
    subagents: Option<Arc<crate::subagent::Supervisor>>,
    /// The child-side link (ticket #23): present on child sessions only;
    /// routes `parent_notify` to the child's supervisor.
    child: Option<Arc<crate::subagent::ChildLink>>,
    /// The parent's session (child sessions only, ticket: shared task
    /// model): the child's task tools route here, so the task never
    /// leaves the parent's file. `Weak` — the supervisor's `Arc` already
    /// links parent and child; this adds no new strong cycle.
    parent_task_store: Option<Weak<AgentSession>>,
}

/// One session's agent loop. Single-writer per session (spec §2): the GUI
/// (or a parent agent) drives it with `send` + `process`; the kill flag is
/// the only cross-thread surface (a force from the GUI's thread).
/// Everything an agent session needs to run, grouped so the constructor
/// stays a single parameter as the ticket series grows the loop (spec §5).
pub(crate) struct SessionParams {
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
    /// A closed session is a one-way transition: set by `SessionClose`, never
    /// cleared (unlike `stop`, which a new send resets). Marks the in-flight
    /// turn's segment interrupted.
    closed: Arc<AtomicBool>,
}

impl AgentSession {
    #[must_use]
    pub(crate) fn new(p: SessionParams) -> Self {
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
                entry_upsert_hook: None,
                queue_event_hook: None,
                subagents: p.subagents,
                child: p.child,
                parent_task_store: None,
            }),
            provider: p.provider,
            kill: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Queue a user message on its lane (spec §7). A force kills the
    /// in-flight stream and jumps to the head of the queue; everything else
    /// keeps its position.
    pub fn send(&self, text: impl Into<String>, lane: Lane) {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        // A new send clears a previous stop (ticket #23): a stop means
        // "interrupt current work", not "never work again".
        self.stop.store(false, Ordering::SeqCst);
        let msg = Queued {
            text: text.into(),
            lane,
            source: None,
            skill: None,
            in_file: false,
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
    pub(crate) fn send_skill(
        &self,
        text: impl Into<String>,
        lane: Lane,
        name: &str,
        location: &str,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        self.stop.store(false, Ordering::SeqCst);
        let msg = Queued {
            text: text.into(),
            lane,
            source: None,
            skill: Some((name.to_owned(), location.to_owned())),
            in_file: false,
        };
        if lane == Lane::Force {
            self.kill.store(true, Ordering::SeqCst);
            inner.queue.push_front(msg);
        } else {
            inner.queue.push_back(msg);
        }
    }

    /// A child that reports while
    /// the parent's turn is in flight lands on the steering lane, so the
    /// report rides the current turn's next LLM call — temporally correct
    /// where a follow-up would splice it into a later, unrelated turn.
    /// An idle parent degrades to the next turn (the wake starts one).
    pub fn send_notified_steer(&self, text: impl Into<String>, source: String) {
        self.queue_notified(text, Lane::Steering, source);
    }

    fn queue_notified(&self, text: impl Into<String>, lane: Lane, source: String) {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        self.stop.store(false, Ordering::SeqCst);
        let text = text.into();
        let steering = matches!(lane, Lane::Steering);
        // A steering report is appended to the file the moment it is queued, so
        // the EntryUpsert event fires now and the GUI shows the report immediately
        // (not at the next LLM-call boundary, which for an idle parent may be a
        // long way off). The in_file flag tells the turn loop's dequeue to skip
        // the second append.
        if steering {
            let _ = inner.store.append(
                KIND_USER,
                serde_json::json!({
                    "text": text,
                    "lane": "steering",
                    "source": source,
                }),
                None,
            );
        }
        inner.queue.push_back(Queued {
            text,
            lane,
            source: Some(source),
            skill: None,
            in_file: steering,
        });
    }

    /// A persistent stop (ticket #23): cuts the in-flight stream —
    /// including during the prefill window, before the first token — and
    /// stays set until the next send.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Mark the session closed: a one-way transition that interrupts the
    /// in-flight turn (set by `SessionClose`; unlike `stop`, a new send does
    /// not reset it).
    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// The persistent stop flag (a forwarding seam can share it so one
    /// flag cuts at both the transport mirror and the loop sink).
    pub(crate) fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Whether a queued message is waiting (a parked sub-agent with a
    /// queued message is resumed by it, ADR-0001).
    pub fn has_pending(&self) -> bool {
        !self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .queue
            .is_empty()
    }

    /// The queued message at the head of the lane queue — for a
    /// turn-starting send, the one the in-flight turn will deliver first.
    /// The harness uses it to decide whether a failed turn-start still has
    /// a pending message to surface in the queue's projection.
    pub(crate) fn first_pending(&self) -> Option<(String, Lane)> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .queue
            .front()
            .map(|q| (q.text.clone(), q.lane))
    }

    /// The session's pending messages as the GUI's queue shows them: a
    /// projection of the loop's queue (the single source of truth), taken
    /// at emit time — the GUI keeps no second ledger of its own.
    pub(crate) fn queued_items(&self) -> Vec<tau_protocol::snapshot::QueuedItem> {
        let inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        inner
            .queue
            .iter()
            .map(|q| tau_protocol::snapshot::QueuedItem {
                text: q.text.clone(),
                lane: crate::harness::lane_to_message_lane(q.lane),
                source: q.source.clone(),
            })
            .collect()
    }

    /// The child-side link (a child session's `parent_notify` routing).
    pub(crate) fn child_link(&self) -> Option<Arc<crate::subagent::ChildLink>> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .child
            .clone()
    }

    /// This session's supervisor (None for a child session — children
    /// cannot spawn, so the supervisor is parent-side only).
    pub(crate) fn subagents(&self) -> Option<Arc<crate::subagent::Supervisor>> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .subagents
            .clone()
    }

    /// The `recall` tool (spec §4): a child's `scope: "parent"` browses
    /// the parent session's raw history (a compacted child's frozen prefix
    /// points there); everything else is this session's own log.
    pub(crate) fn recall_scoped(&self, args: &Value) -> String {
        if args.get("scope").and_then(Value::as_str) == Some("parent") {
            let (link, cwd) = {
                let inner = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
                (inner.child.clone(), inner.cwd.clone())
            };
            match link {
                Some(link) => {
                    // The parent id comes from the link (R3): the handle
                    // is opaque, never parsed.
                    let mut store = SessionStore::for_workspace(&cwd, link.parent_session());
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
            let mut inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            let record = inner
                .om
                .as_ref()
                .map(|om| om.record.clone())
                .unwrap_or_default();
            crate::om_integration::recall(&mut inner.store, &record, args)
        }
    }

    /// The parent's task-routing target (child sessions only): set at
    /// spawn from the supervisor's attached parent.
    pub(crate) fn set_parent_task_store(&self, parent: Option<Weak<AgentSession>>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .parent_task_store = parent;
    }

    /// The task tools (spec §5.3/§5.4). A child session's worker tools
    /// route to the parent's store — the task never leaves the parent's
    /// file (one store, one lock); the child's pane is a projection.
    /// The child's `inner` is read and released before the parent's lock
    /// is taken: no child-lock → parent-lock nesting (the deadlock guard).
    fn task_tool_dispatch(&self, name: &str, args: &Value) -> String {
        let (is_child, parent) = {
            let inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            (
                inner.child.is_some(),
                inner.parent_task_store.as_ref().and_then(Weak::upgrade),
            )
        };
        if let Some(r) = tools::surface::task_child_refusal(is_child, name) {
            return r;
        }
        if let Some(parent) = parent {
            parent.with_task_store(|store| crate::task::tool_call(store, name, args))
        } else {
            let mut inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            crate::task::tool_call(&mut inner.store, name, args)
        }
    }

    /// The session store's header timestamp (epoch ms).
    pub(crate) fn store_created(&self) -> u64 {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .store
            .created()
    }

    /// Run one operation against this session's own store — the
    /// single-writer surface for other components (the sub-agent task
    /// gate, the app's task commands; review B3).
    pub(crate) fn with_task_store<R>(&self, f: impl FnOnce(&mut SessionStore) -> R) -> R {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        f(&mut inner.store)
    }

    /// The seven task tools (the app's task commands; the model's dispatch
    /// routes them through the surface) — both share the child→parent routing.
    pub(crate) fn task_tool_call(&self, name: &str, args: &Value) -> String {
        self.task_tool_dispatch(name, args)
    }

    #[cfg(test)]
    /// Test-only OM state injection (the sub-agent module's tests).
    pub(crate) fn set_om(&self, om: Option<crate::om_integration::OmState>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .om = om;
    }

    /// The OM-run observer (the app's `om_status` emitter); `None` clears it.
    pub(crate) fn set_om_status_hook(&self, hook: Option<OmStatusHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .om_status_hook = hook;
    }

    pub(crate) fn set_queue_event_hook(&self, hook: Option<QueueEventHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .queue_event_hook = hook;
    }

    /// The live entry sink, set on this session's store (the append choke
    /// point); `None` clears it.
    pub(crate) fn set_entry_event_hook(&self, hook: Option<EntryEventHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .store
            .set_entry_event_hook(hook);
    }

    /// The wire-only upsert sink (ADR-0008); `None` clears it.
    pub(crate) fn set_entry_upsert_hook(&self, hook: Option<EntryEventHook>) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .entry_upsert_hook = hook;
    }

    /// The session's model for the next turn's calls (the `session_set_model`
    /// command's live half; provider resolution happens at the call).
    pub(crate) fn set_model(&self, model: String) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .model = model;
    }

    /// The turn's provider options (spec §12, #35): re-derived when the
    /// session's model changes, so clamping and reasoning levels track the
    /// active model.
    pub(crate) fn set_turn_config(&self, turn: TurnConfig) {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .turn = turn;
    }

    /// Append a record entry against the active leaf (the sub-agent
    /// lifecycle records, ticket #23).
    pub(crate) fn append_entry(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        self.append(kind, payload)
    }

    pub(crate) fn model(&self) -> String {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .model
            .clone()
    }

    pub(crate) fn tools(&self) -> Vec<ToolSpec> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .tools
            .clone()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn system_prompt(&self) -> String {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .system_prompt
            .clone()
    }

    /// The session's OM state (ticket #22); `None` = OM disabled.
    pub(crate) fn om_state(&self) -> Option<crate::om_integration::OmState> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .om
            .clone()
    }
}
mod turn;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_om;
#[cfg(test)]
mod tests_turns;
