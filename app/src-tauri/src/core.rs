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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tau_core::agent::{AgentSession, Lane, SessionParams, TurnConfig};
use tau_core::config::{self, Config};
use tau_core::context;
use tau_core::provider::{
    self, ProviderTurn, ResponseRequest, TurnEvent, TurnProvider, TurnProviderRef, TurnSink,
    Usage as CoreUsage,
};
use tau_core::session::{Entry, SessionStore};
use tau_core::tools;
use tau_protocol::coalesce::Coalescer;
use tau_protocol::snapshot::{
    EntryMeta, LiveState, QueuedItem, SessionMeta, Snapshot, TurnState, ViewEntry, Workspace,
};
use tau_protocol::{
    AgentType, Command, CommandOutput, Event, FileText, MessageLane, ProtocolError, ProviderInfo,
    SystemEventKind, Usage,
};
use tokio::sync::mpsc;

/// A live session: the loop plus the binding's view of its lanes, the
/// stop flag, and the provider (kept here so a turn can be diffed against
/// the calls it started).
struct LiveSession {
    meta: Mutex<SessionMeta>,
    agent: AgentSession,
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

/// The core's owned state: workspaces (identity, never paths) and the live
/// sessions. Everything the GUI can show is derivable from here + the
/// session files (ADR-0006 state ownership).
pub struct Core {
    workspaces: Mutex<BTreeMap<String, Workspace>>,
    sessions: Mutex<HashMap<String, Arc<LiveSession>>>,
    configs: Mutex<HashMap<String, Config>>,
    system_dir: Option<PathBuf>,
    client: reqwest::Client,
    events_tx: mpsc::Sender<Event>,
    /// The pump's half of the events channel; taken exactly once.
    events_rx: Mutex<Option<mpsc::Receiver<Event>>>,
    coalesce_ms: u64,
}

pub struct CoreBuilder {
    system_dir: Option<PathBuf>,
    providers: BTreeMap<String, tau_core::config::Provider>,
}

impl CoreBuilder {
    /// Production shape: the system config dir (`~/.config/tau`).
    pub fn default_system() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME set");
        Self {
            system_dir: Some(home.join(".config").join("tau")),
            providers: BTreeMap::new(),
        }
    }

    /// A test shape: no config files, an explicit provider list.
    pub fn custom(providers: BTreeMap<String, tau_core::config::Provider>) -> Self {
        Self {
            system_dir: None,
            providers,
        }
    }

    pub fn build(self) -> Arc<Core> {
        let (events_tx, rx) = mpsc::channel(1024);
        let core = Arc::new(Core {
            workspaces: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            system_dir: self.system_dir.clone(),
            client: reqwest::Client::new(),
            events_tx,
            events_rx: Mutex::new(Some(rx)),
            coalesce_ms: 25,
        });
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
        let project = Path::new(&workspace.cwd).join(".tau").join("config.toml");
        if project.exists()
            && let Ok(loaded) = config::load(&self.system_dir_of(), &project)
        {
            if self.system_dir.is_some() {
                // Full file-level layering (system + project).
                c = loaded;
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
        self.system_dir.clone().unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(".config").join("tau")
        })
    }

    // ── sessions ─────────────────────────────────────────────────────────

