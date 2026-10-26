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
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
pub(crate) use tau_protocol::{Diagnostic, EntryKind};

/// A message lane (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Force,
    Steering,
    FollowUp,
}

/// A loop-level failure: session storage, the provider, or a prompt the
/// gate's budget math already knows is over the window (a refusal that
/// never reaches the wire).
#[derive(Debug)]
pub enum AgentError {
    Session(crate::session::Error),
    Provider(crate::provider::ProviderError),
    Om(crate::om_integration::OmError),
    /// The assembled prompt exceeds the context gate's safe budget with no
    /// unobserved material left to compact (tickets #255/#256).
    ContextOverflow {
        prompt: u32,
        budget: u32,
    },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(e) => write!(f, "session: {e}"),
            Self::Provider(e) => write!(f, "provider: {e}"),
            Self::Om(e) => write!(f, "om: {e}"),
            Self::ContextOverflow { prompt, budget } => write!(
                f,
                "context overflow: prompt ~{prompt} tokens exceeds the safe budget of {budget}; nothing left to compact"
            ),
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

/// The endpoint address for a model id: the config key the provider declares,
/// resolved from the id the session carries (§6.7 — a menu switch sends
/// `provider/key`, a launch writes the bare key, and the endpoint is addressed
/// by the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAddress {
    /// The id as the session, its records and the GUI name the model.
    pub id: String,
    /// The key the request body's `model` field carries.
    pub key: String,
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
    /// Where the endpoint is addressed, and the model id that resolution was
    /// done for (§6.7). The pair travels together so a key resolved for one
    /// model can never address another: a `.md` agent type overrides a child's
    /// model while the child inherits its parent's turn config. `None` = no
    /// config resolved this turn, so the id addresses itself.
    pub address: Option<ModelAddress>,
    /// The model's declared context window (its facts); the clamp's ceiling.
    pub context_window: Option<u32>,
    /// The `limits.image.max_bytes` cap on a `read` of an image file (#36);
    /// `None` (the default) is uncapped.
    pub image_max_bytes: Option<u64>,
    /// The `limits.image.max_pixels` decode-area budget for a `read` of an
    /// image file (#257): above it, the image is downscaled, not refused.
    /// `None` falls back to the default budget.
    pub image_max_pixels: Option<u64>,
}

/// The framing margin the context gate keeps against the model's declared
/// window (spec §4): the system prompt, the tool schemas, the request envelope.
const CONTEXT_GATE_MARGIN: u32 = 4096;

/// What the context gate knows about a turn (spec §4, ADR-0034). One predicate
/// computes it, so the gate's own budget check and every `gate_unavailable`
/// record read the same arm: a record cannot claim a gap the gate does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextGate {
    /// The safe prompt budget: the declared window less the output cap and the
    /// framing margin.
    Ready { budget: u32 },
    /// No gate at all. `reasons` is never empty: the missing facts by their
    /// config names, or `context_room` when both are declared and the
    /// subtraction leaves nothing.
    Missing { reasons: Vec<String> },
}

impl ContextGate {
    #[must_use]
    pub(crate) fn budget(&self) -> Option<u32> {
        match self {
            Self::Ready { budget } => Some(*budget),
            Self::Missing { .. } => None,
        }
    }
}

impl TurnConfig {
    /// §6.7: the model name a provider request carries. The session's id may
    /// name the provider it was picked from; the endpoint is addressed by the
    /// config key. An address applies only to the id it was resolved from, so a
    /// turn config inherited by a session on another model addresses that model
    /// itself.
    #[must_use]
    pub(crate) fn request_address(&self, model: &str) -> String {
        self.address
            .as_ref()
            .filter(|address| address.id == model)
            .map_or_else(|| model.to_owned(), |address| address.key.clone())
    }

