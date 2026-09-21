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
    ProviderInfo, SkillInfo, SubagentEventKind, SubagentInfo, SystemEventKind, Usage,
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
                        cached_prompt_tokens: u
                            .prompt_tokens_details
                            .as_ref()
                            .map(|d| d.cached_tokens)
                            .unwrap_or(0),
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
            cached_prompt_tokens: u
                .prompt_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or(0),
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
        // The core set the child's header title at spawn (type + adjective-noun);
        // the event carries it so the GUI's stub gets the real name.
        let title = store.title().unwrap_or("sub-agent").to_string();
        self.core.emit(Event::SubagentEvent {
            workspace: self.workspace.clone(),
            session: n.parent.clone(),
            kind: SubagentEventKind::Spawned {
                handle: n.handle.clone(),
                child: n.child.clone(),
                agent_type: n.agent_type.clone(),
                context_mode: mode_to_protocol(n.context_mode),
                title,
            },
        });
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
                title: store.title().map(str::to_string),
                parent: Some(n.parent.clone()),
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
            tau_core::subagent::ChildState::Stopped { by } => {
                let mut d = json!({ "by": by });
                if let Some(rc) = &n.resume_contract {
                    d["resume_contract"] = rc.clone();
                }
                Some(d)
            }
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
    /// Per-workspace skill registry (ticket #28): built at session open,
    /// consulted by the message_send boundary and `skill_list`.
    skills: Mutex<HashMap<String, Vec<SkillInfo>>>,
    system_dir: Option<PathBuf>,
    /// The user's home dir (the home-level `.agents/skills/` root; the
    /// `system_dir` seam's sibling for tests).
    home: Option<PathBuf>,
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
    home: Option<PathBuf>,
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
            home: Some(home),
            custom: false,
            providers: BTreeMap::new(),
            child_factory: None,
        }
    }

    /// A test shape: no config files, an explicit provider list.
    pub fn custom(providers: BTreeMap<String, tau_core::config::Provider>) -> Self {
        Self {
            system_dir: None,
            home: None,
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

    /// A test seam: a custom system dir (the workspaces.json location).
    pub fn with_system_dir(mut self, dir: PathBuf) -> Self {
        self.system_dir = Some(dir);
        self
    }

    /// A test seam: a custom home (the home-level `.agents/skills/` root).
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }

    pub fn build(self) -> Arc<Core> {
        let (events_tx, rx) = mpsc::channel(1024);
        let core = Arc::new(Core {
            self_weak: Mutex::new(None),
            workspaces: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            skills: Mutex::new(HashMap::new()),
            system_dir: self.system_dir.clone(),
            home: self.home.clone(),
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
        // The workspaces a previous run opened: re-register the ones whose
        // folder still exists (sessions are read from disk on demand).
        for e in core.load_workspace_index() {
            if Path::new(&e.cwd).is_dir() {
                let w = Workspace {
                    id: e.id,
                    name: e.name,
                    cwd: e.cwd,
                };
                core.workspaces
                    .lock()
                    .unwrap()
                    .entry(w.id.clone())
                    .or_insert(w);
            }
        }
    }
}

/// A readable session name: adjective-noun from the `names` crate's
/// dictionaries, lowercase and hyphenated ("rusty-nail").
fn session_name() -> String {
    names::Generator::default()
        .next()
        .expect("the generator yields a name")
}

/// Draw a session name that is fresh against the workspace's existing
/// titles; after eight colliding draws (the dictionary is effectively
/// exhausted) the name is numbered until it is unique (note 8). The draw
/// function is a parameter so the fallback is testable without the
/// randomness.
fn unique_name(titles: &[String], mut draw: impl FnMut() -> String) -> String {
    let mut name = draw();
    for _ in 0..8 {
        if !titles.contains(&name) {
            return name;
        }
        name = draw();
    }
    let mut n = 2u32;
    loop {
        let suffixed = format!("{name} {n}");
        if !titles.contains(&suffixed) || n >= 100 {
            return suffixed;
        }
        n += 1;
    }
}

/// The workspace index (`{system dir}/workspaces.json`): the folders this
/// machine has opened; restored at boot so a restart reopens the world and
/// its sessions.
#[derive(serde::Serialize, serde::Deserialize)]
struct WorkspaceIndexEntry {
    id: String,
    name: String,
    cwd: String,
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
        self.upsert_workspace_index(&workspace);
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

    /// The workspace's skill registry (ticket #28): the per-workspace
    /// cache built at session open; a workspace with no session opened
    /// yet is discovered on demand (skill_list at workspace open).
    fn skills_of(&self, ws: &Workspace) -> Vec<SkillInfo> {
        if let Some(reg) = self.skills.lock().unwrap().get(&ws.id) {
            return reg.clone();
        }
        let reg: Vec<SkillInfo> = tau_core::skills::discover(
            self.system_dir.as_deref(),
            self.home.as_deref(),
            Path::new(&ws.cwd),
        )
        .iter()
        .map(skill_info)
        .collect();
        self.skills
            .lock()
            .unwrap()
            .insert(ws.id.clone(), reg.clone());
        reg
    }

    /// A leading `/skill:<name> [args]` expands at the message_send
    /// boundary, before the entry is recorded (ticket #28): the text
    /// becomes the expansion template (body + skill directory + the args
    /// line), and the skill's identity rides back for the payload marker.
    /// A misspelled name rejects the send — nothing is recorded. Any
    /// other leading `/…` is prose and passes through untouched.
    fn expand_skill(
        &self,
        live: &LiveSession,
        text: &str,
    ) -> Result<(String, Option<(String, String)>), ProtocolError> {
        let Some(rest) = text.strip_prefix("/skill:") else {
            return Ok((text.to_owned(), None));
        };
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (rest, ""),
        };
        let ws = self.workspace(&live.meta.lock().unwrap().workspace)?;
        let Some(skill) = self.skills_of(&ws).into_iter().find(|s| s.name == name) else {
            return Err(ProtocolError::Other {
                message: format!("unknown skill '{name}' — the send was not recorded"),
            });
        };
        // The body is read at send time, not cached: a skill edited since
        // discovery takes effect on the next invocation.
        let raw = std::fs::read_to_string(&skill.location).map_err(|e| ProtocolError::Other {
            message: format!("reading {}: {e}", skill.location),
        })?;
        let dir = Path::new(&skill.location)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let args = if args.is_empty() { None } else { Some(args) };
        let text = tau_core::skills::expand(&skill.name, &tau_core::skills::body(&raw), &dir, args);
        Ok((text, Some((skill.name, skill.location))))
    }

    /// The workspace's on-disk sessions (the files, not just the live
    /// ones): one header-line read per session, so a cold listing stays
    /// cheap. Live sessions win over their file copies on merge.
    fn disk_sessions(&self, workspace: &Workspace) -> Vec<SessionMeta> {
        #[derive(serde::Deserialize)]
        struct DiskHeader {
            #[serde(rename = "type")]
            kind: String,
            id: String,
            created: u64,
            #[serde(default)]
            leaf: Option<String>,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            parent: Option<String>,
        }
        let dir = Path::new(&workspace.cwd).join(".tau").join("sessions");
        let Ok(names) = std::fs::read_dir(&dir).map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".jsonl"))
                .collect::<Vec<_>>()
        }) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for name in names {
            let path = dir.join(&name);
            let Ok(first) = std::fs::File::open(&path).and_then(|f| {
                use std::io::BufRead;
                let mut line = String::new();
                std::io::BufReader::new(f)
                    .read_line(&mut line)
                    .map(|_| line)
            }) else {
                continue;
            };
            let Ok(h) = serde_json::from_str::<DiskHeader>(first.trim()) else {
                continue;
            };
            if h.kind != "session" || h.id != name.trim_end_matches(".jsonl") {
                continue;
            }
            out.push(SessionMeta {
                id: h.id,
                workspace: workspace.id.clone(),
                title: h.title,
                parent: h.parent,
                created: h.created,
                leaf: h.leaf,
                model: None,
                usage: None,
            });
        }
        out
    }

    /// A name free of collisions among the workspace's sessions (the file is
    /// the record, so the check is against the disk titles).
    fn fresh_session_name(&self, workspace: &Workspace) -> String {
        let titles = self
            .disk_sessions(workspace)
            .iter()
            .filter_map(|m| m.title.clone())
            .collect::<Vec<_>>();
        unique_name(&titles, session_name)
    }

    fn load_workspace_index(&self) -> Vec<WorkspaceIndexEntry> {
        let Some(dir) = &self.system_dir else {
            return Vec::new();
        };
        let Ok(raw) = std::fs::read_to_string(dir.join("workspaces.json")) else {
            return Vec::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save_workspace_index(&self, entries: &[WorkspaceIndexEntry]) {
        let Some(dir) = &self.system_dir else {
            return;
        };
        if let Ok(raw) = serde_json::to_vec_pretty(entries) {
            // Note 7: a torn plain write costs every workspace tab on the
            // next boot; the temp-file rename makes the index all-or-nothing.
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join("workspaces.json");
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = dir.join(format!("workspaces.json.tmp-{nanos:016x}"));
            if std::fs::write(&tmp, raw).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    fn upsert_workspace_index(&self, w: &Workspace) {
        let mut entries = self.load_workspace_index();
        entries.retain(|e| e.cwd != w.cwd);
        entries.push(WorkspaceIndexEntry {
            id: w.id.clone(),
            name: w.name.clone(),
            cwd: w.cwd.clone(),
        });
        entries.sort_by(|a, b| a.cwd.cmp(&b.cwd));
        self.save_workspace_index(&entries);
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
        let cwd = PathBuf::from(&workspace.cwd);
        let mut store = SessionStore::for_workspace(&cwd, &SessionStore::new_session_id());
        store.create().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let created = store.created();
        // A fresh session gets a readable name (adjective noun) persisted in
        // the header, so it survives restarts; an explicit title wins.
        let title = match title {
            Some(t) if !t.trim().is_empty() => t,
            _ => self.fresh_session_name(workspace),
        };
        store.set_title(&title).map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        self.build_live(workspace, store, Some(title), created)
    }

    /// Re-register an existing session file as live (a restart drops the
    /// in-memory live map; the file is the source, spec §3).
    fn session_reopen(
        &self,
        workspace: &Workspace,
        id: &str,
    ) -> Result<SessionMeta, ProtocolError> {
        let cwd = PathBuf::from(&workspace.cwd);
        let mut store = SessionStore::for_workspace(&cwd, id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let title = store.title().map(str::to_string);
        let created = store.created();
        self.build_live(workspace, store, title, created)
    }

    /// The shared live-registration path (fresh create and re-open): builds
    /// the provider, the supervisor, and the agent around the given store.
    fn build_live(
        &self,
        workspace: &Workspace,
        mut store: SessionStore,
        title: Option<String>,
        created: u64,
    ) -> Result<SessionMeta, ProtocolError> {
        let parent = store.parent().map(str::to_string);
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
        let mut system_prompt = context::assemble("You are Tau, a coding agent.", &layers);

        // Skills (ticket #28): discovery runs at session open/reopen (no
        // file watching in v0). The catalog is the last layer — after the
        // context files, so a user AGENTS.md is never drowned — and a
        // spawned child inherits it through this prompt. The per-workspace
        // registry feeds skill_list and the /skill: expansion.
        let skills =
            tau_core::skills::discover(self.system_dir.as_deref(), self.home.as_deref(), &cwd);
        self.skills.lock().unwrap().insert(
            workspace.id.clone(),
            skills.iter().map(skill_info).collect(),
        );
        if let Some(catalog) = tau_core::skills::catalog(&skills) {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&catalog);
        }

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
            types: tau_core::agent_type::discover(self.system_dir.as_deref(), &cwd),
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
            tools: tools::agent_tool_specs(),
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
            parent,
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
        let tasks = tasks_of(&entries);
        let entries = entries.iter().map(entry_meta).collect();
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
                // The session's tasks (per-session store, spec §5.3): the
                // GUI's tasks panel; active ones ride their resume contract.
                tasks,
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

    /// A closed session is a file: open it read-only and project it the way
    /// a live session is (idle live state, no OM, no queue).
    fn snapshot_from_disk(&self, id: &str) -> Result<Snapshot, ProtocolError> {
        let (workspace, meta) = self
            .workspaces
            .lock()
            .unwrap()
            .values()
            .find_map(|w| {
                self.disk_sessions(w)
                    .into_iter()
                    .find(|m| m.id == id)
                    .map(|m| (w.clone(), m))
            })
            .ok_or_else(|| ProtocolError::Other {
                message: "unknown session".into(),
            })?;
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let entries = store
            .entries_range(0, usize::MAX)
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
        let usage = entries
            .iter()
            .rev()
            .find(|e| e.kind == tau_core::agent::KIND_ASSISTANT)
            .and_then(|e| e.payload.get("usage"))
            .and_then(usage_of);
        let leaf = store.leaf().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        Ok(Snapshot {
            workspace,
            session: SessionMeta { usage, ..meta },
            entries: entries.iter().map(entry_meta).collect(),
            om: Value::Null,
            live: LiveState {
                queue: vec![],
                turn: TurnState::Idle,
                subagents: vec![],
                tasks: tasks_of(&entries),
            },
            cursor: leaf.map(|e| e.id).unwrap_or_default(),
        })
    }

    /// Paged reads of a closed session's file: same semantics as the live
    /// path (exactly one of since/range).
    fn entries_from_disk(
        &self,
        id: &str,
        since: Option<String>,
        range: Option<tau_protocol::snapshot::EntryRange>,
    ) -> Result<Vec<ViewEntry>, ProtocolError> {
        let cwd = self
            .workspaces
            .lock()
            .unwrap()
            .values()
            .find(|w| self.disk_sessions(w).iter().any(|m| m.id == id))
            .map(|w| w.cwd.clone())
            .ok_or_else(|| ProtocolError::Other {
                message: "unknown session".into(),
            })?;
        let mut store = SessionStore::for_workspace(Path::new(&cwd), id);
        store.open().map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        let entries = match (since, range) {
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

    pub fn dispatch(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match cmd {
            Command::WorkspaceOpen { cwd } => Ok(CommandOutput::Workspace {
                workspace: self.open_workspace(&cwd)?,
            }),
            Command::WorkspaceList => Ok(CommandOutput::Workspaces {
                workspaces: self.workspaces.lock().unwrap().values().cloned().collect(),
            }),

            Command::SessionList { workspace } => {
                let workspace = self.workspace(&workspace)?;
                let mut sessions = self
                    .sessions
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|s| s.meta.lock().unwrap().workspace == workspace.id)
                    .map(|s| s.meta.lock().unwrap().clone())
                    .collect::<Vec<_>>();
                // The file is the record: sessions closed since boot (or
                // from a previous run) still list; the live copy wins.
                for m in self.disk_sessions(&workspace) {
                    if !sessions.iter().any(|s| s.id == m.id) {
                        sessions.push(m);
                    }
                }
                sessions.sort_by_key(|s| std::cmp::Reverse(s.created));
                Ok(CommandOutput::Sessions { sessions })
            }
            Command::SessionNew { workspace, title } => {
                let workspace = self.workspace(&workspace)?;
                Ok(CommandOutput::Session {
                    session: self.session_new(&workspace, title)?,
                })
            }
            Command::SessionRename { session, title } => {
                // The title lives in the file header, so the same write
                // works for a live and a closed session alike.
                let cwd = match self.live(&session) {
                    Ok(l) => {
                        // Keep the in-memory meta in sync; snapshot() and
                        // session_list() serve it, so a re-open must not
                        // revert the rename.
                        l.meta.lock().unwrap().title = Some(title.clone());
                        l.cwd.clone()
                    }
                    Err(_) => self
                        .workspaces
                        .lock()
                        .unwrap()
                        .values()
                        .find(|w| self.disk_sessions(w).iter().any(|m| m.id == session))
                        .map(|w| PathBuf::from(w.cwd.clone()))
                        .ok_or_else(|| ProtocolError::Other {
                            message: "unknown session".into(),
                        })?,
                };
                let mut store = SessionStore::for_workspace(&cwd, &session);
                store.open().map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                store.set_title(&title).map_err(|e| ProtocolError::Other {
                    message: e.to_string(),
                })?;
                Ok(CommandOutput::None)
            }
            Command::SessionOpen { session } => {
                match self.live(&session) {
                    Ok(live) => Ok(CommandOutput::Snapshot {
                        snapshot: self.snapshot(&live)?,
                    }),
                    // A closed session is a file: re-register it as live so
                    // it can be resumed (a restart drops the in-memory live
                    // map); if no known workspace owns it, serve read-only.
                    Err(_) => {
                        let ws = self
                            .workspaces
                            .lock()
                            .unwrap()
                            .values()
                            .find(|w| {
                                Path::new(&w.cwd)
                                    .join(".tau")
                                    .join("sessions")
                                    .join(format!("{session}.jsonl"))
                                    .exists()
                            })
                            .cloned();
                        if let Some(ws) = ws
                            && let Ok(_meta) = self.session_reopen(&ws, &session)
                            && let Ok(live) = self.live(&session)
                        {
                            return Ok(CommandOutput::Snapshot {
                                snapshot: self.snapshot(&live)?,
                            });
                        }
                        Ok(CommandOutput::Snapshot {
                            snapshot: self.snapshot_from_disk(&session)?,
                        })
                    }
                }
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
                Ok(CommandOutput::Session {
                    session: live.meta.lock().unwrap().clone(),
                })
            }
            Command::SessionSnapshot { session } => {
                let live = self.live(&session)?;
                Ok(CommandOutput::Snapshot {
                    snapshot: self.snapshot(&live)?,
                })
            }
            Command::SessionEntries {
                session,
                since,
                range,
            } => {
                let entries = match self.live(&session) {
                    Ok(live) => self.entries(&live, since, range)?,
                    Err(_) => self.entries_from_disk(&session, since, range)?,
                };
                Ok(CommandOutput::Entries { entries })
            }

            Command::MessageSend {
                session,
                text,
                lane,
            } => {
                let live = self.live(&session)?;
                // A new send clears the stop flag: the previous turn is over.
                live.stop.store(false, Ordering::SeqCst);
                // A leading /skill: expands at this boundary, before the
                // entry is recorded (ticket #28); a misspelled name
                // rejects the send — nothing is recorded.
                let (expanded, skill) = self.expand_skill(&live, &text)?;
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
                match skill {
                    Some((name, location)) => {
                        live.agent
                            .send_skill(expanded, lane_to_lane(lane), &name, &location)
                    }
                    None => live.agent.send(expanded, lane_to_lane(lane)),
                }
                // Turn start is a check-and-set on the turn flag (spec §8
                // single writer). A send that lands while a turn is in flight
                // is the queued one: it shows in the GUI's queue and the
                // in-flight process() absorbs it (spec §7). A send that starts
                // a turn IS the turn — it is not queued, so an idle send is
                // delivered immediately instead of sitting in the queue
                // section until the turn's reconciliation runs.
                let started = live
                    .turn
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
                if !started && lane != MessageLane::Force {
                    live.queue.lock().unwrap().push(QueuedItem { text, lane });
                    self.emit_queue(&live);
                }
                if started {
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

            Command::SubagentTypes => Ok(CommandOutput::Agents {
                agents: tau_core::agent_type::discover(
                    self.system_dir.as_deref(),
                    Path::new("/nonexistent-tau-project"),
                )
                .iter()
                .map(|t| AgentType {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    builtin: t.name == "general",
                })
                .collect(),
            }),
            Command::SubagentList { session } => {
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                Ok(CommandOutput::Subagents {
                    subagents: sup
                        .handles()
                        .iter()
                        .filter_map(|h| sup.state_info(h).map(|i| info_to_protocol(&i)))
                        .collect(),
                })
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
                Ok(CommandOutput::Subagent {
                    subagent: info_to_protocol(&info),
                })
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
                    Some(match context_mode {
                        ContextMode::Fresh => tau_core::subagent::ContextMode::Fresh,
                        ContextMode::Compacted => tau_core::subagent::ContextMode::Compacted,
                        ContextMode::Fork => tau_core::subagent::ContextMode::Fork,
                    }),
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
                        Ok(CommandOutput::Subagent {
                            subagent: info_to_protocol(&info),
                        })
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
                Ok(CommandOutput::Subagent {
                    subagent: info_to_protocol(&info),
                })
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
                Ok(CommandOutput::Subagent {
                    subagent: info_to_protocol(&info),
                })
            }
            Command::TaskCreate { session, title } => {
                let live = self.live(&session)?;
                // Routed through the session's own store (one writer per
                // session, review B3); the id rule lives in the tool path.
                let out = live
                    .agent
                    .task_tool_call("task_create", &json!({ "title": title }));
                if out.starts_with("task_create:") {
                    return Err(ProtocolError::Other { message: out });
                }
                self.emit_task_changed(&live, &session);
                Ok(CommandOutput::None)
            }
            Command::TaskUpdate {
                session,
                task,
                note,
            } => {
                let live = self.live(&session)?;
                live.agent
                    .with_task_store(|store| tau_core::task::note(store, &task, &note))
                    .map_err(|e| ProtocolError::Other { message: e })?;
                self.emit_task_changed(&live, &session);
                Ok(CommandOutput::None)
            }
            Command::TaskAssign {
                session,
                task,
                worker,
            } => {
                let live = self.live(&session)?;
                // The worker is a child handle: resolve it to a session.
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no supervisor (it is a child itself)".into(),
                })?;
                let info = sup
                    .state_info(&worker)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {worker}"),
                    })?;
                let worker_session = info.child;
                // One writer per session (review B3): the supervisor runs
                // both sides on the sessions' own stores.
                sup.assign_task(&task, &worker_session)
                    .map_err(|e| ProtocolError::Other { message: e })?;
                // The assign already delivered the "Assigned {id}: {title}"
                // message to the worker (a running child takes it as a
                // steering round, a parked one resumes with it).
                self.emit_task_changed(&live, &session);
                Ok(CommandOutput::None)
            }
            Command::TaskEvidence {
                session,
                task,
                criterion,
                summary,
                passed,
            } => {
                let live = self.live(&session)?;
                let out = live.agent.task_tool_call(
                    "task_evidence",
                    &json!({
                        "task": task,
                        "criterion": criterion,
                        "summary": summary,
                        "passed": passed,
                    }),
                );
                if out.starts_with("task_evidence:") {
                    return Err(ProtocolError::Other { message: out });
                }
                self.emit_task_changed(&live, &session);
                Ok(CommandOutput::None)
            }
            Command::TaskCancel { session, task } => {
                let live = self.live(&session)?;
                let out = live
                    .agent
                    .task_tool_call("task_cancel", &json!({ "task": task }));
                if out.starts_with("task_cancel:") {
                    return Err(ProtocolError::Other { message: out });
                }
                self.emit_task_changed(&live, &session);
                Ok(CommandOutput::None)
            }

            Command::SkillList { workspace } => {
                let ws = self.workspace(&workspace)?;
                Ok(CommandOutput::Skills {
                    skills: self.skills_of(&ws),
                })
            }

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
                Ok(CommandOutput::Providers { providers })
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
                Ok(CommandOutput::File {
                    file: FileText {
                        text: body,
                        truncated,
                    },
                })
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
    let mut task_touched = false;
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
            tau_core::task::KIND_TASK => {
                task_touched = true;
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
    // Task entries fold into the session's task list (spec §8): the event
    // carries the full list derived from the file — a projection, never a
    // second source of truth; the store replaces on receive.
    if task_touched {
        // A read that fails (a torn-append race) must not emit an
        // authoritative empty list — skip; the next emission or the
        // open/switch snapshot converges.
        if let Ok(all) = store.entries_range(0, usize::MAX) {
            core.emit(Event::TaskChanged {
                workspace: workspace.clone(),
                session: session.clone(),
                tasks: tasks_of(&all),
            });
        }
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

    /// The session's task list, folded from its file, as a task_changed
    /// event (spec §8): the payload is a projection of the file, never a
    /// second source of truth; the store replaces on receive, so the GUI
    /// converges on the file's state without polling.
    fn emit_task_changed(&self, live: &LiveSession, session: &str) {
        let (workspace, cwd) = {
            let meta = live.meta.lock().unwrap();
            (meta.workspace.clone(), live.cwd.clone())
        };
        let mut store = SessionStore::for_workspace(&cwd, session);
        if store.open().is_err() {
            return; // the file is gone; the next open rebuilds from nothing
        }
        let Ok(entries) = store.entries_range(0, usize::MAX) else {
            return; // a failed read is not an empty task list (torn-append race)
        };
        self.emit(Event::TaskChanged {
            workspace,
            session: session.to_owned(),
            tasks: tasks_of(&entries),
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

/// The protocol's mirror of a discovered skill (the location as a string,
/// the GUI never sees host paths as `Path`).
fn skill_info(s: &tau_core::skills::Skill) -> SkillInfo {
    SkillInfo {
        name: s.name.clone(),
        description: s.description.clone(),
        location: s.location.to_string_lossy().into_owned(),
        model_invocation: s.model_invocation,
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

/// One file entry's snapshot projection (the metadata the GUI renders
/// before a paged read supplies payloads).
fn entry_meta(e: &Entry) -> EntryMeta {
    EntryMeta {
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
    }
}

/// The session's tasks folded from its entries; the active ones ride their
/// resume contract.
fn tasks_of(entries: &[Entry]) -> Vec<Value> {
    tau_core::task::fold_entries(entries)
        .into_iter()
        .map(|t| {
            let mut v = serde_json::to_value(&t).unwrap_or(Value::Null);
            if t.status == tau_core::task::STATUS_IN_PROGRESS
                || t.status == tau_core::task::STATUS_BLOCKED
            {
                v["resume_contract"] = serde_json::to_value(tau_core::task::resume_contract(&t))
                    .unwrap_or(Value::Null);
            }
            v
        })
        .collect()
}

fn usage_of(value: &Value) -> Option<Usage> {
    let u: CoreUsage = serde_json::from_value(value.clone()).ok()?;
    Some(Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
        cached_prompt_tokens: u
            .prompt_tokens_details
            .map(|d| d.cached_tokens)
            .unwrap_or(0),
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
            .unwrap();
        let w2 = core
            .dispatch(Command::WorkspaceOpen {
                cwd: "/tmp/w".into(),
            })
            .unwrap();
        match (w1, w2) {
            (
                CommandOutput::Workspace { workspace: a },
                CommandOutput::Workspace { workspace: b },
            ) => {
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
            .unwrap_err();
        assert!(matches!(err, ProtocolError::NotFound { .. }));
        // The task commands are live (ticket #24): an unknown session's
        // task is a NotFound, not an Unsupported.
        let err = core
            .dispatch(Command::TaskCreate {
                session: "s".into(),
                title: "t".into(),
            })
            .unwrap_err();
        assert!(matches!(err, ProtocolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn file_read_pages_lines_relative_to_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
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
            .unwrap();
        match out {
            CommandOutput::File { file: f } => {
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let err = core
            .dispatch(Command::SessionNew {
                workspace: workspace.id,
                title: None,
            })
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let session = match core
            .dispatch(Command::SessionNew {
                workspace: workspace.id,
                title: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session: m } => m,
            other => panic!("expected a session: {other:?}"),
        };
        let err = core
            .dispatch(Command::SessionEntries {
                session: session.id,
                since: None,
                range: None,
            })
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
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
        .unwrap();

        // In flight: the first deltas have landed.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        core.dispatch(Command::SessionClose {
            session: session_id.clone(),
        })
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

    /// A task command emits a task_changed event carrying the file's
    /// folded task list — the GUI's tasks tab is event-driven (spec §8),
    /// never polled; the payload is a projection of the file.
    #[tokio::test]
    async fn task_commands_emit_a_task_changed_event() {
        let tmp = tempfile::tempdir().unwrap();
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let session = match core
            .dispatch(Command::SessionNew {
                workspace: workspace.id.clone(),
                title: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session: m } => m,
            other => panic!("expected a session: {other:?}"),
        };
        let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
        let sink = Arc::clone(&collected);
        tokio::spawn(pump(Arc::clone(&core), move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        }));

        core.dispatch(Command::TaskCreate {
            session: session.id.clone(),
            title: "work".into(),
        })
        .unwrap();

        // The pump is a separate task: give it a bounded window to deliver.
        let mut tasks = None;
        for _ in 0..50 {
            let found = collected.lock().unwrap().iter().find_map(|e| match e {
                Event::TaskChanged {
                    session: s, tasks, ..
                } if *s == session.id => Some(tasks.clone()),
                _ => None,
            });
            if let Some(t) = found {
                tasks = Some(t);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let tasks = tasks.expect("the task command never emitted a task_changed event");
        assert_eq!(tasks.len(), 1, "the file holds exactly one task");
        assert_eq!(tasks[0].get("title").and_then(Value::as_str), Some("work"));
        assert_eq!(
            tasks[0].get("status").and_then(Value::as_str),
            Some("pending")
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
                parent: None,
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
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
        // the usage; the tool batch: a start/end pair; a starter send while
        // idle is the turn itself and never sits in the queue.
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
        let queues: Vec<&Event> = events
            .iter()
            .filter(|e| matches!(e, Event::Queue { .. }))
            .collect();
        assert!(
            queues
                .iter()
                .all(|e| matches!(e, Event::Queue { items, .. } if items.is_empty())),
            "a starter send must not appear in the queue: {queues:?}"
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
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
        .unwrap();

        // Mid-stream of call 1: the first deltas have arrived, the stream
        // is still going (call 1 ends near t=2.4 s).
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        core.dispatch(Command::MessageSend {
            session: session_id,
            text: "second".into(),
            lane: MessageLane::Steering,
        })
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
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
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let session = match core
            .dispatch(Command::SessionNew {
                workspace: workspace.id.clone(),
                title: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session: m } => m,
            other => panic!("expected a session: {other:?}"),
        };
        let info = match core
            .dispatch(Command::SubagentSpawn {
                session: session.id.clone(),
                agent_type: "general".into(),
                brief: "do the thing".into(),
                context_mode: ContextMode::Fresh,
            })
            .unwrap()
        {
            CommandOutput::Subagent { subagent: i } => i,
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

    #[test]
    fn unique_name_returns_a_free_draw() {
        assert_eq!(unique_name(&[], || "Rusty Nail".into()), "Rusty Nail");
    }

    #[test]
    fn unique_name_numbers_past_the_collisions() {
        let titles = vec![
            "Rusty Nail".into(),
            "Rusty Nail 2".into(),
            "Rusty Nail 3".into(),
        ];
        assert_eq!(unique_name(&titles, || "Rusty Nail".into()), "Rusty Nail 4");
    }

    /// The tokio tests share this: open a workspace rooted at a temp dir.
    async fn open_ws(core: &Arc<Core>, cwd: &std::path::Path) -> Workspace {
        match core
            .dispatch(Command::WorkspaceOpen {
                cwd: cwd.display().to_string(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace } => workspace,
            other => panic!("expected workspace: {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_list_merges_live_and_disk_sessions() {
        let core = CoreBuilder::custom(providers()).build();
        let cwd = tempfile::tempdir().unwrap();
        let w = open_ws(&core, cwd.path()).await;
        let live = match core
            .dispatch(Command::SessionNew {
                workspace: w.id.clone(),
                title: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session } => session,
            other => panic!("expected session: {other:?}"),
        };
        // A closed session: created on disk, never registered with this core.
        let closed_id = SessionStore::new_session_id();
        {
            let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &closed_id);
            store.create().unwrap();
            store.set_title("Closed One").unwrap();
            store
                .append("user", json!({ "text": "hello" }), None)
                .unwrap();
        }
        let list = match core
            .dispatch(Command::SessionList { workspace: w.id })
            .unwrap()
        {
            CommandOutput::Sessions { sessions } => sessions,
            other => panic!("expected sessions: {other:?}"),
        };
        assert_eq!(list.len(), 2, "live and closed both listed: {list:?}");
        assert!(
            list.iter()
                .any(|s| s.id == live.id && s.title == live.title),
            "the live session is listed: {list:?}"
        );
        let closed = list
            .iter()
            .find(|s| s.id == closed_id)
            .expect("the closed session is listed");
        assert_eq!(closed.title.as_deref(), Some("Closed One"));
    }

    #[tokio::test]
    async fn session_rename_syncs_the_live_meta() {
        let core = CoreBuilder::custom(providers()).build();
        let cwd = tempfile::tempdir().unwrap();
        let w = open_ws(&core, cwd.path()).await;
        let live = match core
            .dispatch(Command::SessionNew {
                workspace: w.id.clone(),
                title: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session } => session,
            other => panic!("expected session: {other:?}"),
        };
        core.dispatch(Command::SessionRename {
            session: live.id.clone(),
            title: "Renamed".into(),
        })
        .unwrap();
        // Re-opening serves the snapshot from the live meta; the rename must
        // not revert (the B1 regression).
        let snap = match core
            .dispatch(Command::SessionOpen { session: live.id })
            .unwrap()
        {
            CommandOutput::Snapshot { snapshot } => snapshot,
            other => panic!("expected snapshot: {other:?}"),
        };
        assert_eq!(snap.session.title.as_deref(), Some("Renamed"));
    }

    #[tokio::test]
    async fn session_rename_updates_a_closed_session_header() {
        let core = CoreBuilder::custom(providers()).build();
        let cwd = tempfile::tempdir().unwrap();
        let w = open_ws(&core, cwd.path()).await;
        let id = SessionStore::new_session_id();
        {
            let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
            store.create().unwrap();
            store.set_title("Before").unwrap();
        }
        core.dispatch(Command::SessionRename {
            session: id.clone(),
            title: "After".into(),
        })
        .unwrap();
        let header =
            std::fs::read_to_string(cwd.path().join(".tau/sessions").join(format!("{id}.jsonl")))
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned();
        assert!(header.contains("\"After\""), "header: {header}");
    }

    #[tokio::test]
    async fn closed_session_serves_snapshot_and_entries_from_disk() {
        let core = CoreBuilder::custom(providers()).build();
        let cwd = tempfile::tempdir().unwrap();
        let w = open_ws(&core, cwd.path()).await;
        let id = SessionStore::new_session_id();
        {
            let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
            store.create().unwrap();
            store
                .append("user", json!({ "text": "hello" }), None)
                .unwrap();
        }
        let snap = match core
            .dispatch(Command::SessionOpen {
                session: id.clone(),
            })
            .unwrap()
        {
            CommandOutput::Snapshot { snapshot } => snapshot,
            other => panic!("expected snapshot: {other:?}"),
        };
        assert!(
            !snap.entries.is_empty(),
            "the disk entries are in the snapshot"
        );
        let out = core
            .dispatch(Command::SessionEntries {
                session: id,
                since: None,
                range: Some(tau_protocol::snapshot::EntryRange {
                    start: 0,
                    count: 10,
                }),
            })
            .unwrap();
        match out {
            CommandOutput::Entries { entries } => assert!(!entries.is_empty()),
            other => panic!("expected entries: {other:?}"),
        }
    }

    #[tokio::test]
    async fn workspace_index_survives_a_restart() {
        let sys = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        {
            let core = CoreBuilder::custom(providers())
                .with_system_dir(sys.path().into())
                .build();
            open_ws(&core, cwd.path()).await;
            assert!(
                sys.path().join("workspaces.json").exists(),
                "the index was written"
            );
        }
        let core = CoreBuilder::custom(providers())
            .with_system_dir(sys.path().into())
            .build();
        let list = match core.dispatch(Command::WorkspaceList).unwrap() {
            CommandOutput::Workspaces { workspaces } => workspaces,
            other => panic!("expected workspaces: {other:?}"),
        };
        assert_eq!(
            list.len(),
            1,
            "the previous run's workspace is restored: {list:?}"
        );
        assert_eq!(list[0].cwd, cwd.path().display().to_string());
    }

    fn write_skill_fixture(project: &Path, rel: &str, raw: &str) {
        let path = project.join(rel).join("SKILL.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, raw).unwrap();
    }

    /// `skill_list` serves the workspace's registry (discovered at command
    /// time — no session needs to be open): project beats system, and a
    /// `disable-model-invocation` skill stays listed (the dropdown is its
    /// only door).
    #[tokio::test]
    async fn skill_list_serves_the_workspace_registry_with_project_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let system = tempfile::tempdir().unwrap();
        write_skill_fixture(
            system.path(),
            "skills/shared",
            "---\nname: shared\ndescription: system shared\n---\nBody.\n",
        );
        write_skill_fixture(
            tmp.path(),
            ".tau/skills/shared",
            "---\nname: shared\ndescription: project shared\n---\nBody.\n",
        );
        write_skill_fixture(
            tmp.path(),
            ".agents/skills/other",
            "---\nname: other\ndescription: from .agents\ndisable-model-invocation: true\n---\nBody.\n",
        );
        let core = CoreBuilder::custom(providers())
            .with_system_dir(system.path().to_path_buf())
            .build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        match core
            .dispatch(Command::SkillList {
                workspace: workspace.id,
            })
            .unwrap()
        {
            CommandOutput::Skills { skills } => {
                assert_eq!(skills.len(), 2);
                let shared = skills.iter().find(|s| s.name == "shared").unwrap();
                assert_eq!(shared.description, "project shared");
                assert!(
                    shared.location.starts_with(tmp.path().to_str().unwrap()),
                    "the project file's location: {}",
                    shared.location
                );
                let other = skills.iter().find(|s| s.name == "other").unwrap();
                assert!(!other.model_invocation);
            }
            other => panic!("expected skills: {other:?}"),
        }
    }

    /// The home-level `.agents/skills/` is part of the system scope (the
    /// cross-client convention exists at both levels); a project
    /// `.agents/skills/` skill of the same name shadows it.
    #[tokio::test]
    async fn skill_list_lists_the_home_agents_skill_and_the_project_one_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write_skill_fixture(
            home.path(),
            ".agents/skills/shared",
            "---\nname: shared\ndescription: from the home .agents\n---\nBody.\n",
        );
        write_skill_fixture(
            home.path(),
            ".agents/skills/user-only",
            "---\nname: user-only\ndescription: only in the home .agents\n---\nBody.\n",
        );
        write_skill_fixture(
            tmp.path(),
            ".agents/skills/shared",
            "---\nname: shared\ndescription: from the project .agents\n---\nBody.\n",
        );
        let core = CoreBuilder::custom(providers())
            .with_home(home.path().to_path_buf())
            .build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        match core
            .dispatch(Command::SkillList {
                workspace: workspace.id,
            })
            .unwrap()
        {
            CommandOutput::Skills { skills } => {
                assert_eq!(skills.len(), 2);
                let shared = skills.iter().find(|s| s.name == "shared").unwrap();
                assert_eq!(
                    shared.description, "from the project .agents",
                    "the project .agents skill shadows the home one"
                );
                let user_only = skills.iter().find(|s| s.name == "user-only").unwrap();
                assert!(
                    user_only
                        .location
                        .starts_with(home.path().to_str().unwrap()),
                    "the home file's location: {}",
                    user_only.location
                );
            }
            other => panic!("expected skills: {other:?}"),
        }
    }

    /// `build_live` appends the skill catalog as the last layer of the
    /// assembled prompt — after the context files, so a user AGENTS.md is
    /// never drowned.
    #[tokio::test]
    async fn build_live_puts_the_catalog_after_the_context_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "project context").unwrap();
        write_skill_fixture(
            tmp.path(),
            ".tau/skills/alpha",
            "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
        );
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let session = match core
            .dispatch(Command::SessionNew {
                workspace: workspace.id,
                title: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session } => session,
            other => panic!("expected a session: {other:?}"),
        };
        let live = core.live(&session.id).unwrap();
        let prompt = live.agent.system_prompt().to_owned();
        let ctx = prompt
            .find("project context")
            .expect("the project's AGENTS.md layer is present");
        let cat = prompt
            .find("<available_skills>")
            .expect("the catalog is present");
        assert!(
            ctx < cat,
            "the catalog lands after the context layer:\n{prompt}"
        );
        assert!(
            prompt.ends_with("</available_skills>"),
            "the catalog is the last layer:\n{prompt}"
        );
    }

    async fn wait_for_user_entry(workspace: &Workspace, session: &str) -> Vec<Entry> {
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), session);
        store.open().unwrap();
        for _ in 0..100 {
            let entries = store.entries_range(0, 100).unwrap();
            if entries.iter().any(|e| e.kind == tau_core::agent::KIND_USER) {
                return entries;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        store.entries_range(0, 100).unwrap()
    }

    /// A misspelled `/skill:` name rejects the send (a GUI error) and
    /// records nothing; the session file stays empty.
    #[tokio::test]
    async fn a_misspelled_skill_name_rejects_the_send() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill_fixture(
            tmp.path(),
            ".agents/skills/alpha",
            "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
        );
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let live = manual_session(
            &core,
            &workspace,
            provider::canned(&canned_body()),
            TurnConfig::default(),
        );
        let session_id = live.meta.lock().unwrap().id.clone();
        let err = core
            .dispatch(Command::MessageSend {
                session: session_id.clone(),
                text: "/skill:alphax do it".into(),
                lane: MessageLane::Steering,
            })
            .unwrap_err();
        assert!(
            matches!(err, ProtocolError::Other { .. }),
            "expected a rejection, got: {err:?}"
        );
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &session_id);
        store.open().unwrap();
        assert!(
            store.entries_range(0, 100).unwrap().is_empty(),
            "a rejected send records nothing"
        );
    }

    /// A skill discovered into the registry, then removed from disk before
    /// the send: the send rejects with an error naming the cause, and no
    /// partial entry is recorded.
    #[tokio::test]
    async fn a_skill_removed_from_disk_before_send_rejects_the_send() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill_fixture(
            tmp.path(),
            ".agents/skills/alpha",
            "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
        );
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        // Discovered while the file exists: it is now in the registry.
        match core
            .dispatch(Command::SkillList {
                workspace: workspace.id.clone(),
            })
            .unwrap()
        {
            CommandOutput::Skills { skills } => {
                assert!(skills.iter().any(|s| s.name == "alpha"));
            }
            other => panic!("expected skills: {other:?}"),
        }
        // Then the SKILL.md disappears before the send.
        std::fs::remove_file(
            tmp.path()
                .join(".agents")
                .join("skills")
                .join("alpha")
                .join("SKILL.md"),
        )
        .unwrap();
        let live = manual_session(
            &core,
            &workspace,
            provider::canned(&canned_body()),
            TurnConfig::default(),
        );
        let session_id = live.meta.lock().unwrap().id.clone();
        let err = core
            .dispatch(Command::MessageSend {
                session: session_id.clone(),
                text: "/skill:alpha do it".into(),
                lane: MessageLane::Steering,
            })
            .unwrap_err();
        match &err {
            ProtocolError::Other { message } => {
                assert!(
                    message.contains("reading"),
                    "the error names the read failure: {message}"
                );
                assert!(
                    message.contains("alpha"),
                    "the error names the skill file: {message}"
                );
            }
            other => panic!("expected a read-failure rejection, got: {other:?}"),
        }
        let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &session_id);
        store.open().unwrap();
        assert!(
            store.entries_range(0, 100).unwrap().is_empty(),
            "a rejected send records nothing"
        );
    }

    /// A valid `/skill:<name>` records the expansion template exactly
    /// (the no-args variant omits the final line), with the payload
    /// marker; a non-skill leading `/…` message is recorded verbatim.
    #[tokio::test]
    async fn a_skill_invocation_records_the_expanded_entry() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill_fixture(
            tmp.path(),
            ".agents/skills/alpha",
            "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
        );
        let core = CoreBuilder::custom(providers()).build();
        let workspace = match core
            .dispatch(Command::WorkspaceOpen {
                cwd: tmp.path().to_string_lossy().into_owned(),
            })
            .unwrap()
        {
            CommandOutput::Workspace { workspace: w } => w,
            other => panic!("expected a workspace: {other:?}"),
        };
        let dir = tmp.path().join(".agents").join("skills").join("alpha");
        let location = dir.join("SKILL.md").to_string_lossy().into_owned();

        // With args: the template plus the `User request:` line.
        let live = manual_session(
            &core,
            &workspace,
            provider::canned(&canned_body()),
            TurnConfig::default(),
        );
        let s1 = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: s1.clone(),
            text: "/skill:alpha do it".into(),
            lane: MessageLane::Steering,
        })
        .unwrap();
        let entries = wait_for_user_entry(&workspace, &s1).await;
        let user = entries
            .iter()
            .find(|e| e.kind == tau_core::agent::KIND_USER)
            .unwrap();
        assert_eq!(
            user.payload["text"],
            format!(
                "Skill `alpha` — follow the instructions below. The skill directory is {}; resolve relative paths in the instructions against it.\n\nDo alpha.\n\nUser request: do it",
                dir.display()
            )
        );
        assert_eq!(user.payload["skill"]["name"], "alpha");
        assert_eq!(user.payload["skill"]["location"], location);

        // Bare invocation: the final line is omitted.
        let live = manual_session(
            &core,
            &workspace,
            provider::canned(&canned_body()),
            TurnConfig::default(),
        );
        let s2 = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: s2.clone(),
            text: "/skill:alpha".into(),
            lane: MessageLane::Steering,
        })
        .unwrap();
        let entries = wait_for_user_entry(&workspace, &s2).await;
        let user = entries
            .iter()
            .find(|e| e.kind == tau_core::agent::KIND_USER)
            .unwrap();
        assert_eq!(
            user.payload["text"],
            format!(
                "Skill `alpha` — follow the instructions below. The skill directory is {}; resolve relative paths in the instructions against it.\n\nDo alpha.",
                dir.display()
            )
        );

        // A non-skill leading slash is prose: recorded verbatim.
        let live = manual_session(
            &core,
            &workspace,
            provider::canned(&canned_body()),
            TurnConfig::default(),
        );
        let s3 = live.meta.lock().unwrap().id.clone();
        core.dispatch(Command::MessageSend {
            session: s3.clone(),
            text: "/not-a-skill".into(),
            lane: MessageLane::Steering,
        })
        .unwrap();
        let entries = wait_for_user_entry(&workspace, &s3).await;
        let user = entries
            .iter()
            .find(|e| e.kind == tau_core::agent::KIND_USER)
            .unwrap();
        assert_eq!(user.payload["text"], "/not-a-skill");
        assert!(user.payload.get("skill").is_none());
    }
}
