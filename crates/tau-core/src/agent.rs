//! The agent loop and the message lanes (spec §2, §7): one turn = system
//! prompt + history → provider call → tool dispatch → repeat until the model
//! ends the turn. Lanes: **force** (kill the in-flight stream, partial kept
//! as an `interrupted` entry, tool batch completed per config, message at
//! the head of the queue), **steering** (delivered at the next LLM call),
//! **follow-up** (delivered after the turn completes).

use crate::config::ToolBatchPolicy;
use crate::provider::{
    FunctionCall, FunctionCallInput, FunctionCallOutputInput, InputEntry, InputMessage,
    ReasoningEffort, ResponseRequest, ToolSpec, TurnProviderRef, TurnResult, TurnSink,
};
use crate::session::{Entry, SessionStore};
use crate::tools;
use serde_json::{Value, json};
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

/// Per-turn provider limits (v0: fixed for the session's life; the hosted
/// test model is a thinking model, so capping reasoning is how live tests
/// stay cheap).
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnConfig {
    pub max_output_tokens: Option<u64>,
    pub reasoning: Option<ReasoningEffort>,
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

    /// Append a record entry against the active leaf (the sub-agent
    /// lifecycle records, ticket #23).
    pub fn append_entry(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        self.append(kind, payload)
    }

    /// Run turns until the queue is empty.
    pub async fn process(&self) -> Result<(), AgentError> {
        loop {
            let Some(starter) = self.inner.lock().unwrap().queue.pop_front() else {
                return Ok(());
            };
            self.append_user(starter).await?;
            self.run_turn().await?;
        }
    }

    async fn run_turn(&self) -> Result<(), AgentError> {
        let mut rounds = 0usize;
        loop {
            rounds += 1;
            if rounds > MAX_ROUNDS {
                // A runaway turn dies visibly, not silently: the session
                // records why it stopped.
                self.append(
                    KIND_SYSTEM,
                    json!({
                        "note": format!(
                            "Stopped after {MAX_ROUNDS} tool rounds without the model ending the turn"
                        )
                    }),
                )?;
                break;
            }
            // Steering and forced messages ride this LLM call (spec §7);
            // follow-ups wait for the turn boundary.
            let delivered = {
                let mut inner = self.inner.lock().unwrap();
                let mut out = Vec::new();
                inner.queue.retain(|m| {
                    if matches!(m.lane, Lane::Steering | Lane::Force) {
                        out.push(m.clone());
                        false
                    } else {
                        true
                    }
                });
                out
            };
            for msg in delivered {
                self.append_user(msg).await?;
            }

            // The context (spec §4): with OM enabled, the system prompt
            // carries the observation log and the raw window is the
            // unobserved entries; without it, the session's entries as-is.
            // Everything is read under one lock, then assembled purely.
            let (system_prompt, input) = {
                let mut inner = self.inner.lock().unwrap();
                let base = inner.system_prompt.clone();
                let entries = inner
                    .store
                    .entries_range(0, usize::MAX)
                    .map_err(AgentError::Session)?;
                let leaf_id = inner
                    .store
                    .leaf()
                    .map_err(AgentError::Session)?
                    .map(|e| e.id);
                // The active tasks' resume contracts (spec §5.3): re-injected
                // into every assembly while active, so a compaction can
                // never make a session lose sight of them. All active tasks
                // ride (bounded) — a session may work several (review N4).
                let contracts = crate::task::active_tasks(&crate::task::fold_entries(&entries))
                    .iter()
                    .take(5)
                    .map(|t| crate::task::resume_contract(t).to_string())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let contract = if contracts.is_empty() {
                    None
                } else {
                    Some(contracts)
                };
                // Assembly runs on the persistent state, not a clone: it is
                // pure over the record, and the one-shot continuation-hint
                // flip must stick (a clone's flip would be dropped, and the
                // om_turn_end write-back would re-set `changed`, so the hint
                // would re-inject on every assembly).
                match inner.om.as_mut() {
                    Some(om) => {
                        let instructions = om.assemble_context(&base, contract.as_deref());
                        let raw = om.raw_window_from(&entries, leaf_id.as_deref());
                        (instructions, input_items(&raw))
                    }
                    None => {
                        let mut instructions = base;
                        if let Some(contract) = &contract {
                            instructions.push_str("\n\n# Task (resume contract)\n");
                            instructions.push_str(contract);
                        }
                        (instructions, input_items(&entries))
                    }
                }
            };
            let mut request =
                ResponseRequest::new(self.model().to_owned(), Some(system_prompt.as_str()), input)
                    .with_tools(self.tools().clone());
            if let Some(n) = self.turn_config().max_output_tokens {
                request = request.with_max_output_tokens(n);
            }
            if let Some(e) = self.turn_config().reasoning {
                request = request.with_reasoning(e);
            }

            // A force has already killed the stream it targeted; every
            // fresh call starts un-killed (spec §7).
            self.kill.store(false, Ordering::SeqCst);
            let mut sink = KillSink {
                kill: self.kill.clone(),
                stop: self.stop.clone(),
            };
            let result = self.provider.call(&request, &mut sink).await?;
            self.append_assistant(&result)?;

            if !result.completed {
                // A force-killed turn skips the OM pass: the Observer needs
                // no tool batch in flight, and the interrupted material
                // joins the next turn's unobserved window (v0).
                // Per the policy, the in-flight tool batch is let to
                // complete, then one final call so the model sees the tool
                // results alongside the forced message (spec §7).
                if self.tool_batch_on_force() == ToolBatchPolicy::Complete
                    && !result.calls.is_empty()
                {
                    self.run_tools(&result.calls).await?;
                    continue;
                }
                break;
            }
            if result.calls.is_empty() {
                // The model ended the turn: the synchronous OM pass runs
                // before any follow-up starts the next one (spec §4).
                self.om_turn_end().await?;
                break;
            }
            self.run_tools(&result.calls).await?;
            if self
                .inner
                .lock()
                .unwrap()
                .child
                .as_ref()
                .is_some_and(|c| !c.is_running())
            {
                // Quiescence (ADR-0001): the child's notify ended it, so the
                // core auto-terminates its loop — a done child never burns
                // another provider call.
                break;
            }
        }
        Ok(())
    }

    async fn run_tools(&self, calls: &[FunctionCall]) -> Result<(), AgentError> {
        for call in calls {
            let args = serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null);
            let tc = tools::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args: args.clone(),
            };
            let output = if call.name == "task_assign" {
                // Cross-session: routed through the supervisor, which runs
                // both sides on the sessions' own stores (review B3).
                let sup = {
                    let inner = self.inner.lock().unwrap();
                    inner.subagents.clone()
                };
                match sup {
                    Some(sup) => {
                        let task = tc.args.get("task").and_then(Value::as_str).unwrap_or("");
                        let worker = tc.args.get("worker").and_then(Value::as_str).unwrap_or("");
                        match sup.assign_task(task, worker) {
                            Ok(()) => format!("assigned {task} to {worker}"),
                            Err(e) => e,
                        }
                    }
                    None => "task_assign: this session has no sub-agents".into(),
                }
            } else if call.name.starts_with("task_") {
                // Tasks live in this session's own store (spec §5.3) — parent
                // and child alike. Routed before the sub-agent surface so a
                // child (which has no supervisor) still gets its tools.
                self.task_tool(&tc)
            } else if call.name == "recall" {
                self.recall_scoped(&args)
            } else {
                // The sub-agent surface routes outside the core tools: the
                // parent's supervisor tools, or the child's `parent_notify`
                // (ticket #23); everything else is a core tool. The links
                // are cloned under the lock and invoked outside it: the
                // routes re-lock this session's `inner` (a child notify
                // records a state entry; a spawn reads the parent's OM
                // record), and the lock is not reentrant.
                let (sup, link) = {
                    let inner = self.inner.lock().unwrap();
                    (inner.subagents.clone(), inner.child.clone())
                };
                // Route by name, not by which link exists: a parent session
                // carries a supervisor AND core tools (ticket #23's original
                // `if let Some(sup)` swallowed every core tool into
                // route_parent, which only knows sub-agent names), and a
                // child carries a link AND core tools.
                if call.name.starts_with("subagent_") {
                    match sup {
                        Some(sup) => crate::subagent::route_parent(&sup, &tc),
                        None => format!("{}: not available in this session", call.name),
                    }
                } else if call.name == "parent_notify" {
                    match link {
                        Some(link) => link.notify(&args),
                        None => "parent_notify: not available in a top-level session".into(),
                    }
                } else {
                    tools::dispatch(&self.cwd(), &tc).await
                }
            };
            self.append(
                KIND_TOOL,
                json!({
                    "call_id": call.call_id,
                    "name": call.name,
                    "args": args,
                    "output": output,
                }),
            )?;
        }
        Ok(())
    }

    /// The turn-end OM pass (ticket #22): the state runs on a clone and
    /// every store op takes the session lock briefly; the LLM round-trips
    /// run between the lock scopes, so a force or steering send is never
    /// blocked on an OM call.
    async fn om_turn_end(&self) -> Result<(), AgentError> {
        let (mut state, hook) = {
            let inner = self.inner.lock().unwrap();
            (inner.om.clone(), inner.om_status_hook.clone())
        };
        let Some(state) = &mut state else {
            return Ok(());
        };
        let mut action = {
            let mut inner = self.inner.lock().unwrap();
            let unobserved = state.unobserved(&mut inner.store).map_err(AgentError::Om)?;
            state.record.pending_tokens = state.pending_tokens(&unobserved);
            // Activation (no LLM call): the token threshold, or the fixed
            // idle timeout with pending chunks (spec §4).
            let idle = {
                let all = inner
                    .store
                    .entries_range(0, usize::MAX)
                    .map_err(|e| AgentError::Om(e.into()))?;
                let leaf = inner
                    .store
                    .leaf()
                    .map_err(|e| AgentError::Om(e.into()))?
                    .map(|e| e.id);
                crate::om_integration::idle_gap_secs(&all, leaf.as_deref())
            };
            if !state.buffered.is_empty()
                && (state.activation_reached(state.record.pending_tokens)
                    || idle >= crate::om_integration::IDLE_ACTIVATION_SECS)
            {
                state.promote(&mut inner.store).map_err(AgentError::Om)?;
            }
            state.plan(&unobserved)
        };
        loop {
            // The activity the status bar's gauge shows while this run is in
            // flight; `Done` closes the run (the hook is the app's om_status
            // emitter, absent in tests and on children).
            if let Some(hook) = &hook {
                let kind = match &action {
                    crate::om_integration::TurnEndAction::Done => "idle",
                    crate::om_integration::TurnEndAction::Observe { .. }
                    | crate::om_integration::TurnEndAction::Buffer { .. } => "observing",
                    crate::om_integration::TurnEndAction::Reflect { .. } => "reflecting",
                };
                hook(kind);
            }
            let result = match &action {
                crate::om_integration::TurnEndAction::Done => break,
                crate::om_integration::TurnEndAction::Observe { transcript }
                | crate::om_integration::TurnEndAction::Buffer { transcript } => {
                    let system = crate::om::observer_system_prompt();
                    let request = ResponseRequest::new(
                        self.om_model(),
                        Some(system.as_str()),
                        vec![InputEntry::Message(InputMessage {
                            role: "user".into(),
                            content: transcript.clone(),
                        })],
                    );
                    let mut sink = crate::om_integration::NoopSink;
                    self.provider
                        .call(&request, &mut sink)
                        .await
                        .map_err(AgentError::Provider)?
                }
                crate::om_integration::TurnEndAction::Reflect { level } => {
                    let prompt = state.reflector_prompt(*level);
                    let system = crate::om::reflector_system_prompt();
                    let request = ResponseRequest::new(
                        self.om_model(),
                        Some(system.as_str()),
                        vec![InputEntry::Message(InputMessage {
                            role: "user".into(),
                            content: prompt,
                        })],
                    );
                    let mut sink = crate::om_integration::NoopSink;
                    self.provider
                        .call(&request, &mut sink)
                        .await
                        .map_err(AgentError::Provider)?
                }
            };
            {
                let mut inner = self.inner.lock().unwrap();
                state
                    .commit(&mut inner.store, &mut action, &result)
                    .map_err(AgentError::Om)?;
            }
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.om = Some(state.clone());
        }
        Ok(())
    }

    /// The OM model (global config, spec §4); empty = the session's model.
    fn om_model(&self) -> String {
        let inner = self.inner.lock().unwrap();
        if inner.om_model.is_empty() {
            inner.model.clone()
        } else {
            inner.om_model.clone()
        }
    }

    async fn append_user(&self, msg: Queued) -> Result<(), AgentError> {
        let mut payload = json!({ "text": msg.text, "lane": lane_name(msg.lane) });
        if let Some(source) = &msg.source {
            payload["source"] = json!(source);
        }
        if let Some((name, location)) = &msg.skill {
            payload["skill"] = json!({ "name": name, "location": location });
        }
        self.append(KIND_USER, payload)
    }

    fn append_assistant(&self, result: &TurnResult) -> Result<(), AgentError> {
        // A partial cut before anything arrived has nothing to record
        // (review N10): an empty, call-less interrupted entry is noise.
        if !result.completed
            && result.text.is_empty()
            && result.reasoning.is_empty()
            && result.calls.is_empty()
        {
            return Ok(());
        }
        self.append(
            KIND_ASSISTANT,
            json!({
                "text": result.text,
                "reasoning": result.reasoning,
                "interrupted": !result.completed,
                "usage": result.usage,
                "calls": result.calls,
            }),
        )
    }

    fn append(&self, kind: &str, payload: Value) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
        // A fresh session has no leaf: the first entry starts the branch.
        // Any other failure is a storage error and propagates.
        let parent = match inner.store.leaf() {
            Ok(leaf) => leaf.map(|e| e.id),
            Err(e) => return Err(AgentError::Session(e)),
        };
        let parent = parent.as_deref();
        inner.store.append(kind, payload, parent)?;
        Ok(())
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

    fn cwd(&self) -> PathBuf {
        self.inner.lock().unwrap().cwd.clone()
    }

    fn tool_batch_on_force(&self) -> ToolBatchPolicy {
        self.inner.lock().unwrap().tool_batch_on_force
    }

    fn turn_config(&self) -> TurnConfig {
        self.inner.lock().unwrap().turn
    }

    /// The session's OM state (ticket #22); `None` = OM disabled.
    pub fn om_state(&self) -> Option<crate::om_integration::OmState> {
        self.inner.lock().unwrap().om.clone()
    }
}