    /// The context gate, from the model's declared facts (spec §4).
    #[must_use]
    pub(crate) fn context_gate(&self) -> ContextGate {
        match (self.context_window, self.max_output_tokens) {
            (Some(window), Some(max_output)) => u32::try_from(max_output)
                .ok()
                .and_then(|output| window.checked_sub(output))
                .and_then(|rest| rest.checked_sub(CONTEXT_GATE_MARGIN))
                .map_or_else(
                    || ContextGate::Missing {
                        reasons: vec!["context_room".to_owned()],
                    },
                    |budget| ContextGate::Ready { budget },
                ),
            (None, None) => ContextGate::Missing {
                reasons: vec!["context_window".to_owned(), "max_tokens".to_owned()],
            },
            (None, Some(_)) => ContextGate::Missing {
                reasons: vec!["context_window".to_owned()],
            },
            (Some(_), None) => ContextGate::Missing {
                reasons: vec!["max_tokens".to_owned()],
            },
        }
    }
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
    /// The entry's pre-minted id: a notified steering report mints its id
    /// at queue time (the wire upsert fires then) and the turn loop's
    /// dequeue writes the file line under it (#148) — never a queue-time
    /// write, which would land ahead of an in-flight earlier-minted entry.
    entry_id: Option<String>,
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
/// The plan-state reader (design: the plan file is the source of truth):
/// the harness wires it to the core's watcher-parsed state; it returns
/// the rendered current state, or `None` when the session has no
/// observed plan state yet.
pub(crate) type PlanStateHook = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub(crate) struct Inner {
    pub(crate) store: SessionStore,
    system_prompt: String,
    model: String,
    tools: Vec<ToolSpec>,
    cwd: PathBuf,
    tool_batch_on_force: ToolBatchPolicy,
    turn: TurnConfig,
    queue: VecDeque<Queued>,
    /// The input-item count the previous wire request carried (the #98
    /// wire log's delta view): this request's log line reports only the
    /// items appended since then, not the whole conversation. OM
    /// re-observations shrink the raw window, so the count is tracked per
    /// request, not derived from a high-water mark.
    prev_input_len: usize,
    /// The completion gate (spec §5.3); `None` = no gate configured.
    pub(crate) gates: Option<crate::config::Gates>,
    /// The live gate state: `true` when the most recent gate-qualified turn
    /// failed after exhausting `max_retries` repairs; a pass clears it
    /// (spec §5.3: the session is marked `gate_failed`).
    pub(crate) gate_failed: bool,
    pub(crate) om: Option<crate::om_integration::OmState>,
    /// The in-flight background buffer cycle (ticket #86 P2): set under
    /// the lock at spawn, so the task's first re-lock sees it; process
    /// death drops the handle and clears it (no mastra-style static
    /// registry to recover).
    pub(crate) om_inflight: Option<tokio::task::JoinHandle<()>>,
    /// In-process buffer boundary (the spawn-time tier of the two-tier
    /// boundary, ticket #86 P2); lost on restart, where the record's
    /// `last_buffered_at_tokens` takes over.
    pub(crate) om_buffer_boundary: u64,
    /// Whether this run has already recorded an `om_stalled` fact: the flag
    /// mirrors the record, so a restart re-records the stall it resumes into
    /// rather than re-scanning the log (ADR-0034's `LiveState` mirror).
    pub(crate) om_stall_recorded: bool,
    pub(crate) om_model: String,
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
    /// The plan-state reader (design: re-injected at each turn boundary
    /// and after compaction); `None` = no plan wiring (test kits, seams).
    plan_hook: Option<PlanStateHook>,
    /// The parent-side supervisor (ticket #23): present on non-child
    /// sessions only — the depth cap (a child cannot spawn) is structural.
    subagents: Option<Arc<crate::subagent::Supervisor>>,
    /// The child-side link (ticket #23): present on child sessions only;
    /// routes `parent_notify` to the child's supervisor.
    child: Option<Arc<crate::subagent::ChildLink>>,
    /// The session's anchor registry (spec §5.4, the #207 port remainder):
    /// the per-session owner of the read/edit anchor state — which anchors
    /// the session minted, and which it showed the model. In-memory for the
    /// session's lifetime; shared across tool calls via Arc.
    anchors: Arc<std::sync::Mutex<crate::hashline::registry::Registry>>,
    /// The per-session background job registry (#53); `bash background:true`
    /// records here; `mark_closed` kills every live job.
    background_jobs: Arc<crate::tools::bgjobs::BackgroundJobs>,
    /// The job-completion hook (#53): the harness wakes the session on it —
    /// the same steering lane a sub-agent's report rides.
    job_notify_hook: Option<crate::tools::bgjobs::JobCompletionHook>,
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
    /// The completion gate (spec §5.3); `None` = no gate.
    pub gates: Option<crate::config::Gates>,
    /// The gate state resumed from the session's record on re-open (spec
    /// §5.3); fresh sessions pass `false`.
    pub gate_failed: bool,
    /// The OM integration state (ticket #22); `None` = OM disabled and the
    /// loop behaves as before.
    pub om: Option<crate::om_integration::OmState>,
    /// The OM model (global config, spec §4); empty = the session's model.
    pub om_model: String,
    /// The pin store's session-start GC policy (#225, the `[hashline]`
    /// config section); defaults at the test seams.
    pub hashline: crate::hashline::pin_store::GcPolicy,
    /// The parent-side sub-agent supervisor (ticket #23); `None` for child
    /// sessions (a child cannot spawn — the depth cap is structural).
    pub subagents: Option<Arc<crate::subagent::Supervisor>>,
    /// The child-side link (ticket #23); `None` for non-child sessions.
    pub child: Option<Arc<crate::subagent::ChildLink>>,
}

pub struct AgentSession {
    inner: Arc<Mutex<Inner>>,
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
        // The registry is backed by the workspace's persistent pin store
        // (#207): a file's anchors stay stable across sessions while its
        // lines are unchanged. Built before the struct literal (which moves
        // `p.cwd`) so it borrows the cwd without cloning.
        let anchor_registry = {
            let store = crate::hashline::pin_store::PinStore::new(&p.cwd.join(".tau"), p.hashline);
            crate::hashline::registry::Registry::with_store(store)
        };
        Self {
            inner: Arc::new(Mutex::new(Inner {
                store: p.store,
                system_prompt: p.system_prompt,
                model: p.model,
                tools: p.tools,
                cwd: p.cwd,
                tool_batch_on_force: p.tool_batch_on_force,
                turn: p.turn,
                queue: VecDeque::new(),
                prev_input_len: 0,
                gates: p.gates,
                gate_failed: p.gate_failed,
                om: p.om,
                om_inflight: None,
                om_buffer_boundary: 0,
                om_stall_recorded: false,
                om_model: p.om_model,
                om_status_hook: None,
                entry_upsert_hook: None,
                queue_event_hook: None,
                plan_hook: None,
                subagents: p.subagents,
                child: p.child,
                anchors: Arc::new(std::sync::Mutex::new(anchor_registry)),
                background_jobs: Arc::new(crate::tools::bgjobs::BackgroundJobs::default()),
                job_notify_hook: None,
            })),
            provider: p.provider,
            kill: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The child-side link (a child session's `parent_notify` routing).
    pub(crate) fn child_link(&self) -> Option<Arc<crate::subagent::ChildLink>> {
        self.inner.lock().child.clone()
    }

    /// This session's supervisor (None for a child session — children
    /// cannot spawn, so the supervisor is parent-side only).
    pub(crate) fn subagents(&self) -> Option<Arc<crate::subagent::Supervisor>> {
        self.inner.lock().subagents.clone()
    }

    /// The `recall` tool (spec §4): a child's `scope: "parent"` browses
    /// the parent session's raw history (a compacted child's frozen prefix
    /// points there); everything else is this session's own log.
    pub(crate) fn recall_scoped(&self, args: &Value) -> Result<String, String> {
        if args.get("scope").and_then(Value::as_str) == Some("parent") {
            let (link, cwd) = {
                let inner = self.inner.lock();
                (inner.child.clone(), inner.cwd.clone())
            };
            match link {
                Some(link) => {
                    // The parent id comes from the link (R3): the handle
                    // is opaque, never parsed.
                    let mut store = SessionStore::for_workspace(&cwd, link.parent_session());
                    match store.open() {
                        Ok(()) => {
                            let record =
                                match crate::om_integration::OmState::load_record(&mut store) {
                                    Ok(r) => r,
                                    Err(e) => return Err(format!("recall: parent record: {e}")),
                                };
                            crate::om_integration::recall(&mut store, &record, args)
                        }
                        Err(e) => Err(format!("recall: parent session: {e}")),
                    }
                }
                None => {
                    Err("recall: scope \"parent\" needs a parent link (compacted child)".into())
                }
            }
        } else {
            let mut inner = self.inner.lock();
            let record = inner
                .om
                .as_ref()
                .map(|om| om.record.clone())
                .unwrap_or_default();
            crate::om_integration::recall(&mut inner.store, &record, args)
        }
    }

    /// The session's anchor registry handle (spec §5.4): the tool dispatch
    /// locks it per read/edit call.
    pub(crate) fn anchors(&self) -> Arc<std::sync::Mutex<crate::hashline::registry::Registry>> {
        self.inner.lock().anchors.clone()
    }

    /// The session's background job registry (#53): the tool dispatch reads
    /// it per bash call; the completion gate asks it whether a job is live.
    pub(crate) fn background_jobs(&self) -> Arc<crate::tools::bgjobs::BackgroundJobs> {
        self.inner.lock().background_jobs.clone()
    }

    /// The job-completion hook (#53): the dispatch passes it to the spawn.
    pub(crate) fn job_hook(&self) -> Option<crate::tools::bgjobs::JobCompletionHook> {
        self.inner.lock().job_notify_hook.clone()
    }
}
mod config;
mod gate_hook;
mod queue;
mod turn;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests_address;
#[cfg(test)]
mod tests_bgjobs;
#[cfg(test)]
mod tests_gate;
#[cfg(test)]
mod tests_om;
#[cfg(test)]
mod tests_overflow;
#[cfg(test)]
mod tests_queue;
#[cfg(test)]
mod tests_turns;
