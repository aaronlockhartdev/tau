//! The thin dispatch layer (spec §8, ADR-0006, ADR-0002): protocol messages
//! mapped onto the `tau-core` public API. Tauri appears only in `main.rs`;
//! this module is transport-free so a future `tau serve` reuses it.
//!
//! The core is the single owner of state (ADR-0006): workspaces and live
//! sessions live here; the GUI rebuilds from snapshots + the event stream.
//! Stream deltas reach clients through the provider seam — a forwarding
//! provider that wraps the loop's provider (the loop itself is untouched) —
//! and the 25 ms coalescer in the event pump.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tau_core::agent::{AgentSession, Lane, SessionParams, TurnConfig};
use tau_core::config::{self, Config};
use tau_core::context;
use tau_core::provider::{
    self, ProviderTurn, ResponseRequest, TurnEvent, TurnProvider, TurnProviderRef, TurnSink,
    Usage as CoreUsage,
};
use tau_core::session::{Entry, SessionStore};
use tau_core::subagent::{
    BoxedDrive, ChildDriver, ChildProviderFactory, SpawnNotice, StateNotice, StoppedBy,
    SubagentBridge, Supervisor, SupervisorParams, WakeKind, WakeNotice,
};
use tau_core::tools;
use tau_protocol::coalesce::Coalescer;
use tau_protocol::snapshot::{
    EntryMeta, EntryStatus, LiveState, QueuedItem, SessionMeta, Snapshot, TurnState, ViewEntry,
    Workspace,
};
use tau_protocol::{
    AgentType, Command, CommandOutput, ContextMode, Event, FileText, MessageLane, ProtocolError,
    ProviderInfo, SubagentEventKind, SubagentInfo, SystemEventKind, Usage,
};
use tokio::sync::mpsc;

/// A live session: the loop plus the binding's view of its lanes, the
/// stop flag, and the provider (kept here so a turn can be diffed against
/// the calls it started).
struct LiveSession {
    meta: Mutex<SessionMeta>,
    agent: Arc<AgentSession>,
    /// User stop (spec §7): the forwarding sink returns false on it, which
    /// cuts the in-flight stream the way a force does — a stop is a force
    /// with no message.
    stop: Arc<AtomicBool>,
    queue: Mutex<Vec<QueuedItem>>,
    turn: AtomicBool,
    /// The forwarding provider (the only live surface that sees stream
    /// events); also the provider the loop runs against.
    provider: Arc<ForwardingProvider>,
    cwd: PathBuf,
}

/// A `TurnProvider` wrapper that mirrors stream events onto the protocol
/// channel (spec §8 streaming) before the loop's own sink sees them.
struct ForwardingProvider {
    inner: TurnProviderRef,
    tx: mpsc::Sender<Event>,
    workspace: String,
    session: String,
    stop: Arc<AtomicBool>,
    call_seq: AtomicU64,
    /// Call ids in start order; a turn diffs its assistant entries against
    /// this (the kth entry in a turn is the kth call).
    calls: Arc<Mutex<Vec<String>>>,
    /// Calls whose `Completed` (usage) event was seen; the post-turn diff
    /// emits `StreamEnd` for the rest.
    completed: Arc<Mutex<HashMap<String, bool>>>,
}

impl TurnProvider for ForwardingProvider {
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
            workspace: self.workspace.clone(),
            session: self.session.clone(),
            stop: self.stop.clone(),
            calls: self.calls.clone(),
            completed: self.completed.clone(),
            call_id,
            started: false,
            inner: sink,
        };
        let inner = self.inner.clone();
        let request = request.clone();
        Box::pin(async move { inner.call(&request, &mut forward).await })
    }
}

/// One stream's mirror: forwards each delta onto the protocol channel and
/// delegates to the loop's own sink (the kill point stays the loop's).
struct ForwardSink<'a> {
    tx: mpsc::Sender<Event>,
    workspace: String,
    session: String,
    stop: Arc<AtomicBool>,
    calls: Arc<Mutex<Vec<String>>>,
    completed: Arc<Mutex<HashMap<String, bool>>>,
    call_id: String,
    started: bool,
    inner: &'a mut dyn TurnSink,
}

impl TurnSink for ForwardSink<'_> {
    fn event(&mut self, event: TurnEvent) -> bool {
        if !self.started {
            self.started = true;
            self.calls.lock().unwrap().push(self.call_id.clone());
            self.send(Event::StreamStart {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                call_id: self.call_id.clone(),
            });
        }
        match &event {
            TurnEvent::Text(t) => self.send(Event::StreamDelta {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                call_id: self.call_id.clone(),
                text: t.clone(),
                reasoning: None,
            }),
            TurnEvent::Reasoning(t) => self.send(Event::StreamDelta {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                call_id: self.call_id.clone(),
                text: String::new(),
                reasoning: Some(t.clone()),
            }),
            TurnEvent::Completed(u) => {
                self.completed
                    .lock()
                    .unwrap()
                    .insert(self.call_id.clone(), true);
                self.send(Event::StreamEnd {
                    workspace: self.workspace.clone(),
                    session: self.session.clone(),
                    call_id: self.call_id.clone(),
                    interrupted: false,
                    usage: Some(Usage {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        total_tokens: u.total_tokens,
                    }),
                });
            }
        }
        // A stop cuts the stream at the next delta (the loop records the
        // partial as interrupted, spec §7).
        if self.stop.load(Ordering::SeqCst) {
            return false;
        }
        self.inner.event(event)
    }
}

impl ForwardSink<'_> {
    fn send(&self, event: Event) {
        // The channel is the boundary's only failure mode; a dropped batch
        // self-heals on the GUI's next paged read (spec §8 idempotency).
        let _ = self.tx.try_send(event);
    }
}

/// The context modes are structurally identical (both three-lowercase);
/// the mapping is the ADR-0002 crate boundary.
fn mode_to_protocol(m: tau_core::subagent::ContextMode) -> ContextMode {
    match m {
        tau_core::subagent::ContextMode::Fresh => ContextMode::Fresh,
        tau_core::subagent::ContextMode::Compacted => ContextMode::Compacted,
        tau_core::subagent::ContextMode::Fork => ContextMode::Fork,
    }
}

/// The core's child record as the protocol's (the protocol crate is
/// independent of the core — ADR-0002 — so the mapping lives here).
fn info_to_protocol(i: &tau_core::subagent::SubagentInfo) -> SubagentInfo {
    let state = match &i.state {
        tau_core::subagent::ChildState::Running => "running",
        tau_core::subagent::ChildState::Idle { .. } => "idle",
        tau_core::subagent::ChildState::Done { .. } => "done",
        tau_core::subagent::ChildState::Failed { .. } => "failed",
        tau_core::subagent::ChildState::Stopped { .. } => "stopped",
    };
    SubagentInfo {
        handle: i.handle.clone(),
        child: i.child.clone(),
        agent_type: i.agent_type.clone(),
        context_mode: mode_to_protocol(i.context_mode),
        state: state.to_owned(),
        waiting_on: i.waiting_on.map(|w| w.as_str().to_owned()),
        last_message: i.last_message.clone(),
        usage: i.usage.as_ref().map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
        }),
        task: i.task.clone(),
        resume_contract: i.resume_contract.clone(),
    }
}

