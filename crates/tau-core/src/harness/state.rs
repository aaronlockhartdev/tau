//! Core state: the owned registry (workspaces, live sessions), the builder, the workspace index, and the small shared helpers.

use super::*;

/// The watcher's debounce window (design #30): a save burst (a temp-write
/// followed by an atomic rename) ends well inside it; downstream work (a
/// few directory reads, depth ≤ 4) is trivial, so there is nothing to
/// gain from a longer window, and 500 ms reads as instant in the GUI.
pub(crate) const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// The coalescing window: 25 ms (spec §8, ADR-0006).
pub(crate) const COALESCE_MS: u64 = 25;
/// The files pane's excluded dir names (design #30): a hard requirement,
/// not an optimization — on Linux inotify a recursive watch is one
/// descriptor per directory (this repo: 4,471, 3,777 under `target/`).
pub(crate) const TREE_EXCLUDES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "__pycache__",
];

/// A title's cap: the archive listing reads the header line bounded
/// (4 KiB), so a title must stay far below that — an oversized title
/// would overflow the read and the entry would silently drop from the
/// archive list, losing its only restore row (review N3).
pub(crate) const MAX_TITLE_LEN: usize = 200;

/// The client's connect-phase timeout (applied to the shared client; the
/// streaming body is bounded per provider by `requests.timeout_secs` as a
/// per-chunk idle deadline). A connect that takes this long is a dead
/// endpoint, not a slow model.
pub(crate) const CONNECT_TIMEOUT_SECS: u64 = 30;
/// A live session: the loop plus the binding's view of its lanes, the
/// stop flag, and the provider (kept here so a turn can be diffed against
/// the calls it started).
pub(crate) struct LiveSession {
    pub(crate) meta: Mutex<SessionMeta>,
    pub(crate) agent: Arc<AgentSession>,
    /// User stop (spec §7): the forwarding sink returns false on it, which
    /// cuts the in-flight stream the way a force does — a stop is a force
    /// with no message.
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) queue: Mutex<Vec<QueuedItem>>,
    pub(crate) turn: AtomicBool,
    /// The forwarding provider (the only live surface that sees stream
    /// events); also the provider the loop runs against.
    pub(crate) provider: Arc<ForwardingProvider>,
    pub(crate) cwd: PathBuf,
}

/// The core's owned state: workspaces (identity, never paths) and the live
/// sessions. Everything the GUI can show is derivable from here + the
/// session files (ADR-0006 state ownership).
pub struct Core {
    pub(crate) workspaces: Mutex<BTreeMap<String, Workspace>>,
    pub(crate) sessions: Mutex<HashMap<String, Arc<LiveSession>>>,
    pub(crate) configs: Mutex<HashMap<String, Config>>,
    /// Per-workspace skill registry (tickets #28/#31): built at session
    /// open and refreshed by the file watcher; consulted by the
    /// message_send boundary and `skill_list`.
    pub(crate) skills: Mutex<HashMap<String, Vec<SkillInfo>>>,
    /// The home-level skill roots (`{system}/skills` + `{home}/.agents/skills`),
    /// watched once globally (ticket #31): identical for every workspace, so
    /// one watcher re-discovers all open workspaces. A `None` seam (the test
    /// shape) is a no-op — there are no roots to watch.
    pub(crate) home_watcher: Mutex<Option<Watcher>>,
    /// The per-workspace project roots (the `.tau/skills` + `.agents/skills`
    /// pair), created at the first `open_workspace`; a batch re-discovers
    /// that workspace. `workspace_close` drops the entry (the debouncer
    /// stops on drop), so the map tracks the open workspaces.
    pub(crate) project_watchers: Mutex<HashMap<String, Watcher>>,
    /// The per-workspace tree watcher (the files pane, ticket #32): the
    /// second consumer of the shared `Watcher` plumbing, watching the
    /// workspace cwd with the design's exclusions.
    pub(crate) tree_watchers: Mutex<HashMap<String, Watcher>>,
    pub(crate) system_dir: Option<PathBuf>,
    /// The user's home dir (the home-level `.agents/skills/` root; the
    /// `system_dir` seam's sibling for tests).
    pub(crate) home: Option<PathBuf>,
    pub(crate) custom: bool,
    pub(crate) client: reqwest::Client,
    pub(crate) events_tx: mpsc::Sender<Event>,
    /// The event pipe's drop bookkeeping (never silent: a drop is counted
    /// and a summary error is delivered at the next successful send).
    pub(crate) pipe: PipeCounters,
    /// The pump's half of the events channel; taken exactly once.
    pub(crate) events_rx: Mutex<Option<mpsc::Receiver<Event>>>,
    /// The test-seam child provider factory (None in production builds).
    pub(crate) child_factory: Option<Arc<dyn ChildProviderFactory>>,
    /// Self-reference for the seams that need an `Arc<Core>` (the child
    /// driver/factory/bridge); set in `build`.
    pub(crate) self_weak: Mutex<Option<Weak<Core>>>,
}