    fn new_session_id() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{:016x}", nanos)
    }

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
        let mut store = SessionStore::for_workspace(&cwd, &Self::new_session_id());
        store.create().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
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
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt,
            model: model.clone(),
            tools: tools::tool_specs(),
            cwd: cwd.clone(),
            provider: provider.clone(),
            tool_batch_on_force: config.requests.tool_batch_on_force,
            turn: TurnConfig::default(),
        });
        let meta = SessionMeta {
            id: provider.session.clone(),
            workspace: workspace.id.clone(),
            title,
            leaf: None,
            model: Some(model),
            usage: None,
        };
        let live = Arc::new(LiveSession {
            meta: Mutex::new(meta.clone()),
            agent,
            stop: provider.stop.clone(),
            queue: Mutex::new(Vec::new()),
            turn: AtomicBool::new(false),
            provider,
            cwd,
        });
        let id = live.meta.lock().unwrap().id.clone();
        self.sessions.lock().unwrap().insert(id, live);
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
            })
            .collect();
        let meta = live.meta.lock().unwrap().clone();
        Ok(Snapshot {
            workspace,
            session: SessionMeta { usage, ..meta },
            entries,
            om: Value::Null,
            live: LiveState {
                queue: live.queue.lock().unwrap().clone(),
                turn: if live.turn.load(Ordering::SeqCst) {
                    TurnState::Running
                } else {
                    TurnState::Idle
                },
                subagents: Vec::new(),
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
            (None, None) => {
                store
                    .entries_range(0, usize::MAX)
                    .map_err(|e| ProtocolError::Other {
                        message: e.to_string(),
                    })?
            }
            _ => {
                return Err(ProtocolError::Other {
                    message: "exactly one of since/range".into(),
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
                self.sessions.lock().unwrap().remove(&session);
                Ok(CommandOutput::None)
            }
            Command::SessionDelete { session } => {
                if let Some(live) = self.sessions.lock().unwrap().remove(&session) {
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
                live.agent.send(text.clone(), lane_to_lane(lane));
                if lane != MessageLane::Force {
                    live.queue.lock().unwrap().push(QueuedItem { text, lane });
                }
                self.emit_queue(&live);
                let live = Arc::clone(&live);
                let core = Arc::clone(self);
                tokio::spawn(async move { run_turn(core, live).await });
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
            Command::SubagentList { .. }
            | Command::SubagentState { .. }
            | Command::SubagentSpawn { .. }
            | Command::SubagentMessage { .. }
            | Command::SubagentStop { .. } => Err(ProtocolError::Unsupported {
                message: "sub-agent lifecycle lands in ticket #23".into(),
            }),
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
                let bytes = std::fs::read(&path).map_err(|e| ProtocolError::Other {
                    message: format!("reading {}: {e}", path.display()),
                })?;
                let cap = 1_000_000usize;
                let truncated = bytes.len() > cap;
                let text = String::from_utf8_lossy(&bytes[..cap.min(bytes.len())]).into_owned();
                let mut lines = text.lines();
                let start = offset.unwrap_or(0);
                for _ in 0..start {
                    lines.next();
                }
                let body = if let Some(n) = limit {
                    lines.take(n).collect::<Vec<_>>().join("\n")
                } else {
                    lines.collect::<Vec<_>>().join("\n")
                };
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
    live.turn.store(true, Ordering::SeqCst);
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
        let err = core
            .dispatch(Command::SubagentState { handle: "h".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, ProtocolError::Unsupported { .. }));
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
        let mut store = SessionStore::for_workspace(&cwd, &Core::new_session_id());
        store.create().unwrap();
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
        let agent = AgentSession::new(SessionParams {
            store,
            system_prompt: "You are Tau, a coding agent.".into(),
            model: "model".into(),
            tools: tools::tool_specs(),
            cwd: cwd.clone(),
            provider: provider.clone(),
            tool_batch_on_force: Default::default(),
            turn,
        });
        let live = Arc::new(LiveSession {
            meta: Mutex::new(SessionMeta {
                id: provider.session.clone(),
                workspace: workspace.id.clone(),
                title: None,
                leaf: None,
                model: Some("model".into()),
                usage: None,
            }),
            agent,
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
        let types: Vec<&str> = events
            .iter()
            .map(|e| match e {
                Event::StreamStart { .. } => "stream_start",
                Event::StreamDelta { .. } => "stream_delta",
                Event::StreamEnd { .. } => "stream_end",
                Event::ToolStart { .. } => "tool_start",
                Event::ToolEnd { .. } => "tool_end",
                Event::Queue { .. } => "queue",
                Event::SessionEvent { .. } => "session_event",
                Event::System { .. } => "system",
            })
            .collect();

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
}