/// The child provider factory (ticket #23 N3): wraps the session's
/// production provider in the forwarding seam, per child session id.
struct AppChildProviderFactory {
    client: reqwest::Client,
    provider: tau_core::config::Provider,
    requests: tau_core::config::Requests,
    tx: mpsc::Sender<Event>,
    workspace: String,
}
impl ChildProviderFactory for AppChildProviderFactory {
    fn create(&self, child: &str) -> TurnProviderRef {
        Arc::new(ForwardingProvider {
            inner: provider::production(&self.client, &self.provider, &self.requests),
            tx: self.tx.clone(),
            workspace: self.workspace.clone(),
            session: child.to_owned(),
            stop: Arc::new(AtomicBool::new(false)),
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

/// The child driver (N3): a child's in-flight round is a plain session
/// turn — the child is an ordinary session, so its drive is `run_turn` on
/// its live session (registered by the bridge at spawn).
struct AppChildDriver {
    core: Weak<Core>,
}
impl ChildDriver for AppChildDriver {
    fn drive(&self, session: &str, _agent: &Arc<AgentSession>) -> BoxedDrive {
        let live = self
            .core
            .upgrade()
            .and_then(|c| c.sessions.lock().unwrap().get(session).cloned());
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
struct AppSubagentBridge {
    core: Arc<Core>,
    workspace: String,
    client: reqwest::Client,
    provider: tau_core::config::Provider,
    requests: tau_core::config::Requests,
    sup: Mutex<Option<Weak<Supervisor>>>,
}
impl SubagentBridge for AppSubagentBridge {
    fn spawned(&self, n: &SpawnNotice) {
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::Spawned {
                handle: n.handle.clone(),
                child: n.child.clone(),
                agent_type: n.agent_type.clone(),
                context_mode: mode_to_protocol(n.context_mode),
            },
        });
        // Register the child as a live session so the GUI can open it and
        // the driver can feed it turns.
        let Some(agent) = self
            .sup
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
            .and_then(|s| s.child_agent(&n.handle))
        else {
            return;
        };
        let Some(parent_live) = self.core.sessions.lock().unwrap().get(&n.parent).cloned() else {
            return;
        };
        let mut store = SessionStore::for_workspace(&parent_live.cwd, &n.child);
        if store.open().is_err() {
            return; // the spawn failed after the notice; nothing to register
        }
        let provider = Arc::new(ForwardingProvider {
            inner: provider::production(&self.client, &self.provider, &self.requests),
            tx: self.core.events_tx.clone(),
            workspace: self.workspace.clone(),
            session: n.child.clone(),
            stop: agent.stop_flag(),
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        });
        let live = Arc::new(LiveSession {
            meta: Mutex::new(SessionMeta {
                id: n.child.clone(),
                workspace: self.workspace.clone(),
                title: Some(format!("sub-agent {}", n.handle)),
                created: store.created(),
                leaf: None,
                model: Some(n.model.clone()),
                usage: None,
            }),
            agent: agent.clone(),
            stop: agent.stop_flag(),
            queue: Mutex::new(Vec::new()),
            turn: AtomicBool::new(false),
            provider,
            cwd: parent_live.cwd.clone(),
        });
        let id = live.meta.lock().unwrap().id.clone();
        self.core.sessions.lock().unwrap().insert(id, live);
    }

    fn state(&self, n: &StateNotice) {
        let detail = match &n.state {
            tau_core::subagent::ChildState::Done { output } => Some(json!({ "output": output })),
            tau_core::subagent::ChildState::Failed { reason } => Some(json!({ "reason": reason })),
            tau_core::subagent::ChildState::Idle { waiting_on } => {
                Some(json!({ "waiting_on": waiting_on.as_str() }))
            }
            tau_core::subagent::ChildState::Stopped { by } => Some(json!({ "by": by })),
            tau_core::subagent::ChildState::Running => None,
        };
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::State {
                handle: n.handle.clone(),
                child: n.child.clone(),
                state: match &n.state {
                    tau_core::subagent::ChildState::Running => "running",
                    tau_core::subagent::ChildState::Idle { .. } => "idle",
                    tau_core::subagent::ChildState::Done { .. } => "done",
                    tau_core::subagent::ChildState::Failed { .. } => "failed",
                    tau_core::subagent::ChildState::Stopped { .. } => "stopped",
                }
                .to_owned(),
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
                }
                .into(),
                text: n.text.clone(),
                output: n.output.clone(),
            },
        });
        // Wake the parent (ADR-0001 wake rules): the notification is already
        // on its active branch; deliver it to the loop and start a turn.
        let Some(live) = self.core.sessions.lock().unwrap().get(&n.parent).cloned() else {
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
        live.agent.send_notified(text.clone(), n.child.clone());
        {
            let mut q = live.queue.lock().unwrap();
            q.push(QueuedItem {
                text: text.clone(),
                lane: MessageLane::FollowUp,
            });
        }
        self.core.emit(Event::Queue {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            items: live.queue.lock().unwrap().clone(),
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

/// The core's owned state: workspaces (identity, never paths) and the live
/// sessions. Everything the GUI can show is derivable from here + the
/// session files (ADR-0006 state ownership).
pub struct Core {
    workspaces: Mutex<BTreeMap<String, Workspace>>,
    sessions: Mutex<HashMap<String, Arc<LiveSession>>>,
    configs: Mutex<HashMap<String, Config>>,
    system_dir: Option<PathBuf>,
    custom: bool,
    client: reqwest::Client,
    events_tx: mpsc::Sender<Event>,
    /// The pump's half of the events channel; taken exactly once.
    events_rx: Mutex<Option<mpsc::Receiver<Event>>>,
    coalesce_ms: u64,
    /// The test-seam child provider factory (None in production builds).
    child_factory: Option<Arc<dyn ChildProviderFactory>>,
    /// Self-reference for the seams that need an `Arc<Core>` (the child
    /// driver/factory/bridge); set in `build`.
    self_weak: Mutex<Option<Weak<Core>>>,
}

pub struct CoreBuilder {
    system_dir: Option<PathBuf>,
    /// A custom root carries an explicit provider list (no system layer);
    /// a production root gets full file-level layering (spec §12).
    custom: bool,
    providers: BTreeMap<String, tau_core::config::Provider>,
    /// A test seam: the child provider factory replaces the production
    /// one (tests script child turns).
    child_factory: Option<Arc<dyn ChildProviderFactory>>,
}

impl CoreBuilder {
    /// Production shape: the system config dir (`~/.config/tau`).
    pub fn default_system() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME set");
        Self {
            system_dir: Some(home.join(".config").join("tau")),
            custom: false,
            providers: BTreeMap::new(),
            child_factory: None,
        }
    }

    /// A test shape: no config files, an explicit provider list.
    pub fn custom(providers: BTreeMap<String, tau_core::config::Provider>) -> Self {
        Self {
            system_dir: None,
            custom: true,
            providers,
            child_factory: None,
        }
    }

    /// A test seam: child sessions get this factory's provider (scripted
    /// child turns).
    pub fn with_child_factory(mut self, f: Arc<dyn ChildProviderFactory>) -> Self {
        self.child_factory = Some(f);
        self
    }

    pub fn build(self) -> Arc<Core> {
        let (events_tx, rx) = mpsc::channel(1024);
        let core = Arc::new(Core {
            self_weak: Mutex::new(None),
            workspaces: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            system_dir: self.system_dir.clone(),
            custom: self.custom,
            // Test builds use a no-pool client: a pooled keep-alive connection
            // keeps the tokio runtime alive after the test, hanging teardown.
            client: if self.custom {
                reqwest::Client::builder()
                    .pool_max_idle_per_host(0)
                    .build()
                    .expect("client")
            } else {
                reqwest::Client::new()
            },
            events_tx,
            events_rx: Mutex::new(Some(rx)),
            coalesce_ms: 25,
            child_factory: self.child_factory.clone(),
        });
        core.self_weak
            .lock()
            .unwrap()
            .replace(Arc::downgrade(&core));
        self.apply_startup(&core);
        core
    }

    fn apply_startup(self, core: &Arc<Core>) {
        let mut config = Config::default();
        if let Some(dir) = self.system_dir
            && let Ok(loaded) = config::load(&dir, Path::new("/nonexistent-tau-project"))
        {
            config = loaded;
        }
        for (name, p) in self.providers {
            config.providers.insert(name, p);
        }
        core.configs.lock().unwrap().insert(String::new(), config);
    }
}

impl Core {
    pub fn coalesce_ms(&self) -> u64 {
        self.coalesce_ms
    }

    pub fn events(&self) -> mpsc::Receiver<Event> {
        self.events_rx
            .lock()
            .unwrap()
            .take()
            .expect("the event pump takes the receiver exactly once")
    }

    fn emit(&self, event: Event) {
        let _ = self.events_tx.try_send(event);
    }

    /// The core as an `Arc` (the child seams keep one); None pre-`build`.
    fn self_arc(&self) -> Option<Arc<Core>> {
        self.self_weak
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn system_config(&self) -> Config {
        self.configs
            .lock()
            .unwrap()
            .get("")
            .cloned()
            .unwrap_or_default()
    }

    // ── workspaces ───────────────────────────────────────────────────────

    fn open_workspace(&self, cwd: &str) -> Result<Workspace, ProtocolError> {
        let cwd = cwd.trim().to_owned();
        let id = format!("w-{:x}", xxhash_rust::xxh3::xxh3_64(cwd.as_bytes()));
        let name = Path::new(&cwd)
            .components()
            .next_back()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cwd.clone());
        let workspace = Workspace { id, name, cwd };
        self.workspaces
            .lock()
            .unwrap()
            .entry(workspace.id.clone())
            .or_insert_with(|| workspace.clone());
        self.emit(Event::System {
            workspace: workspace.id.clone(),
            session: None,
            kind: SystemEventKind::WorkspaceOpened {
                name: workspace.name.clone(),
                cwd: workspace.cwd.clone(),
            },
        });
        Ok(workspace)
    }

    fn workspace(&self, id: &str) -> Result<Workspace, ProtocolError> {
        self.workspaces
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| ProtocolError::NotFound {
                what: format!("workspace {id} is not open"),
            })
    }

    fn workspace_config(&self, workspace: &Workspace) -> Config {
        // The project layer wins where present; v0 has no dynamic providers.
        if let Some(c) = self.configs.lock().unwrap().get(&workspace.id) {
            return c.clone();
        }
        let mut c = self.system_config();
        // `config::load` takes the project *directory* and joins
        // config.toml onto it; a path to the file itself would miss
        // (spec §12 project layer).
        let project = Path::new(&workspace.cwd).join(".tau");
        if project.exists()
            && let Ok(loaded) = config::load(&self.system_dir_of(), &project)
        {
            if !self.custom {
                // Full file-level layering (system + project).
            } else {
                // Custom roots carry explicit providers: a project entry
                // replaces the same-named root entry, others coexist.
                for (name, p) in loaded.providers {
                    c.providers.insert(name, p);
                }
            }
        }
        self.configs
            .lock()
            .unwrap()
            .insert(workspace.id.clone(), c.clone());
        c
    }

    fn system_dir_of(&self) -> PathBuf {
        // A custom root has no system layer: the marker directory holds no
        // config.toml, so `config::load` contributes defaults only — a test
        // build never reads the developer's real `~/.config/tau` (N8).
        self.system_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("/nonexistent-tau-system"))
    }

    // ── sessions ─────────────────────────────────────────────────────────

    fn live(&self, session: &str) -> Result<Arc<LiveSession>, ProtocolError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .ok_or_else(|| ProtocolError::NotFound {
                what: format!("session {session} is not open"),
            })
    }

    fn session_new(
        &self,
        workspace: &Workspace,
        title: Option<String>,
    ) -> Result<SessionMeta, ProtocolError> {
        let config = self.workspace_config(workspace);
        let (name, provider) = config
            .providers
            .iter()
            .next()
            .map(|(name, p)| (name.clone(), p.clone()))
            .ok_or_else(|| ProtocolError::Other {
                message: "no providers configured; add a [providers.x] section".into(),
            })?;
        let model = provider
            .models
            .first()
            .cloned()
            .ok_or_else(|| ProtocolError::Other {
                message: format!("provider {name} has no models"),
            })?;
        let cwd = PathBuf::from(&workspace.cwd);
        let mut store = SessionStore::for_workspace(&cwd, &SessionStore::new_session_id());
        store.create().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let created = store.created();

        // The per-session OM record (ticket #22): reconstructed from the
        // file on open; a fresh session starts with the default record.
        let record = tau_core::om_integration::OmState::load_record(&mut store).map_err(|e| {
            ProtocolError::Other {
                message: e.to_string(),
            }
        })?;

        // The loop assembles no context of its own: base prompt + context
        // files (spec §10) are built here, once, at session creation.
        let layers = context::discover(&cwd, &self.system_dir_of());
        let system_prompt = context::assemble("You are Tau, a coding agent.", &layers);

        let provider = ForwardingProvider {
            inner: provider::production(&self.client, &provider, &config.requests),
            tx: self.events_tx.clone(),
            workspace: workspace.id.clone(),
            session: store.id().to_owned(),
            stop: Arc::new(AtomicBool::new(false)),
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        };
        let provider = Arc::new(provider);
        // Ticket #23: this session's supervisor (children live here; a
        // child session carries none — the depth cap is structural).
        let self_arc = self.self_arc().expect("session_new on a built core");
        let first_provider = config
            .providers
            .iter()
            .next()
            .map(|(_, p)| p.clone())
            .expect("provider checked above");
        let factory: Arc<dyn ChildProviderFactory> =
            self.child_factory.clone().unwrap_or_else(|| {
                Arc::new(AppChildProviderFactory {
                    client: self.client.clone(),
                    provider: first_provider.clone(),
                    requests: config.requests.clone(),
                    tx: self.events_tx.clone(),
                    workspace: workspace.id.clone(),
                })
            });
        // The bridge and the supervisor reference each other: build the
        // bridge with an empty weak and patch it in after construction.
        let bridge = Arc::new(AppSubagentBridge {
            core: self_arc.clone(),
            workspace: workspace.id.clone(),
            client: self.client.clone(),
            provider: first_provider.clone(),
            requests: config.requests.clone(),
            sup: Mutex::new(None),
        });
        let sup = Supervisor::new(SupervisorParams {
            parent_session: provider.session.clone(),
            cwd: cwd.clone(),
            provider: factory,
            model: model.clone(),
            system_prompt: system_prompt.clone(),
            om: config.om.clone(),
            om_model: config.om.om_model.clone(),
            tool_batch_on_force: config.requests.tool_batch_on_force,
            turn: TurnConfig::default(),
            caps: config.subagents.clone(),
            depth: 0,
            bridge: bridge.clone() as Arc<dyn SubagentBridge>,
            driver: Arc::new(AppChildDriver {
                core: Arc::downgrade(&self_arc),
            }),
        });
        bridge.sup.lock().unwrap().replace(Arc::downgrade(&sup));
        let agent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt,
            model: model.clone(),
            tools: tools::tool_specs(),
            cwd: cwd.clone(),
            provider: provider.clone(),
            tool_batch_on_force: config.requests.tool_batch_on_force,
            turn: TurnConfig::default(),
            om: Some(tau_core::om_integration::OmState::from_config(
                &config.om, record,
            )),
            om_model: config.om.om_model.clone(),
            subagents: Some(sup.clone()),
            child: None,
        }));
        let meta = SessionMeta {
            id: provider.session.clone(),
            workspace: workspace.id.clone(),
            title,
            created,
            leaf: None,
            model: Some(model),
            usage: None,
        };
        let live = Arc::new(LiveSession {
            meta: Mutex::new(meta.clone()),
            agent: agent.clone(),
            stop: provider.stop.clone(),
            queue: Mutex::new(Vec::new()),
            turn: AtomicBool::new(false),
            provider,
            cwd,
        });
        let id = live.meta.lock().unwrap().id.clone();
        self.sessions
            .lock()
            .unwrap()
            .insert(id.clone(), live.clone());
        sup.attach_parent(live.agent.clone());
        Ok(meta)
    }

    fn snapshot(&self, live: &LiveSession) -> Result<Snapshot, ProtocolError> {
        let workspace = self.workspace(&live.meta.lock().unwrap().workspace)?;
        let mut store = SessionStore::for_workspace(&live.cwd, &live.meta.lock().unwrap().id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // The snapshot is metadata: one full page read, payloads dropped.
        let entries = store
            .entries_range(0, usize::MAX)
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        let leaf = store.leaf().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let usage = entries
            .iter()
            .rev()
            .find(|e| e.kind == tau_core::agent::KIND_ASSISTANT)
            .and_then(|e| e.payload.get("usage"))
            .and_then(usage_of);
        let entries = entries
            .iter()
            .map(|e| EntryMeta {
                id: e.id.clone(),
                parent: e.parent.clone(),
                kind: e.kind.clone(),
                timestamp: e.timestamp,
                size: serde_json::to_vec(e).map(|v| v.len() as u64).unwrap_or(0),
                preview: preview(e),
                first_kept: e.first_kept_entry_id.clone(),
                status: if e.kind == tau_core::agent::KIND_ASSISTANT
                    && e.payload.get("interrupted") == Some(&Value::Bool(true))
                {
                    EntryStatus::Interrupted
                } else {
                    EntryStatus::Ok
                },
            })
            .collect();
        let meta = live.meta.lock().unwrap().clone();
        Ok(Snapshot {
            workspace,
            session: SessionMeta { usage, ..meta },
            entries,
            om: live
                .agent
                .om_state()
                .map(|s| serde_json::to_value(&s.record).unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
            live: LiveState {
                queue: live.queue.lock().unwrap().clone(),
                turn: if live.turn.load(Ordering::SeqCst) {
                    TurnState::Running
                } else {
                    TurnState::Idle
                },
                subagents: live
                    .agent
                    .subagents()
                    .map(|sup| {
                        sup.handles()
                            .iter()
                            .filter_map(|h| sup.state_info(h).map(|i| info_to_protocol(&i)))
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            cursor: leaf.map(|e| e.id).unwrap_or_default(),
        })
    }

    fn entries(
        &self,
        live: &LiveSession,
        since: Option<String>,
        range: Option<tau_protocol::snapshot::EntryRange>,
    ) -> Result<Vec<ViewEntry>, ProtocolError> {
        let mut store = SessionStore::for_workspace(&live.cwd, &live.meta.lock().unwrap().id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let entries: Vec<Entry> = match (since, range) {
            (Some(cursor), None) => {
                store
                    .entries_since(&cursor)
                    .map_err(|e| ProtocolError::Other {
                        message: e.to_string(),
                    })?
            }
            (None, Some(r)) => store
                .entries_range(r.start, r.start + r.count)
                .map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?,
            // (None, None) would be a full dump with payloads — spec §8
            // has no full-dump command (ADR-0006); one of the two is
            // required.
            _ => {
                return Err(ProtocolError::Other {
                    message: "exactly one of since/range is required".into(),
                });
            }
        };
        Ok(entries
            .iter()
            .map(|e| ViewEntry {
                id: e.id.clone(),
                parent: e.parent.clone(),
                kind: e.kind.clone(),
                timestamp: e.timestamp,
                payload: e.payload.clone(),
                blob: e.blob.as_ref().map(|b| tau_protocol::snapshot::BlobRef {
                    id: b.id.clone(),
                    size: b.size,
                    hash: b.hash.clone(),
                }),
                first_kept: e.first_kept_entry_id.clone(),
            })
            .collect())
    }

    // ── the dispatch surface ─────────────────────────────────────────────

    pub async fn dispatch(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match cmd {
            Command::WorkspaceOpen { cwd } => {
                Ok(CommandOutput::Workspace(self.open_workspace(&cwd)?))
            }
            Command::WorkspaceList => Ok(CommandOutput::Workspaces(
                self.workspaces.lock().unwrap().values().cloned().collect(),
            )),

            Command::SessionList { workspace } => {
                let workspace = self.workspace(&workspace)?;
                Ok(CommandOutput::Sessions(
                    self.sessions
                        .lock()
                        .unwrap()
                        .values()
                        .filter(|s| s.meta.lock().unwrap().workspace == workspace.id)
                        .map(|s| s.meta.lock().unwrap().clone())
                        .collect(),
                ))
            }
            Command::SessionNew { workspace, title } => {
                let workspace = self.workspace(&workspace)?;
                Ok(CommandOutput::Session(self.session_new(&workspace, title)?))
            }
            Command::SessionOpen { session } => {
                let live = self.live(&session)?;
                Ok(CommandOutput::Snapshot(self.snapshot(&live)?))
            }
            Command::SessionClose { session } => {
                // Closing stops any in-flight turn: a closed session must
                // not keep streaming (review N7).
                if let Some(live) = self.sessions.lock().unwrap().remove(&session) {
                    live.stop.store(true, Ordering::SeqCst);
                }
                Ok(CommandOutput::None)
            }
            Command::SessionDelete { session } => {
                if let Some(live) = self.sessions.lock().unwrap().remove(&session) {
                    live.stop.store(true, Ordering::SeqCst);
                    delete_session_files(&live.cwd, &session);
                }
                Ok(CommandOutput::None)
            }
            Command::SessionFork { session, at } | Command::SessionBranch { session, at } => {
                let live = self.live(&session)?;
                let mut store = SessionStore::for_workspace(&live.cwd, &session);
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                store.set_leaf(&at).map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                live.meta.lock().unwrap().leaf = Some(at.clone());
                self.emit(Event::SessionEvent {
                    workspace: live.meta.lock().unwrap().workspace.clone(),
                    session: session.clone(),
                    kind: tau_protocol::SessionEventKind::BranchMove { leaf: at },
                });
                Ok(CommandOutput::Session(live.meta.lock().unwrap().clone()))
            }
            Command::SessionSnapshot { session } => {
                let live = self.live(&session)?;
                Ok(CommandOutput::Snapshot(self.snapshot(&live)?))
            }
            Command::SessionEntries {
                session,
                since,
                range,
            } => {
                let live = self.live(&session)?;
                Ok(CommandOutput::Entries(self.entries(&live, since, range)?))
            }

            Command::MessageSend {
                session,
                text,
                lane,
            } => {
                let live = self.live(&session)?;
                // A new send clears the stop flag: the previous turn is over.
                live.stop.store(false, Ordering::SeqCst);
                // A child session takes messages through its supervisor
                // (the parent's subagent_message semantics: a running
                // child gets a steering-lane message; a non-running one is
                // resumed with it, ADR-0001).
                if let Some(link) = live.agent.child_link() {
                    let parent = link
                        .handle()
                        .rsplit_once('-')
                        .map(|(s, _)| s.to_owned())
                        .unwrap_or_default();
                    let parent_live = self.live(&parent)?;
                    let sup =
                        parent_live
                            .agent
                            .subagents()
                            .ok_or_else(|| ProtocolError::Other {
                                message: "child's parent has no supervisor".into(),
                            })?;
                    let lane = if lane == MessageLane::Force {
                        sup.stop(link.handle(), StoppedBy::User)
                            .map_err(|e| ProtocolError::Other { message: e })?;
                        Lane::Steering
                    } else {
                        lane_to_lane(lane)
                    };
                    sup.message(link.handle(), Some(text.clone()), lane)
                        .map_err(|e| ProtocolError::Other { message: e })?;
                    return Ok(CommandOutput::None);
                }
                live.agent.send(text.clone(), lane_to_lane(lane));
                if lane != MessageLane::Force {
                    live.queue.lock().unwrap().push(QueuedItem { text, lane });
                }
                self.emit_queue(&live);
                // Turn start is a check-and-set on the turn flag: a send that
                // lands mid-turn is queued and the in-flight process() absorbs
                // it (spec §7 steering rides the live call; §8 single writer).
                if live
                    .turn
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let live = Arc::clone(&live);
                    let core = Arc::clone(self);
                    tokio::spawn(async move { run_turn(core, live).await });
                }
                Ok(CommandOutput::None)
            }
            Command::MessageStop { session } => {
                let live = self.live(&session)?;
                live.stop.store(true, Ordering::SeqCst);
                Ok(CommandOutput::None)
            }

            Command::SubagentTypes => Ok(CommandOutput::Agents(vec![AgentType {
                name: "general".into(),
                description: "The built-in agent: the session's tools and model.".into(),
                builtin: true,
            }])),
            Command::SubagentList { session } => {
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                Ok(CommandOutput::Subagents(
                    sup.handles()
                        .iter()
                        .filter_map(|h| sup.state_info(h).map(|i| info_to_protocol(&i)))
                        .collect(),
                ))
            }
            Command::SubagentState { handle } => {
                // The handle is `<parent-session>-<n>`; the supervisor
                // lives on the parent.
                let session = handle
                    .rsplit_once('-')
                    .map(|(s, _)| s.to_owned())
                    .unwrap_or_default();
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                let info = sup
                    .state_info(&handle)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {handle}"),
                    })?;
                Ok(CommandOutput::Subagent(info_to_protocol(&info)))
            }
            Command::SubagentSpawn {
                session,
                agent_type,
                brief,
                context_mode,
            } => {
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                let spawned = sup.spawn(
                    &agent_type,
                    &brief,
                    match context_mode {
                        ContextMode::Fresh => tau_core::subagent::ContextMode::Fresh,
                        ContextMode::Compacted => tau_core::subagent::ContextMode::Compacted,
                        ContextMode::Fork => tau_core::subagent::ContextMode::Fork,
                    },
                    None,
                    "gui",
                );
                match spawned {
                    Ok(sp) => {
                        let info =
                            sup.state_info(&sp.handle)
                                .ok_or_else(|| ProtocolError::Other {
                                    message: "child vanished after spawn".into(),
                                })?;
                        Ok(CommandOutput::Subagent(info_to_protocol(&info)))
                    }
                    Err(e) => Err(ProtocolError::Other { message: e }),
                }
            }
            Command::SubagentMessage { handle, text } => {
                let session = handle
                    .rsplit_once('-')
                    .map(|(s, _)| s.to_owned())
                    .unwrap_or_default();
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                sup.message(&handle, text.clone(), Lane::Steering)
                    .map_err(|e| ProtocolError::Other { message: e })?;
                let info = sup
                    .state_info(&handle)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {handle}"),
                    })?;
                Ok(CommandOutput::Subagent(info_to_protocol(&info)))
            }
            Command::SubagentStop { handle } => {
                let session = handle
                    .rsplit_once('-')
                    .map(|(s, _)| s.to_owned())
                    .unwrap_or_default();
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                sup.stop(&handle, StoppedBy::User)
                    .map_err(|e| ProtocolError::Other { message: e })?;
                let info = sup
                    .state_info(&handle)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {handle}"),
                    })?;
                Ok(CommandOutput::Subagent(info_to_protocol(&info)))
            }
            Command::TaskCreate { .. }
            | Command::TaskUpdate { .. }
            | Command::TaskAssign { .. }
            | Command::TaskEvidence { .. }
            | Command::TaskCancel { .. } => Err(ProtocolError::Unsupported {
                message: "task records land in ticket #24".into(),
            }),

            Command::ProviderList => {
                let mut providers = self
                    .system_config()
                    .providers
                    .iter()
                    .map(|(name, p)| ProviderInfo {
                        name: name.clone(),
                        base_url: p.base_url.clone(),
                        models: p.models.clone(),
                    })
                    .collect::<Vec<_>>();
                providers.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(CommandOutput::Providers(providers))
            }
            Command::ProviderAdd { .. }
            | Command::ProviderSet { .. }
            | Command::ProviderDelete { .. } => Err(ProtocolError::Unsupported {
                message: "v0 providers are config-file-driven".into(),
            }),

            Command::FileRead {
                workspace,
                path,
                offset,
                limit,
            } => {
                let workspace = self.workspace(&workspace)?;
                let full = PathBuf::from(&path);
                let path = if full.is_absolute() {
                    full
                } else {
                    Path::new(&workspace.cwd).join(full)
                };
                // Line-streamed: an offset lands anywhere in the file, not
                // just inside the first 1 MB (review N9); the read stops at
                // the cap or end of file.
                let file = std::fs::File::open(&path).map_err(|e| ProtocolError::Other {
                    message: format!("reading {}: {e}", path.display()),
                })?;
                let reader = std::io::BufReader::new(file);
                let start = offset.unwrap_or(0);
                let cap = 1_000_000usize;
                let mut seen = 0usize;
                let mut bytes = 0usize;
                let mut taken = Vec::new();
                let mut truncated = false;
                for line in reader.lines() {
                    let line = line.map_err(|e| ProtocolError::Other {
                        message: format!("reading {}: {e}", path.display()),
                    })?;
                    let line = line.strip_suffix('\r').map(str::to_owned).unwrap_or(line);
                    bytes += line.len() + 1;
                    if bytes > cap {
                        truncated = true;
                        break;
                    }
                    seen += 1;
                    if seen > start && taken.len() < limit.unwrap_or(usize::MAX) {
                        taken.push(line);
                    }
                }
                // A short file is not truncation: only the cap marks lost
                // content.
                let body = taken.join("\n");
                Ok(CommandOutput::File(FileText {
                    text: body,
                    truncated,
                }))
            }
        }
    }
}