pub struct CoreBuilder {
    system_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    /// A custom root carries an explicit provider list (no system layer);
    /// a production root gets full file-level layering (spec §12).
    custom: bool,
    providers: BTreeMap<String, crate::config::Provider>,
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
    pub fn custom(providers: BTreeMap<String, crate::config::Provider>) -> Self {
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
            pipe: PipeCounters {
                dropped: Arc::new(AtomicU64::new(0)),
                overflow_pending: Arc::new(AtomicBool::new(false)),
            },
            self_weak: Mutex::new(None),
            workspaces: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            skills: Mutex::new(HashMap::new()),
            home_watcher: Mutex::new(None),
            project_watchers: Mutex::new(HashMap::new()),
            tree_watchers: Mutex::new(HashMap::new()),
            system_dir: self.system_dir.clone(),
            home: self.home.clone(),
            custom: self.custom,
            // Test builds use a no-pool client: a pooled keep-alive connection
            // keeps the tokio runtime alive after the test, hanging teardown.
            client: if self.custom {
                reqwest::Client::builder()
                    .pool_max_idle_per_host(0)
                    .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
                    .build()
                    .expect("client")
            } else {
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
                    .build()
                    .expect("client")
            },
            events_tx,
            events_rx: Mutex::new(Some(rx)),
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
        // The workspaces a previous run left open: re-register the ones
        // whose folder still exists (sessions are read from disk on
        // demand); a closed workspace (B7) stays closed across the restart.
        for e in core.load_workspace_index() {
            if e.open && Path::new(&e.cwd).is_dir() {
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
        // The home-level roots are watched once globally (ticket #31); a
        // `None` seam (the test shape) is a no-op.
        core.start_home_watcher();
    }
}

/// A readable session name: adjective-noun from the `names` crate's
/// dictionaries, lowercase and hyphenated ("rusty-nail").
pub(crate) fn session_name() -> String {
    names::Generator::default()
        .next()
        .expect("the generator yields a name")
}

/// Draw a session name that is fresh against the workspace's existing
/// titles; after eight colliding draws (the dictionary is effectively
/// exhausted) the name is numbered until it is unique (note 8). The draw
/// function is a parameter so the fallback is testable without the
/// randomness.
pub(crate) fn unique_name(titles: &[String], mut draw: impl FnMut() -> String) -> String {
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
    /// The tab's open flag (B7): a closed workspace stays closed across a
    /// restart. Entries written before the flag existed default to open.
    #[serde(default = "open_default")]
    open: bool,
}
fn open_default() -> bool {
    true
}
/// The routing pair an overflow notification rides on: the dropped
/// event's own workspace/session, insofar as it has one.
fn event_route(event: &Event) -> (String, Option<String>) {
    match event {
        Event::System {
            workspace, session, ..
        } => (workspace.clone(), session.clone()),
        Event::SkillListChanged { workspace, .. } => (workspace.clone(), None),
        Event::FileTreeChanged { workspace, .. } => (workspace.clone(), None),
        Event::StreamStart {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::StreamDelta {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::StreamEnd {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::ToolStart {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::ToolEnd {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::Queue {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::SessionEvent {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::OmStatus {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::SubagentEvent {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
        Event::TaskChanged {
            workspace, session, ..
        } => (workspace.clone(), Some(session.clone())),
    }
}

/// The event pipe's drop bookkeeping, shared by the core and every
/// forwarding sink: the drop counter (the ground truth) and the
/// not-yet-delivered overflow summary.
#[derive(Clone)]
pub(crate) struct PipeCounters {
    /// Total events dropped at the pipe's boundary.
    pub(crate) dropped: Arc<AtomicU64>,
    /// An overflow happened whose summary has not been delivered yet.
    pub(crate) overflow_pending: Arc<AtomicBool>,
}

/// Send one event to the GUI, surfacing drops: an overflow (the bounded
/// channel full) is counted and owes a summary. The summary cannot ride
/// the same full channel, so it is delivered by the next *successful*
/// send — the first moment the pipe has room again.
pub(crate) fn pipe_send(
    tx: &mpsc::Sender<Event>,
    counters: &PipeCounters,
    event: Event,
    workspace: String,
    session: Option<String>,
) {
    match tx.try_send(event) {
        Ok(()) => {
            if counters.overflow_pending.swap(false, Ordering::Relaxed) {
                let total = counters.dropped.load(Ordering::Relaxed);
                let summary = Event::System {
                    workspace,
                    session,
                    kind: SystemEventKind::Error {
                        message: format!(
                            "event pipe overflow: {total} event(s) dropped — the view may be missing events; reload"
                        ),
                    },
                };
                // The summary can miss too (the channel refills in the
                // same instant): then it stays owed.
                if tx.try_send(summary).is_err() {
                    counters.overflow_pending.store(true, Ordering::Relaxed);
                }
            }
        }
        Err(_) => {
            counters.dropped.fetch_add(1, Ordering::Relaxed);
            counters.overflow_pending.store(true, Ordering::Relaxed);
        }
    }
}
impl Core {
    pub fn events(&self) -> mpsc::Receiver<Event> {
        self.events_rx
            .lock()
            .unwrap()
            .take()
            .expect("the event pump takes the receiver exactly once")
    }

    pub(crate) fn emit(&self, event: Event) {
        let (workspace, session) = event_route(&event);
        pipe_send(&self.events_tx, &self.pipe, event, workspace, session);
    }

    /// The core as an `Arc` (the child seams keep one); None pre-`build`.
    pub(crate) fn self_arc(&self) -> Option<Arc<Core>> {
        self.self_weak
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
    }

    pub(crate) fn system_config(&self) -> Config {
        self.configs
            .lock()
            .unwrap()
            .get("")
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn open_workspace(&self, cwd: &str) -> Result<Workspace, ProtocolError> {
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
        self.set_workspace_open(&workspace, true);
        self.start_project_watcher(&workspace);
        self.start_tree_watcher(&workspace);
        Ok(workspace)
    }

    pub(crate) fn workspace(&self, id: &str) -> Result<Workspace, ProtocolError> {
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
    /// cache; a workspace with no session opened yet is discovered on
    /// demand (skill_list at workspace open). The watcher refreshes the
    /// slot between opens (ticket #31), so a stale list never outlives a
    /// change.
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

    /// Set the workspace's `open` flag in the index (B7): true on open,
    /// false on close. The entry is created if absent (a first open, or a
    /// close racing ahead of any saved open) so the flag always sticks.
    fn set_workspace_open(&self, w: &Workspace, open: bool) {
        let mut entries = self.load_workspace_index();
        if let Some(e) = entries.iter_mut().find(|e| e.cwd == w.cwd) {
            e.open = open;
        } else {
            entries.push(WorkspaceIndexEntry {
                id: w.id.clone(),
                name: w.name.clone(),
                cwd: w.cwd.clone(),
                open,
            });
            entries.sort_by(|a, b| a.cwd.cmp(&b.cwd));
        }
        self.save_workspace_index(&entries);
    }

    /// Close (archive) a workspace (B7): persist its `open` flag as closed,
    /// then drop it from the live set — a `workspace_list` that still
    /// carried it would resurrect the just-closed tab on the next sync —
    /// and stop its watchers. The sessions stay on disk; the workspace
    /// re-opens.
    pub(crate) fn close_workspace(&self, id: &str) -> Result<(), ProtocolError> {
        let w = self.workspace(id)?;
        self.set_workspace_open(&w, false);
        self.workspaces.lock().unwrap().remove(id);
        self.project_watchers.lock().unwrap().remove(id);
        self.tree_watchers.lock().unwrap().remove(id);
        Ok(())
    }

    pub(crate) fn workspace_config(&self, workspace: &Workspace) -> Config {
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
            if self.custom {
                // Custom roots carry explicit providers: a project entry
                // replaces the same-named root entry, others coexist.
                for (name, p) in loaded.providers {
                    c.providers.insert(name, p);
                }
            } else {
                // Full file-level layering (spec §12): `loaded` is the
                // system + project merge itself.
                c = loaded;
            }
        }
        self.configs
            .lock()
            .unwrap()
            .insert(workspace.id.clone(), c.clone());
        c
    }

    pub(crate) fn system_dir_of(&self) -> PathBuf {
        // A custom root has no system layer: the marker directory holds no
        // config.toml, so `config::load` contributes defaults only — a test
        // build never reads the developer's real `~/.config/tau` (N8).
        self.system_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("/nonexistent-tau-system"))
    }

    pub(crate) fn live(&self, session: &str) -> Result<Arc<LiveSession>, ProtocolError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .ok_or_else(|| ProtocolError::NotFound {
                what: format!("session {session} is not open"),
            })
    }

    /// The write refusal for an archived session (ADR-0005): its live
    /// file is gone, so an append would recreate it headerless (the
    /// restore would then refuse it) — every write command checks this
    /// before it mutates anything.
    pub(crate) fn live_unarchived(&self, session: &str) -> Result<Arc<LiveSession>, ProtocolError> {
        let live = self.live(session)?;
        let (archived, has_parent) = {
            let meta = live.meta.lock().unwrap();
            (meta.archived, meta.parent.is_some())
        };
        if archived {
            return Err(ProtocolError::Other {
                message: if has_parent {
                    format!(
                        "session {session} is archived with its parent — restore the parent first"
                    )
                } else {
                    format!("session {session} is archived — restore it first")
                },
            });
        }
        Ok(live)
    }
}

pub(crate) fn lane_to_lane(lane: MessageLane) -> Lane {
    match lane {
        MessageLane::Force => Lane::Force,
        MessageLane::Steering => Lane::Steering,
        MessageLane::FollowUp => Lane::FollowUp,
    }
}

/// The inverse of `lane_to_lane` (re-queueing a core-lane message into
/// the GUI's queue, where the protocol's lane is the currency).
pub(crate) fn lane_to_message_lane(lane: Lane) -> MessageLane {
    match lane {
        Lane::Force => MessageLane::Force,
        Lane::Steering => MessageLane::Steering,
        Lane::FollowUp => MessageLane::FollowUp,
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