/// The sink the loop gives the provider: the per-call kill flag (force)
/// or the persistent stop flag (ticket #23) stops the stream (spec §7).
struct KillSink {
    kill: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl TurnSink for KillSink {
    fn event(&mut self, _: crate::provider::TurnEvent) -> bool {
        !self.kill.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst)
    }
}

/// The session's entries (file order) mapped to responses-API input items:
/// user messages, assistant text + calls, and tool results (spec §5.4).
fn input_items(entries: &[Entry]) -> Vec<InputEntry> {
    let mut out = Vec::new();
    for entry in entries {
        match entry.kind.as_str() {
            KIND_USER => {
                if let Some(text) = entry.payload.get("text").and_then(Value::as_str) {
                    // A wake message from a child (ticket #23) is stored as a
                    // user entry with its source: the prefix keeps the parent
                    // from reading the child's words as the user's.
                    let content = match entry.payload.get("source").and_then(Value::as_str) {
                        Some(source) => format!("Sub-agent {source}: {text}"),
                        None => text.to_owned(),
                    };
                    out.push(InputEntry::Message(InputMessage {
                        role: "user".into(),
                        content,
                    }));
                }
            }
            KIND_ASSISTANT => {
                if let Some(text) = entry
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    out.push(InputEntry::Message(InputMessage {
                        role: "assistant".into(),
                        content: text.to_owned(),
                    }));
                }
                if let Some(calls) = entry.payload.get("calls").and_then(Value::as_array) {
                    for call in calls {
                        out.push(InputEntry::Call(FunctionCallInput {
                            kind: "function_call",
                            id: call
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            call_id: call
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            name: call
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            arguments: call
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        }));
                    }
                }
            }
            KIND_TOOL => {
                let (Some(call_id), Some(output)) = (
                    entry.payload.get("call_id").and_then(Value::as_str),
                    entry.payload.get("output").and_then(Value::as_str),
                ) else {
                    continue;
                };
                out.push(InputEntry::CallOutput(FunctionCallOutputInput {
                    kind: "function_call_output",
                    call_id: call_id.to_owned(),
                    output: output.to_owned(),
                }));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{canned, canned_cut};

    fn session_in(dir: &std::path::Path) -> SessionStore {
        let mut store = SessionStore::for_workspace(dir, "s1");
        store.create().unwrap();
        store
    }

    fn make_agent(dir: &std::path::Path, provider: TurnProviderRef) -> AgentSession {
        AgentSession::new(SessionParams {
            store: session_in(dir),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.to_path_buf(),
            provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        })
    }

    fn sse(text: &str, calls: &[(String, String, String)]) -> String {
        // (name, call_id, arguments-json)
        let mut body = String::new();
        for (name, call_id, args) in calls {
            let item = serde_json::json!({
                "id": call_id,
                "type": "function_call",
                "name": name,
                "call_id": call_id,
                "arguments": args,
            });
            body.push_str(&format!(
                "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\n"
            ));
        }
        if !text.is_empty() {
            body.push_str(&format!(
                "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n"
            ));
        }
        body.push_str(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        );
        body.push_str("data: [DONE]\n\n");
        body
    }

    /// Like sse(), but the text is JSON-escaped (multi-line deltas).
    fn sse_json(text: &str) -> String {
        let delta = json!({ "type": "response.output_text.delta", "delta": text });
        let done = json!({
            "type": "response.completed",
            "response": { "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } }
        });
        format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", delta, done)
    }

    /// A canned provider scripted per call: each entry is (sse body, calls).
    /// `seen` captures (instructions, first input message content) per call.
    struct ScriptedProvider {
        calls: Vec<String>,
        index: std::sync::atomic::AtomicUsize,
        seen: std::sync::Mutex<Vec<(Option<String>, Option<String>)>>,
    }

    impl ScriptedProvider {
        fn new(calls: Vec<String>) -> Self {
            Self {
                calls,
                index: std::sync::atomic::AtomicUsize::new(0),
                seen: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl crate::provider::TurnProvider for ScriptedProvider {
        fn call<'a>(
            &self,
            request: &ResponseRequest,
            sink: &'a mut dyn TurnSink,
        ) -> crate::provider::ProviderTurn<'a> {
            let body = self
                .calls
                .get(self.index.fetch_add(1, Ordering::SeqCst) % self.calls.len())
                .cloned()
                .unwrap_or_else(|| sse("", &[]));
            let captured = serde_json::to_value(request).unwrap();
            self.seen.lock().unwrap().push((
                captured
                    .get("instructions")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                captured
                    .get("input")
                    .and_then(|i| i.get(0))
                    .and_then(|i| i.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ));
            let provider = canned(&body);
            provider.call(request, sink)
        }
    }

    fn entries_of(store: &SessionStore) -> Vec<Entry> {
        store.entries_range(0, usize::MAX).unwrap()
    }

    #[tokio::test]
    async fn task_tools_run_through_the_loop_and_the_gate_enforces_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_in(dir.path());
        // The session gets the full non-child tool set (incl. tasks).
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::agent_tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: Arc::new(ScriptedProvider::new(vec![
                sse(
                    "",
                    &[(
                        "task_create".into(),
                        "c1".into(),
                        r#"{"title":"write the docs","criteria":["docs exist"]}"#.into(),
                    )],
                ),
                sse(
                    "",
                    &[(
                        "task_start".into(),
                        "c2".into(),
                        r#"{"task":"task-1"}"#.into(),
                    )],
                ),
                // The gate: no evidence yet → the finish fails with the gap.
                sse(
                    "",
                    &[(
                        "task_finish".into(),
                        "c3".into(),
                        r#"{"task":"task-1"}"#.into(),
                    )],
                ),
                sse(
                    "",
                    &[(
                        "task_evidence".into(),
                        "c4".into(),
                        r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                    )],
                ),
                // The same finish now passes.
                sse(
                    "",
                    &[(
                        "task_finish".into(),
                        "c5".into(),
                        r#"{"task":"task-1"}"#.into(),
                    )],
                ),
                sse("done", &[]),
            ])),
            tool_batch_on_force: Default::default(),
            turn: Default::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        agent.send("work the task", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let out = |id: &str| -> String {
            entries
                .iter()
                .find(|e| e.payload["call_id"] == id)
                .unwrap()
                .payload["output"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(out("c1").contains("created task-1"), "{}", out("c1"));
        assert!(out("c2").contains("[in_progress]"), "{}", out("c2"));
        assert!(
            out("c3").contains("completion gate failed"),
            "{}",
            out("c3")
        );
        assert!(out("c4").contains("[in_progress]"), "{}", out("c4"));
        assert!(out("c5").contains("[done]"), "{}", out("c5"));
        // The session file holds the task events, and the fold ends done.
        let task_entries = entries
            .iter()
            .filter(|e| e.kind == crate::task::KIND_TASK)
            .count();
        assert_eq!(task_entries, 4); // the failed finish appends nothing
        let tasks = crate::task::fold_entries(&entries);
        assert_eq!(tasks[0].status, crate::task::STATUS_DONE);
    }

    #[tokio::test]
    async fn a_plain_turn_appends_user_then_assistant() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
        let agent = make_agent(dir.path(), provider);
        agent.send("hello", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        assert_eq!(
            entries.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            vec![KIND_USER, KIND_ASSISTANT]
        );
        assert_eq!(entries[0].payload["text"], "hello");
        assert_eq!(entries[1].payload["text"], "done");
        assert!(!entries[1].payload["interrupted"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn tool_calls_roundtrip_through_the_session() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
        let body1 = sse(
            "",
            &[("read".into(), "c1".into(), r#"{"path":"n.txt"}"#.into())],
        );
        let body2 = sse("read it", &[]);
        let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
        let agent = make_agent(dir.path(), provider);
        agent.send("read n.txt", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![KIND_USER, KIND_ASSISTANT, KIND_TOOL, KIND_ASSISTANT]
        );
        assert_eq!(entries[2].payload["call_id"], "c1");
        assert_eq!(entries[2].payload["name"], "read");
        assert!(entries[2].payload["output"].as_str().unwrap().contains("a"));
    }

    #[tokio::test]
    async fn steering_lands_on_the_next_llm_call() {
        let dir = tempfile::tempdir().unwrap();
        let body1 = sse(
            "",
            &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
        );
        let body2 = sse("after steering", &[]);
        let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
        let agent = make_agent(dir.path(), provider);
        agent.send("start", Lane::FollowUp);
        agent.send("steer me", Lane::Steering);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        // The steering message rides the next LLM call: it is a user entry
        // delivered at the head of the same turn as its starter.
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                KIND_USER,
                KIND_USER,
                KIND_ASSISTANT,
                KIND_TOOL,
                KIND_ASSISTANT
            ]
        );
        assert_eq!(entries[1].payload["text"], "steer me");
        assert_eq!(entries[1].payload["lane"], "steering");
    }

    #[tokio::test]
    async fn follow_up_lands_after_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("first done", &[]),
            sse("second done", &[]),
        ]));
        let agent = make_agent(dir.path(), provider);
        agent.send("first", Lane::FollowUp);
        agent.send("later", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![KIND_USER, KIND_ASSISTANT, KIND_USER, KIND_ASSISTANT]
        );
        assert_eq!(entries[1].payload["text"], "first done");
        // The follow-up starts the *next* turn: its user entry sits between
        // the two assistant entries, not inside the first turn.
        assert_eq!(entries[2].payload["text"], "later");
        assert_eq!(entries[3].payload["text"], "second done");
    }

    #[tokio::test]
    async fn force_kills_the_stream_and_keeps_the_partial() {
        let dir = tempfile::tempdir().unwrap();
        // A long stream the force cuts after two text events; no calls, so
        // the turn ends at the kill.
        let body = sse("a", &[]) + sse("b", &[]).as_str();
        let provider = canned_cut(&body, 1);
        let agent = make_agent(dir.path(), provider);
        agent.send("go", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let assistant = entries.iter().find(|e| e.kind == KIND_ASSISTANT).unwrap();
        assert!(assistant.payload["interrupted"].as_bool().unwrap());
        assert_eq!(assistant.payload["text"], "a");
    }

    #[tokio::test]
    async fn force_preempts_a_queued_steering_at_the_head_of_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("killed", &[]),
            sse("next", &[]),
        ]));
        let agent = make_agent(dir.path(), provider);
        agent.send("go", Lane::FollowUp);
        agent.send("queued steering", Lane::Steering);
        // The force arrives mid-turn: kill + head of queue.
        agent.send("FORCE", Lane::Force);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let users: Vec<&str> = entries
            .iter()
            .filter(|e| e.kind == KIND_USER)
            .map(|e| e.payload["text"].as_str().unwrap())
            .collect();
        // The forced message is delivered before the queued steering, on
        // the turn that follows the killed one.
        assert_eq!(users, vec!["FORCE", "queued steering", "go"]);
    }

    /// A force sent while the stream is IN FLIGHT (not a pre-cut): the kill
    /// flag stops the slow stream mid-way, the partial stands as interrupted,
    /// and the forced message preempts a steering queued at the same instant
    /// on the next call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn force_mid_stream_kills_the_stream_and_preempts_contemporaneous_steering() {
        let dir = tempfile::tempdir().unwrap();
        // One stream, four 40 ms-apart text deltas, ending in a single
        // completed event: left alone it runs ~200 ms to completion, the
        // force at ~100 ms cuts it mid-way. (Built by hand — sse() ends each
        // chunk in [DONE], and the decoder stops at the first one.)
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"c\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"d\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
            "data: [DONE]\n\n"
        );
        let provider = crate::provider::canned_slow(body, 40);
        let agent = Arc::new(make_agent(dir.path(), provider));
        agent.send("go", Lane::FollowUp);
        let killer = {
            let agent = agent.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                agent.send("FORCE", Lane::Force);
                agent.send("steer", Lane::Steering);
            })
        };
        agent.process().await.unwrap();
        killer.await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                KIND_USER,
                KIND_ASSISTANT,
                KIND_USER,
                KIND_USER,
                KIND_ASSISTANT
            ]
        );
        let killed = &entries[1];
        assert!(
            killed.payload["interrupted"].as_bool().unwrap(),
            "{killed:?}"
        );
        // a, b, c land before the 100 ms force (d is at 120 ms); under a
        // loaded runner the 80 ms c may lose the race, so accept ab or abc.
        let partial = killed.payload["text"].as_str().unwrap();
        assert!(
            partial == "ab" || partial == "abc",
            "partial was {partial:?}"
        );
        assert_eq!(entries[2].payload["text"], "FORCE");
        assert_eq!(entries[3].payload["text"], "steer");
        // The following call starts un-killed and runs the stream to completion.
        assert_eq!(entries[4].payload["text"], "abcd");
        assert!(!entries[4].payload["interrupted"].as_bool().unwrap());
    }

    /// Live acceptance (ticket #19): a four-tool session against the hosted
    /// vLLM endpoint; skipped unless TAU_TEST_ENDPOINT is set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_tool_calling_session() {
        let base = match std::env::var("TAU_TEST_ENDPOINT") {
            Ok(base) => base,
            Err(_) => return,
        };
        let model = std::env::var("TAU_TEST_MODEL").unwrap_or_else(|_| "qwen3.8-27b".into());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "line1\nline2\nline3\n").unwrap();
        let provider = crate::provider::production(
            &crate::provider::tests::test_client(),
            &crate::config::Provider {
                base_url: base,
                key_env: String::new(),
                models: vec![model.clone()],
            },
            &crate::config::Requests::default(),
        );
        let agent = AgentSession::new(SessionParams {
            store: session_in(dir.path()),
            system_prompt:
                "You have the tools read, write, edit, and bash. Use them as                  instructed; edit takes the 3-char anchors from read output."
                    .into(),
            model,
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig {
                max_output_tokens: Some(200),
                reasoning: Some(crate::provider::ReasoningEffort::Low),
            },
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        agent.send(
            "Do exactly this: 1) read notes.txt, 2) edit the line containing              'line2' so it becomes 'LINE2', 3) bash: cat notes.txt, 4) write              the file out.txt with the single line 'done'. Then reply              'finished'.",
            Lane::FollowUp,
        );
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let called: Vec<&str> = entries
            .iter()
            .filter(|e| e.kind == KIND_TOOL)
            .map(|e| e.payload["name"].as_str().unwrap())
            .collect();
        for want in ["read", "edit", "bash", "write"] {
            assert!(called.contains(&want), "missing {want}: {called:?}");
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "line1\nLINE2\nline3\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "done"
        );
    }

    #[tokio::test]
    async fn append_propagates_storage_errors_not_a_new_root() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![sse("ok", &[])]));
        let agent = make_agent(dir.path(), provider);
        agent.send("go", Lane::FollowUp);
        agent.process().await.unwrap();
        // Lose the file under the session: the next append must fail with a
        // storage error, not silently start a new root branch.
        std::fs::remove_file(agent.inner.lock().unwrap().store.path()).unwrap();
        agent.send("again", Lane::FollowUp);
        assert!(agent.process().await.is_err());
    }

    #[tokio::test]
    async fn runaway_turn_stops_with_a_visible_note() {
        let dir = tempfile::tempdir().unwrap();
        // The same tool call, forever: the round cap is the only exit.
        let body = sse(
            "",
            &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
        );
        let provider = Arc::new(ScriptedProvider::new(vec![body]));
        let agent = make_agent(dir.path(), provider);
        agent.send("loop", Lane::FollowUp);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let note = entries
            .iter()
            .find(|e| e.kind == KIND_SYSTEM)
            .expect("a system note records why the turn stopped");
        assert!(
            note.payload["note"].as_str().unwrap().contains("32"),
            "{note:?}"
        );
        assert_eq!(
            entries.iter().filter(|e| e.kind == KIND_TOOL).count(),
            MAX_ROUNDS
        );
    }

    #[tokio::test]
    async fn kill_policy_completes_the_inflight_tool_batch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "x\n").unwrap();
        // The killed stream had a completed function_call before the cut:
        // with the Complete policy the tool runs and the turn continues to
        // a final call where the forced message lands alongside the result.
        let mut body = String::new();
        body.push_str(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"c1\",\"type\":\"function_call\",\"name\":\"read\",\"call_id\":\"c1\",\"arguments\":\"{\\\"path\\\":\\\"x.txt\\\"}\"}}\n\n",
        );
        body.push_str("data: {\"type\":\"response.output_text.delta\",\"delta\":\"cut\"}\n\n");
        let body = body;
        let provider = canned_cut(&body, 1);
        let agent = AgentSession::new(SessionParams {
            store: session_in(dir.path()),
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider,
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        agent.send("go", Lane::FollowUp);
        agent.send("FORCE", Lane::Force);
        agent.process().await.unwrap();
        let entries = entries_of(&agent.inner.lock().unwrap().store);
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        // killed assistant + tool result + (the next scripted call = the
        // canned cut again, since the provider is single-shot) + the forced
        // user entry is delivered at the head of that final call.
        assert!(kinds.contains(&KIND_TOOL), "{kinds:?}");
        let interrupted = entries
            .iter()
            .find(|e| e.kind == KIND_ASSISTANT && e.payload["interrupted"].as_bool() == Some(true))
            .unwrap();
        assert_eq!(interrupted.payload["calls"].as_array().unwrap().len(), 1);
    }

    /// End-to-end OM (the ticket's acceptance bar): synthesized raw entries
    /// cross the observe threshold, the canned Observer fills the log past
    /// the reflect threshold, the canned Reflector rewrites the suffix —
    /// and the continuation hint is injected exactly once across the run
    /// (B2 at the agent level).
    #[tokio::test]
    async fn om_crosses_observe_and_reflect_and_the_hint_is_one_shot() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path());
        // Real content accumulation: 5 synthesized entries cross the
        // 1000-token observe threshold (5 x 225 tokens).
        let mut parent: Option<String> = None;
        for _ in 0..5 {
            let e = store
                .append(
                    "user",
                    json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                    parent.as_deref(),
                )
                .unwrap();
            parent = Some(e.id);
        }
        let cfg = crate::config::Om {
            om_model: String::new(),
            observe_threshold: 1000,
            reflect_threshold: 2000,
            buffer_increment: 230,
        };
        let obs_text = format!(
            "<observations>obs {}</observations>",
            (0..900)
                .map(|i| format!("L{:04} data", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let ref_text = format!(
            "<observations>condensed {}</observations>",
            (0..200)
                .map(|i| format!("c{:04}", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("a1", &[]),
            sse_json(&obs_text),
            sse("a2", &[]),
            sse_json(&ref_text),
        ]));
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: provider.clone(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: Some(crate::om_integration::OmState::from_config(
                &cfg,
                crate::om::OmRecord::default(),
            )),
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        agent.send("go1", Lane::FollowUp);
        agent.send("go2", Lane::FollowUp);
        agent.process().await.unwrap();

        let state = agent
            .inner
            .lock()
            .unwrap()
            .om
            .clone()
            .expect("the om state is written back");
        // Observe: the log is non-empty and the cursor sits on turn 1's
        // last raw entry (the raw window for turn 2 is the new user entry).
        assert!(state.record.active_observations.contains("obs"));
        assert_eq!(state.record.cursor.unwrap().entry_id, "00000007");
        // Reflect: the tagged reflection committed its <observations>
        // content only, as generation 1.
        assert_eq!(state.record.generation, 1);
        assert!(state.record.active_observations.contains("condensed"));
        assert!(!state.record.active_observations.contains("<observations>"));
        // The continuation hint: exactly one assembled context in the whole
        // run carries it — the turn-2 assembly, right after the observe.
        let seen = provider.seen.lock().unwrap();
        let hints = seen
            .iter()
            .filter(|(ins, _)| {
                ins.as_deref()
                    .is_some_and(|i| i.contains(crate::om::OBSERVATION_CONTINUATION_HINT))
            })
            .count();
        assert_eq!(hints, 1, "the hint is one-shot: {seen:?}");
    }

    /// The OM-run status hook (the app's om_status emitter): each run
    /// reports its kind at start and `idle` at the end, in order.
    #[tokio::test]
    async fn om_runs_notify_the_status_hook_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path());
        let mut parent: Option<String> = None;
        for _ in 0..5 {
            let e = store
                .append(
                    "user",
                    json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                    parent.as_deref(),
                )
                .unwrap();
            parent = Some(e.id);
        }
        let cfg = crate::config::Om {
            om_model: String::new(),
            observe_threshold: 1000,
            reflect_threshold: 2000,
            buffer_increment: 230,
        };
        let obs_text = format!(
            "<observations>obs {}</observations>",
            (0..900)
                .map(|i| format!("L{:04} data", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let ref_text = format!(
            "<observations>condensed {}</observations>",
            (0..200)
                .map(|i| format!("c{:04}", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("a1", &[]),
            sse_json(&obs_text),
            sse("a2", &[]),
            sse_json(&ref_text),
        ]));
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: provider.clone(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: Some(crate::om_integration::OmState::from_config(
                &cfg,
                crate::om::OmRecord::default(),
            )),
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        let kinds = Arc::new(Mutex::new(Vec::<String>::new()));
        let k = kinds.clone();
        agent.set_om_status_hook(Some(Arc::new(move |kind: &str| {
            k.lock().unwrap().push(kind.to_owned());
        })));
        agent.send("go1", Lane::FollowUp);
        agent.send("go2", Lane::FollowUp);
        agent.process().await.unwrap();
        assert_eq!(
            *kinds.lock().unwrap(),
            vec!["observing", "idle", "reflecting", "idle"]
        );
    }

    /// A compacted-spawn-seeded record: the frozen prefix survives the
    /// child's reflect byte-identical (ADR-0004), and the reflector prompt
    /// carries the frozen-prefix marker.
    #[tokio::test]
    async fn a_compacted_seed_keeps_the_frozen_prefix_byte_identical_across_reflect() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path());
        let mut parent: Option<String> = None;
        for _ in 0..5 {
            let e = store
                .append(
                    "user",
                    json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                    parent.as_deref(),
                )
                .unwrap();
            parent = Some(e.id);
        }
        let cfg = crate::config::Om {
            om_model: String::new(),
            observe_threshold: 1000,
            reflect_threshold: 2000,
            buffer_increment: 230,
        };
        let obs_text = format!(
            "<observations>obs {}</observations>",
            (0..900)
                .map(|i| format!("L{:04} data", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let ref_text = format!(
            "<observations>condensed {}</observations>",
            (0..200)
                .map(|i| format!("c{:04}", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("a1", &[]),
            sse_json(&obs_text),
            sse("a2", &[]),
            sse_json(&ref_text),
        ]));
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: provider.clone(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: Some(crate::om_integration::OmState::from_config(
                &cfg,
                crate::om::OmRecord {
                    frozen_prefix: "FROZEN PARENT LOG".into(),
                    ..Default::default()
                },
            )),
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        agent.send("go1", Lane::FollowUp);
        agent.send("go2", Lane::FollowUp);
        agent.process().await.unwrap();

        let state = agent
            .inner
            .lock()
            .unwrap()
            .om
            .clone()
            .expect("the om state is written back");
        assert_eq!(state.record.frozen_prefix, "FROZEN PARENT LOG");
        assert_eq!(state.record.generation, 1);
        // The reflector call used the frozen variant.
        let seen = provider.seen.lock().unwrap();
        let reflect = seen
            .iter()
            .find(|(_, content)| {
                content
                    .as_deref()
                    .is_some_and(|c| c.contains("<frozen-prefix>"))
            })
            .expect("the reflector prompt carries the frozen-prefix marker");
        assert!(
            reflect.1.as_deref().unwrap().contains("FROZEN PARENT LOG"),
            "{reflect:?}"
        );
    }

    /// Spec §5.3: a session holding an active task re-injects the resume
    /// contract after its OM compacts — the second turn's system prompt
    /// carries it even though the raw window no longer does.
    #[tokio::test]
    async fn a_compaction_reinjects_the_active_task_resume_contract() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path());
        crate::task::create(
            &mut store,
            "task-1",
            "the long job",
            vec![crate::task::Step {
                text: "the step".into(),
                expected_output: "the artifact".into(),
                status: crate::task::StepStatus::Pending,
            }],
            vec![crate::task::Criterion {
                text: "the criterion".into(),
                status: crate::task::CriterionStatus::Pending,
            }],
        )
        .unwrap();
        crate::task::start(&mut store, "task-1").unwrap();
        // 1000-token observe threshold (5 x 225 tokens), like the sibling.
        let mut parent: Option<String> = None;
        for _ in 0..5 {
            let e = store
                .append(
                    "user",
                    json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                    parent.as_deref(),
                )
                .unwrap();
            parent = Some(e.id);
        }
        let cfg = crate::config::Om {
            om_model: String::new(),
            observe_threshold: 1000,
            reflect_threshold: 2000,
            buffer_increment: 230,
        };
        let obs_text = "<observations>observed work</observations>".to_owned();
        let provider = Arc::new(ScriptedProvider::new(vec![
            sse("a1", &[]),
            sse_json(&obs_text),
            sse("a2", &[]),
        ]));
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt: "be terse".into(),
            model: "test-model".into(),
            tools: tools::tool_specs(),
            cwd: dir.path().to_path_buf(),
            provider: provider.clone(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            om: Some(crate::om_integration::OmState::from_config(
                &cfg,
                crate::om::OmRecord::default(),
            )),
            om_model: String::new(),
            subagents: None,
            child: None,
        });
        agent.send("go1", Lane::FollowUp);
        agent.send("go2", Lane::FollowUp);
        agent.process().await.unwrap();
        // The active task's contract rides every assembly — turn 1 (before
        // any compaction) and turn 2 (after the observe compacted the raw
        // window, which no longer carries the task entries).
        let seen = provider.seen.lock().unwrap();
        // `seen` also holds the observer's own calls; the turn assemblies
        // are the ones built on the session's system prompt.
        let assemblies = seen
            .iter()
            .filter(|(ins, _)| ins.as_deref().is_some_and(|i| i.starts_with("be terse")))
            .count();
        assert_eq!(assemblies, 2, "two turn assemblies");
        for (ins, _) in seen
            .iter()
            .filter(|(ins, _)| ins.as_deref().is_some_and(|i| i.starts_with("be terse")))
        {
            let ins = ins.as_deref().unwrap();
            assert!(ins.contains("# Task (resume contract)"), "{ins}");
        }
    }
}