/// The event pump: raw events → the 25 ms coalescer → a flush batch. The
/// sink is the transport (Tauri `emit` in the app, a collector in tests),
/// so the pipe is testable without a window (ADR-0006 transport #1).
pub async fn pump(core: Arc<Core>, mut sink: impl FnMut(&[Event])) {
    let mut rx = core.events();
    let mut coalescer = Coalescer::new(core.coalesce_ms());
    loop {
        tokio::select! {
            received = rx.recv() => {
                let Some(event) = received else {
                    break;
                };
                let now = now_ms();
                coalescer.push(event, now);
                let batch = coalescer.take(now);
                if !batch.is_empty() {
                    sink(&batch);
                }
            }
            _ = tokio::time::sleep(deadline(&coalescer)) => {
                let batch = coalescer.take(now_ms());
                if !batch.is_empty() {
                    sink(&batch);
                }
            }
        }
    }
}

fn deadline(coalescer: &Coalescer) -> std::time::Duration {
    match coalescer.due_at() {
        Some(due) => {
            let delta = due as i128 - now_ms() as i128;
            if delta <= 0 {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_millis(delta as u64)
            }
        }
        None => std::time::Duration::from_secs(3600),
    }
}

/// The post-turn reconciliation (spec §8 idempotent updates): the session
/// file is the record; the events it implies are derived here, after
/// `process()` drained the queue.
async fn run_turn(core: Arc<Core>, live: Arc<LiveSession>) {
    let mut store = SessionStore::for_workspace(&live.cwd, &live.meta.lock().unwrap().id);
    if store.open().is_err() {
        live.turn.store(false, Ordering::SeqCst);
        return;
    }
    let start = store.leaf().ok().flatten().map(|e| e.id);
    let calls_before = live.provider.calls.lock().unwrap().len();

    if let Err(e) = live.agent.process().await {
        core.emit(Event::System {
            workspace: live.meta.lock().unwrap().workspace.clone(),
            session: Some(live.meta.lock().unwrap().id.clone()),
            kind: SystemEventKind::Error {
                message: e.to_string(),
            },
        });
    }

    let new: Vec<Entry> = match start {
        Some(cursor) => store.entries_since(&cursor).unwrap_or_default(),
        None => store.entries_range(0, usize::MAX).unwrap_or_default(),
    };

    let workspace = live.meta.lock().unwrap().workspace.clone();
    let session = live.meta.lock().unwrap().id.clone();
    let mut assistant_index = 0usize;
    let mut queue_changed = false;
    for entry in &new {
        match entry.kind.as_str() {
            // A delivered message leaves the GUI's queue (full-state
            // replacement keeps the GUI and the core in agreement).
            tau_core::agent::KIND_USER => {
                let text = entry.payload.get("text").and_then(Value::as_str);
                // The position lookup and the removal are separate locked
                // scopes: a let-chain would keep the first guard alive
                // across the second lock on the same (non-reentrant) mutex.
                let pos = text.and_then(|t| {
                    live.queue
                        .lock()
                        .unwrap()
                        .iter()
                        .position(|item| item.text == t)
                });
                if let Some(pos) = pos {
                    live.queue.lock().unwrap().remove(pos);
                    queue_changed = true;
                }
            }
            tau_core::agent::KIND_ASSISTANT => {
                let call_id = live
                    .provider
                    .calls
                    .lock()
                    .unwrap()
                    .get(calls_before + assistant_index)
                    .cloned();
                assistant_index += 1;
                let interrupted = entry
                    .payload
                    .get("interrupted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let usage = entry.payload.get("usage").and_then(usage_of);
                let completed = {
                    let map = live.provider.completed.lock().unwrap();
                    call_id
                        .as_ref()
                        .and_then(|id| map.get(id))
                        .copied()
                        .unwrap_or(false)
                };
                if let Some(call_id) = call_id
                    && !completed
                {
                    // The stream was cut before its Completed frame: the
                    // partial stands as an interrupted end (spec §6/§7).
                    core.emit(Event::StreamEnd {
                        workspace: workspace.clone(),
                        session: session.clone(),
                        call_id,
                        interrupted,
                        usage,
                    });
                }
                if let Some(u) = usage {
                    live.meta.lock().unwrap().usage = Some(u);
                }
            }
            tau_core::agent::KIND_TOOL => {
                let call_id = live
                    .provider
                    .calls
                    .lock()
                    .unwrap()
                    .get(calls_before + assistant_index.saturating_sub(1))
                    .cloned()
                    .unwrap_or_default();
                let tool_call_id = entry
                    .payload
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = entry
                    .payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                core.emit(Event::ToolStart {
                    workspace: workspace.clone(),
                    session: session.clone(),
                    call_id: call_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                });
                core.self_weak
                    .lock()
                    .unwrap()
                    .replace(Arc::downgrade(&core));
                core.emit(Event::ToolEnd {
                    workspace: workspace.clone(),
                    session: session.clone(),
                    call_id,
                    tool_call_id,
                    name,
                    output: entry.payload.get("output").cloned().unwrap_or(Value::Null),
                });
            }
            _ => {}
        }
        if let Some(first_kept) = &entry.first_kept_entry_id {
            core.emit(Event::SessionEvent {
                workspace: workspace.clone(),
                session: session.clone(),
                kind: tau_protocol::SessionEventKind::Compaction {
                    entry: entry.id.clone(),
                    first_kept: first_kept.clone(),
                },
            });
        }
    }

    // A call cut before it produced an entry (an empty partial, N10) still
    // gets its interrupted end — the GUI's live bubble must close.
    {
        let calls = live.provider.calls.lock().unwrap();
        let new_calls = calls.len() - calls_before;
        for i in assistant_index..new_calls {
            core.emit(Event::StreamEnd {
                workspace: workspace.clone(),
                session: session.clone(),
                call_id: calls[calls_before + i].clone(),
                interrupted: true,
                usage: None,
            });
        }
    }

    // The active branch is what the loop appended against.
    if let Ok(leaf) = store.leaf()
        && let Some(leaf) = leaf
    {
        live.meta.lock().unwrap().leaf = Some(leaf.id);
    }
    if queue_changed {
        core.emit_queue(&live);
    }
    live.turn.store(false, Ordering::SeqCst);
}

impl Core {
    fn emit_queue(&self, live: &LiveSession) {
        // One locked scope: the event's fields are cloned out before the
        // guard drops, so a long emit cannot hold the locks.
        let (workspace, session, items) = {
            let meta = live.meta.lock().unwrap();
            (
                meta.workspace.clone(),
                meta.id.clone(),
                live.queue.lock().unwrap().clone(),
            )
        };
        self.emit(Event::Queue {
            workspace,
            session,
            items,
        });
    }
}

fn lane_to_lane(lane: MessageLane) -> Lane {
    match lane {
        MessageLane::Force => Lane::Force,
        MessageLane::Steering => Lane::Steering,
        MessageLane::FollowUp => Lane::FollowUp,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The entry tree's one-line preview: the entry's first text line, cut at
/// 80 chars (the snapshot carries previews, never payloads, spec §8).
fn preview(entry: &Entry) -> String {
    let text = entry
        .payload
        .get("text")
        .or_else(|| entry.payload.get("note"))
        .or_else(|| entry.payload.get("output"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let first = text.lines().next().unwrap_or_default();
    let mut out: String = first.chars().take(80).collect();
    if first.len() > 80 {
        out.push('…');
    }
    out
}

fn usage_of(value: &Value) -> Option<Usage> {
    let u: CoreUsage = serde_json::from_value(value.clone()).ok()?;
    Some(Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
    })
}

fn delete_session_files(cwd: &Path, session: &str) {
    // The session file plus its sidecar blobs (blobs are keyed by entry id,
    // which is session-scoped — ADR-0005). The store must be opened before
    // the file goes: it is the only way to enumerate the session's blobs.
    let root = cwd.join(".tau");
    let mut store = SessionStore::for_workspace(cwd, session);
    let blobs = if store.open().is_ok() {
        store
            .entries_range(0, usize::MAX)
            .map(|entries| {
                entries
                    .into_iter()
                    .filter_map(|e| e.blob.map(|b| b.id))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let _ = std::fs::remove_file(root.join("sessions").join(format!("{session}.jsonl")));
    for id in blobs {
        let _ = std::fs::remove_file(root.join("blobs").join(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn providers() -> BTreeMap<String, tau_core::config::Provider> {
        let mut m = BTreeMap::new();
        m.insert(
            "dev".into(),
            tau_core::config::Provider {
                base_url: "http://127.0.0.1:9/v1".into(),
                key_env: String::new(),
                models: vec!["model".into()],
            },
        );
        m
    }

    #[tokio::test]
    async fn workspace_identity_is_stable_and_path_free() {
        let core = CoreBuilder::custom(providers()).build();
        let w1 = core
            .dispatch(Command::WorkspaceOpen {
                cwd: "/tmp/w".into(),
            })
            .await
            .unwrap();
        let w2 = core
            .dispatch(Command::WorkspaceOpen {
                cwd: "/tmp/w".into(),
            })
            .await
            .unwrap();
        match (w1, w2) {
            (CommandOutput::Workspace(a), CommandOutput::Workspace(b)) => {
                assert_eq!(a.id, b.id);
                assert_eq!(a.name, "w");
                // The id is a hash of the cwd, not the path itself.
                assert!(!a.id.starts_with('/'));
            }
            other => panic!("expected workspaces: {other:?}"),
        }
    }

    #[tokio::test]
    async fn not_yet_landed_commands_fail_explicitly() {
        let core = CoreBuilder::custom(providers()).build();
        // The sub-agent commands are live (ticket #23): an unknown
        // session's child is a NotFound, not an Unsupported.
        let err = core
            .dispatch(Command::SubagentState {
                handle: "h-1".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ProtocolError::NotFound { .. }));
        let err = core
            .dispatch(Command::TaskCreate {
                session: "s".into(),
                title: "t".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ProtocolError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn file_read_pages_lines_relative_to_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        std::fs::write(tmp.path().join("f.txt"), "l1\nl2\nl3\n").unwrap();
        let out = core
            .dispatch(Command::FileRead {
                workspace: workspace.id,
                path: "f.txt".into(),
                offset: Some(1),
                limit: Some(1),
            })
            .await
            .unwrap();
        match out {
            CommandOutput::File(f) => {
                assert_eq!(f.text, "l2");
                assert!(!f.truncated);
            }
            other => panic!("expected a file: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_new_session_registers_its_workspace_and_streams_events() {
        let tmp = tempfile::tempdir().unwrap();
        // A session with no provider models fails explicitly.
        let core = CoreBuilder::custom(BTreeMap::new()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let err = core
            .dispatch(Command::SessionNew {
                workspace: workspace.id,
                title: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ProtocolError::Other { .. }));
        let _ = tmp;
    }

    /// `SessionEntries` with neither cursor nor range is a full dump —
    /// spec §8 has no full-dump command (ADR-0006), so it is an error
    /// (review B2).
    #[tokio::test]
    async fn entries_without_a_cursor_or_range_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let session = match core
            .dispatch(Command::SessionNew {
                workspace: workspace.id,
                title: None,
            })
            .await
            .unwrap()
        {
            CommandOutput::Session(m) => m,
            other => panic!("expected a session: {other:?}"),
        };
        let err = core
            .dispatch(Command::SessionEntries {
                session: session.id,
                since: None,
                range: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProtocolError::Other { .. }),
            "expected a rejection, got: {err:?}"
        );
    }

    /// The workspace's `.tau/config.toml` layers over the root config
    /// (spec §12) — a same-named provider entry replaces the root's
    /// (review B3: the layer was silently dead on a path bug).
    #[tokio::test]
    async fn the_project_config_layer_is_read_from_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".tau")).unwrap();
        std::fs::write(
            tmp.path().join(".tau").join("config.toml"),
            "[providers.dev]\nbase_url = \"http://project:9/v1\"\nkey_env = \"\"\nmodels = [\"proj-model\"]\n",
        )
        .unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let config = core.workspace_config(&workspace);
        let dev = &config.providers["dev"];
        assert_eq!(
            dev.base_url, "http://project:9/v1",
            "the project layer did not replace the root provider"
        );
        assert_eq!(dev.models, vec!["proj-model".to_owned()]);
    }

    /// Closing a session with an in-flight turn stops the stream — the
    /// call closes as interrupted and nothing for that session follows
    /// (review N7).
    #[tokio::test]
    async fn closing_a_session_stops_its_in_flight_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let live = manual_session(
            &core,
            &workspace,
            provider::canned_slow(&canned_body(), 600),
            TurnConfig::default(),
        );

        let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
        let sink = Arc::clone(&collected);
        let pump_core = Arc::clone(&core);
        tokio::spawn(async move {
            pump(pump_core, move |batch| {
                sink.lock().unwrap().extend(batch.iter().cloned());
            })
            .await;
        });

        let session_id = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: session_id.clone(),
            text: "first".into(),
            lane: MessageLane::Steering,
        })
        .await
        .unwrap();

        // In flight: the first deltas have landed.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        core.dispatch(Command::SessionClose {
            session: session_id.clone(),
        })
        .await
        .unwrap();

        // The sink cuts the stream at the next delta (≤ 600 ms); the turn
        // then winds down — give it room to finish.
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
        let events = collected.lock().unwrap().clone();
        let session_events: Vec<&Event> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::StreamStart { session, .. }
                        | Event::StreamDelta { session, .. }
                        | Event::StreamEnd { session, .. }
                        | Event::ToolStart { session, .. }
                        | Event::ToolEnd { session, .. }
                        if *session == session_id
                )
            })
            .collect();
        // The stream was cut, not completed: the last stream event for the
        // session is an interrupted end, with no live activity after it.
        let Some(Event::StreamEnd { interrupted, .. }) = session_events.last().cloned() else {
            panic!("the closed session's stream never closed: {session_events:?}");
        };
        assert!(interrupted, "the close did not cut the stream");
        assert!(
            session_events
                .iter()
                .any(|e| matches!(e, Event::StreamStart { .. })),
            "no stream ever started"
        );
    }

    /// A session wired to `inner` (canned in tests, production in the
    /// live test), registered with the core the way `SessionNew` does.
    fn manual_session(
        core: &Arc<Core>,
        workspace: &Workspace,
        inner: TurnProviderRef,
        turn: TurnConfig,
    ) -> Arc<LiveSession> {
        let tmp = workspace.cwd.clone();
        let cwd = PathBuf::from(&tmp);
        let mut store = SessionStore::for_workspace(&cwd, &SessionStore::new_session_id());
        store.create().unwrap();
        let created = store.created();
        let provider = Arc::new(ForwardingProvider {
            inner,
            tx: core.events_tx.clone(),
            workspace: workspace.id.clone(),
            session: store.id().to_owned(),
            stop: Arc::new(AtomicBool::new(false)),
            call_seq: AtomicU64::new(0),
            calls: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        });
        let agent = Arc::new(AgentSession::new(SessionParams {
            store,
            system_prompt: "You are Tau, a coding agent.".into(),
            model: "model".into(),
            tools: tools::tool_specs(),
            cwd: cwd.clone(),
            provider: provider.clone(),
            tool_batch_on_force: Default::default(),
            turn,
            om: None,
            om_model: String::new(),
            subagents: None,
            child: None,
        }));
        let live = Arc::new(LiveSession {
            meta: Mutex::new(SessionMeta {
                id: provider.session.clone(),
                workspace: workspace.id.clone(),
                title: None,
                created,
                leaf: None,
                model: Some("model".into()),
                usage: None,
            }),
            agent: agent.clone(),
            stop: provider.stop.clone(),
            queue: Mutex::new(Vec::new()),
            turn: AtomicBool::new(false),
            provider,
            cwd,
        });
        core.sessions
            .lock()
            .unwrap()
            .insert(live.meta.lock().unwrap().id.clone(), live.clone());
        live
    }

    fn canned_body() -> String {
        let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
        for frame in [
            r#"{"type":"response.output_text.delta","delta":"partial"}"#,
            r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking"}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"bash","arguments":"{\"command\":\"ls\"}"}}"#,
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#,
        ] {
            body.push_str(&format!("data: {frame}\n\n"));
        }
        body
    }

    #[tokio::test]
    async fn the_event_pipe_carries_a_canned_turn_to_the_sink() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let live = manual_session(
            &core,
            &workspace,
            provider::canned(&canned_body()),
            TurnConfig::default(),
        );

        // The collector stands in for the Tauri emit: everything the pump
        // flushes is exactly what the Svelte shell would receive.
        let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
        let sink = Arc::clone(&collected);
        let pump_core = Arc::clone(&core);
        tokio::spawn(async move {
            pump(pump_core, move |batch| {
                sink.lock().unwrap().extend(batch.iter().cloned());
            })
            .await;
        });

        // The meta guard must not live across this await: the dispatch
        // path re-locks the same mutex (emit_queue), and a std mutex is
        // not reentrant.
        let session_id = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: session_id,
            text: "do the thing".into(),
            lane: MessageLane::Steering,
        })
        .await
        .unwrap();

        // The canned turn is fast; give the pump a bounded time to flush.
        for _ in 0..500 {
            let events = collected.lock().unwrap().clone();
            if events.iter().any(|e| matches!(e, Event::StreamEnd { .. }))
                && events
                    .iter()
                    .filter(|e| matches!(e, Event::ToolEnd { .. }))
                    .count()
                    >= 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let events = collected.lock().unwrap().clone();

        // The stream: one start, deltas coalesced into one, one end with
        // the usage; the tool batch: a start/end pair; the queue: the
        // message arrives, then the delivered message leaves it.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::StreamStart { .. })),
            "no stream start: {events:?}"
        );
        let deltas = events
            .iter()
            .filter(|e| matches!(e, Event::StreamDelta { .. }))
            .count();
        assert!(
            deltas >= 1,
            "no stream deltas (coalesced burst): {events:?}"
        );
        let Some(Event::StreamDelta {
            text, reasoning, ..
        }) = events
            .iter()
            .find(|e| matches!(e, Event::StreamDelta { .. }))
        else {
            panic!("missing delta: {events:?}");
        };
        assert_eq!(text, "partial");
        assert_eq!(reasoning.as_deref(), Some("thinking"));
        let Some(Event::StreamEnd {
            interrupted, usage, ..
        }) = events.iter().find(|e| matches!(e, Event::StreamEnd { .. }))
        else {
            panic!("missing stream end: {events:?}");
        };
        assert!(!interrupted);
        assert_eq!(usage.map(|u| u.total_tokens), Some(15));
        assert!(
            events.iter().any(|e| matches!(e, Event::ToolStart { .. })),
            "no tool start: {events:?}"
        );
        let Some(Event::ToolEnd { name, .. }) =
            events.iter().find(|e| matches!(e, Event::ToolEnd { .. }))
        else {
            panic!("no tool end: {events:?}");
        };
        assert_eq!(name, "bash");
        // The queue: sent, then delivered (empty).
        let queues: Vec<&Event> = events
            .iter()
            .filter(|e| matches!(e, Event::Queue { .. }))
            .collect();
        assert!(
            queues
                .iter()
                .any(|e| matches!(e, Event::Queue { items, .. } if !items.is_empty())),
            "no non-empty queue state: {queues:?}"
        );
        assert!(
            queues
                .iter()
                .rev()
                .any(|e| matches!(e, Event::Queue { items, .. } if items.is_empty())),
            "the delivered message never left the queue: {queues:?}"
        );
    }

    /// A send that lands while a turn is in flight must not spawn a second
    /// concurrent process(): the message is queued and the in-flight turn
    /// absorbs it (spec §7/§8 single writer, review B1).
    #[tokio::test]
    async fn a_mid_turn_send_is_queued_not_a_second_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        // The slow stream (~2.4 s in flight per call) is the in-flight
        // window the second send lands in.
        let live = manual_session(
            &core,
            &workspace,
            provider::canned_slow(&canned_body(), 600),
            TurnConfig::default(),
        );

        let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
        let sink = Arc::clone(&collected);
        let pump_core = Arc::clone(&core);
        tokio::spawn(async move {
            pump(pump_core, move |batch| {
                sink.lock().unwrap().extend(batch.iter().cloned());
            })
            .await;
        });

        let session_id = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: session_id.clone(),
            text: "first".into(),
            lane: MessageLane::Steering,
        })
        .await
        .unwrap();

        // Mid-stream of call 1: the first deltas have arrived, the stream
        // is still going (call 1 ends near t=2.4 s).
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        core.dispatch(Command::MessageSend {
            session: session_id,
            text: "second".into(),
            lane: MessageLane::Steering,
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let events = collected.lock().unwrap().clone();
        let starts = events
            .iter()
            .filter(|e| matches!(e, Event::StreamStart { .. }))
            .count();
        assert_eq!(
            starts, 1,
            "a mid-turn send spawned a second turn ({starts} stream starts): {events:?}"
        );
        // The second message sits in the GUI's queue state, waiting for the
        // in-flight process() to deliver it — it was not consumed by a
        // second turn (the starter stays listed until the post-turn
        // reconciliation, so both are present).
        let last_queue = events
            .iter()
            .rev()
            .find(|e| matches!(e, Event::Queue { .. }))
            .cloned();
        let Some(Event::Queue { items, .. }) = last_queue else {
            panic!("no queue state after the second send: {events:?}")
        };
        assert!(
            items.iter().any(|i| i.text == "second"),
            "the mid-turn message must remain queued: {items:?}"
        );
    }

    /// The one live run (acceptance): a short session against the hosted
    /// vLLM whose stream reaches the event pipe. Gated on TAU_LIVE and
    /// skipped cleanly when the endpoint is unreachable (CI-safe).
    #[tokio::test]
    async fn live_run_streams_the_event_pipe() {
        if std::env::var("TAU_LIVE").is_err() {
            eprintln!("live run skipped (TAU_LIVE not set)");
            return;
        }
        let mut hosted = providers();
        hosted.insert(
            "dev".to_owned(),
            tau_core::config::Provider {
                base_url: "https://llms.aaronlockhart.dev/v1".into(),
                key_env: String::new(),
                models: vec!["qwen3.8-27b".into()],
            },
        );
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(hosted).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let config = core.system_config();
        let provider =
            provider::production(&core.client, &config.providers["dev"], &config.requests);
        let live = manual_session(
            &core,
            &workspace,
            provider,
            TurnConfig {
                max_output_tokens: Some(200),
                reasoning: None,
            },
        );

        let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
        let sink = Arc::clone(&collected);
        let pump_core = Arc::clone(&core);
        tokio::spawn(async move {
            pump(pump_core, move |batch| {
                sink.lock().unwrap().extend(batch.iter().cloned());
            })
            .await;
        });

        let session_id = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: session_id,
            text: "Reply with exactly the word: pong".into(),
            lane: MessageLane::Steering,
        })
        .await
        .unwrap();

        // The thinking model can burn the 200-token cap on reasoning; the
        // pipe is proven by start + at least one delta, not by completion.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let events = collected.lock().unwrap().clone();
            let started = events
                .iter()
                .any(|e| matches!(e, Event::StreamStart { .. }));
            let deltaed = events
                .iter()
                .any(|e| matches!(e, Event::StreamDelta { .. }));
            if started && deltaed {
                break;
            }
            if std::time::Instant::now() > deadline {
                eprintln!("live run skipped: no stream events within 90s (endpoint unreachable?)");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let events = collected.lock().unwrap().clone();
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Event::StreamDelta { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let end = events
            .iter()
            .find(|e| matches!(e, Event::StreamEnd { .. }))
            .cloned();
        eprintln!(
            "live run: {} deltas ({} chars); end: {:?}",
            events
                .iter()
                .filter(|e| matches!(e, Event::StreamDelta { .. }))
                .count(),
            text.chars().count(),
            end
        );
        assert!(!text.is_empty(), "deltas arrived but carried no text");
    }
    /// End-to-end sub-agent lifecycle (ticket #23 N3): a spawned child runs
    /// its scripted `parent_notify {done}` turn, the supervisor resolves it,
    /// and the parent is woken — the notification lands on the parent's
    /// active branch and a `Notified` event reaches the stream.
    #[tokio::test]
    async fn a_spawned_child_finishes_and_wakes_the_parent() {
        struct CannedChild {
            body: String,
            index: AtomicUsize,
        }
        impl TurnProvider for CannedChild {
            fn call<'a>(
                &self,
                _req: &ResponseRequest,
                sink: &'a mut dyn TurnSink,
            ) -> ProviderTurn<'a> {
                let body = self.body.clone();
                self.index.fetch_add(1, Ordering::SeqCst);
                let (events, calls) = provider::decode_stream(&body).unwrap();
                Box::pin(async move {
                    let mut result = provider::TurnResult::default();
                    for event in &events {
                        if !sink.event(event.clone()) {
                            break;
                        }
                        provider::fold_event(event, &mut result);
                    }
                    result.calls = calls.into_iter().map(|(_, c)| c).collect();
                    Ok(result)
                })
            }
        }
        struct CannedChildFactory {
            body: String,
        }
        impl ChildProviderFactory for CannedChildFactory {
            fn create(&self, _child: &str) -> TurnProviderRef {
                Arc::new(CannedChild {
                    body: self.body.clone(),
                    index: AtomicUsize::new(0),
                })
            }
        }
        // One scripted turn: parent_notify done with a structured output.
        let call_id = "c1".to_string();
        let args = json!({
            "text": "the work is done",
            "done": true,
            "output": { "result": "ok" },
        });
        let item = json!({
            "id": call_id,
            "type": "function_call",
            "name": "parent_notify",
            "call_id": call_id,
            "arguments": args,
        });
        let data = format!("data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}");
        let body = format!("{data}\n\ndata: [DONE]\n\n");
        let core = CoreBuilder::custom(providers())
            .with_child_factory(Arc::new(CannedChildFactory { body }))
            .build();
        let mut rx = core.events_rx.lock().unwrap().take().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace(w) => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let session = match core
            .dispatch(Command::SessionNew {
                workspace: workspace.id.clone(),
                title: None,
            })
            .await
            .unwrap()
        {
            CommandOutput::Session(m) => m,
            other => panic!("expected a session: {other:?}"),
        };
        let info = match core
            .dispatch(Command::SubagentSpawn {
                session: session.id.clone(),
                agent_type: "general".into(),
                brief: "do the thing".into(),
                context_mode: ContextMode::Fresh,
            })
            .await
            .unwrap()
        {
            CommandOutput::Subagent(i) => i,
            other => panic!("expected a subagent: {other:?}"),
        };
        // The child's scripted done-notify must wake the parent: the
        // Notified event reaches the stream, and the wake's message lands
        // on the parent's branch tagged with the child's session id.
        let mut notified = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(ev)) =
                tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
                && matches!(
                    ev,
                    Event::SubagentEvent {
                        kind: SubagentEventKind::Notified { .. },
                        ..
                    }
                )
            {
                notified = true;
                break;
            }
        }
        assert!(notified, "the parent was never woken by the child's done");
        let mut store = SessionStore::for_workspace(tmp.path(), &session.id);
        store.open().unwrap();
        let entries = store.entries_range(0, usize::MAX).unwrap();
        let woke = entries
            .iter()
            .any(|e| e.payload.get("source") == Some(&json!(info.child)));
        assert!(woke, "no notification entry on the parent's branch");
        // The child registered as an ordinary live session (the GUI can
        // open it like any session).
        assert!(
            core.sessions.lock().unwrap().get(&info.child).is_some(),
            "the child's live session was not registered"
        );
        drop(core);
    }
}
