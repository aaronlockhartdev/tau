//! The watcher consumers: the home/project/config/tree/plan watchers, the
//! batch-to-stale-dirs mapping, the dir listing, and the teardown.

use super::{
    Arc, Batch, Config, Core, Event, FileEntry, Path, ProviderInfo, SystemEventKind,
    WATCH_DEBOUNCE, Watcher, Workspace, config, dispatch, mpsc, state::CoreError,
};

impl Core {
    /// The home-level roots are identical for every workspace (design #30):
    /// one global watcher; a batch re-discovers all open workspaces, each
    /// getting its own `SkillListChanged`. A `None` seam (the test shape)
    /// watches nothing — the watcher is a no-op.
    pub(crate) fn start_home_watcher(self: &Arc<Self>) -> Result<(), CoreError> {
        let mut roots = Vec::new();
        if let Some(dir) = &self.system_dir {
            roots.push(dir.join("skills"));
        }
        if let Some(home) = &self.home {
            roots.push(home.join(".agents").join("skills"));
        }
        if roots.is_empty() {
            return Ok(());
        }
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE)?;
        for root in &roots {
            watcher.add(root);
        }
        self.home_watcher.lock().replace(watcher);
        let core = Arc::clone(self);
        spawn_watcher_consumer(&core, rx, move |core, _batch| {
            for ws in core.workspaces.lock().values() {
                core.refresh_skills(ws);
            }
        })?;
        Ok(())
    }

    /// The workspace's project roots (the `.tau/skills` + `.agents/skills`
    /// pair), watched from the first `open_workspace`; a batch re-discovers
    /// that workspace. `workspace_close` drops the entry (the debouncer
    /// stops on drop), so the map tracks the open workspaces. A missing
    /// project dir has no roots to watch.
    pub(crate) fn start_project_watcher(&self, workspace: &Workspace) -> Result<(), CoreError> {
        if !Path::new(&workspace.cwd).is_dir() {
            return Ok(());
        }
        let mut map = self.project_watchers.lock();
        if map.contains_key(&workspace.id) {
            return Ok(());
        }
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE)?;
        let cwd = Path::new(&workspace.cwd);
        watcher.add(&cwd.join(".tau").join("skills"));
        watcher.add(&cwd.join(".agents").join("skills"));
        map.insert(workspace.id.clone(), watcher);
        drop(map);
        let Some(core) = self.self_arc() else {
            return Ok(());
        };
        let id = workspace.id.clone();
        spawn_watcher_consumer(&core, rx, move |core, _batch| {
            if let Ok(ws) = core.workspace(&id) {
                core.refresh_skills(&ws);
            }
        })?;
        Ok(())
    }

    /// The config layer's watcher (spec §12, #337): the system dir, watched
    /// non-recursively. Started once globally (the system layer is shared by every
    /// workspace); a `None` seam (the test shape) watches nothing.
    pub(crate) fn start_system_config_watcher(&self) -> Result<(), CoreError> {
        let Some(dir) = self.system_dir.as_deref() else {
            return Ok(());
        };
        self.start_layer_watcher(
            dir,
            move |core, watcher| {
                core.system_config_watcher.lock().replace(watcher);
            },
            |core| core.reload_system_config(),
        )
    }

    /// The workspace's project config layer (`.tau/config.toml`, #337), started
    /// from the first `open_workspace`. `workspace_close` drops the entry (the
    /// debouncer stops on drop), so the map tracks the open workspaces.
    pub(crate) fn start_config_watcher(&self, workspace: &Workspace) -> Result<(), CoreError> {
        let project = Path::new(&workspace.cwd).join(".tau");
        if self.config_watchers.lock().contains_key(&workspace.id) {
            return Ok(());
        }
        let parked = workspace.id.clone();
        let id = workspace.id.clone();
        self.start_layer_watcher(
            &project,
            move |core, watcher| {
                core.config_watchers.lock().insert(parked, watcher);
            },
            move |core| {
                if let Ok(ws) = core.workspace(&id) {
                    core.reload_workspace_config(&ws);
                }
            },
        )
    }

    /// One config layer's watcher (#337): watch `dir` non-recursively, park its
    /// `Watcher` where teardown can find it, and reload on a batch naming
    /// `CONFIG_FILE`. A dir that is not a directory has no layer to watch: the
    /// watcher is dropped (its `stop` runs) and reads stay file-only.
    fn start_layer_watcher(
        &self,
        dir: &Path,
        park: impl FnOnce(&Core, Watcher),
        reload: impl Fn(&Arc<Core>) + Send + 'static,
    ) -> Result<(), CoreError> {
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE)?;
        watcher.watch_path(dir);
        if !watcher.watching(dir) {
            return Ok(());
        }
        park(self, watcher);
        let Some(core) = self.self_arc() else {
            return Ok(());
        };
        spawn_watcher_consumer(&core, rx, move |core, batch| {
            if batch_carries_config_file(&batch) {
                reload(&core);
            }
        })?;
        Ok(())
    }

    /// The system layer's reload (spec §12, #337): the swap lands only on a
    /// successful read, so a rejected edit leaves the last good config live and
    /// `ConfigRejected` is that failure's only route to the user (ADR-0034: it
    /// belongs to no session). A success drops the merged entries derived from
    /// this layer (the next read re-merges the current files); a failure deletes
    /// no key, since serving `Config::default()` in place of the last good merge
    /// is the worse failure. `ProviderChanged` is edge-triggered per workspace.
    pub(crate) fn reload_system_config(&self) {
        let workspaces: Vec<Workspace> = self.workspaces.lock().values().cloned().collect();
        let before: Vec<Vec<ProviderInfo>> =
            workspaces.iter().map(|ws| self.provider_rows(ws)).collect();
        let config = match self.read_system_config() {
            Ok(config) => config,
            Err(e) => {
                self.emit_config_rejected("", &e);
                return;
            }
        };
        self.put_merged(config, &workspaces);
        for (ws, before) in workspaces.iter().zip(before) {
            if self.provider_rows(ws) != before {
                self.emit_provider_changed(&ws.id);
            }
        }
    }

    /// The rows this workspace's model menu shows — the `provider_list` answer,
    /// so `ProviderChanged` means exactly what a menu would newly show (#337).
    fn provider_rows(&self, workspace: &Workspace) -> Vec<ProviderInfo> {
        dispatch::provider_rows(&self.workspace_config(workspace))
    }

    /// The workspace layer's reload (spec §12, #337): a success swaps that
    /// workspace's merged entry (the project layer feeds exactly one). A failure
    /// emits `ConfigRejected` and writes nothing, leaving the last good merge live.
    pub(crate) fn reload_workspace_config(&self, workspace: &Workspace) {
        let before = self.provider_rows(workspace);
        match self.read_workspace_config(workspace) {
            Ok(config) => {
                let rows = dispatch::provider_rows(&config);
                self.configs.lock().insert(workspace.id.clone(), config);
                if rows != before {
                    self.emit_provider_changed(&workspace.id);
                }
            }
            Err(e) => self.emit_config_rejected(&workspace.id, &e),
        }
    }

    /// Swap the system entry in and drop the merged entries derived from it
    /// (the reload's success path; the merged keys refill from the new system
    /// entry at the next read). One lock scope: a reader between the two
    /// writes would serve the old system layer under a new project layer.
    fn put_merged(&self, system: Config, workspaces: &[Workspace]) {
        let mut configs = self.configs.lock();
        configs.insert(String::new(), system);
        for ws in workspaces {
            configs.remove(&ws.id);
        }
    }
    fn emit_provider_changed(&self, workspace: &str) {
        self.emit(Event::System {
            workspace: workspace.to_owned(),
            session: None,
            kind: SystemEventKind::ProviderChanged,
        });
    }

    /// A rejected edit's record (#337). `path` names the layer file when the
    /// failure has one — a validation failure is the merged config's property,
    /// not one file's, so it names none. Session-less by ADR-0034, so it rides
    /// the system group; the workspace key is the routing.
    fn emit_config_rejected(&self, workspace: &str, err: &config::LoadError) {
        self.emit(Event::System {
            workspace: workspace.to_owned(),
            session: None,
            kind: SystemEventKind::ConfigRejected {
                path: err.path().map(|p| p.display().to_string()),
                reason: err.to_string(),
            },
        });
    }

    /// The workspace's tree watcher (the files pane, #32/#115): the second
    /// consumer of the shared `Watcher` plumbing. It starts empty — no dir is
    /// watched at open — and tracks the pane: a `file_list` (expand) watches the
    /// dir, a `file_unlist` (collapse) drops it. A batch maps to the stale dir
    /// paths (workspace-relative) and emits `FileTreeChanged`: the client
    /// refetches the affected listed dirs, and any fetch is a fresh read, so
    /// a lost batch self-heals on the next expand.
    pub(crate) fn start_tree_watcher(&self, workspace: &Workspace) -> Result<(), CoreError> {
        let cwd = Path::new(&workspace.cwd);
        if !cwd.is_dir() {
            return Ok(());
        }
        let mut map = self.tree_watchers.lock();
        if map.contains_key(&workspace.id) {
            return Ok(());
        }
        let (watcher, rx) = Watcher::new(WATCH_DEBOUNCE)?;
        map.insert(workspace.id.clone(), watcher);
        drop(map);
        let Some(core) = self.self_arc() else {
            return Ok(());
        };
        let id = workspace.id.clone();
        let cwd = workspace.cwd.clone();
        spawn_watcher_consumer(&core, rx, move |core, batch| {
            let Some(ws) = core.workspaces.lock().get(&id).cloned() else {
                return;
            };
            let changed = tree_changed_dirs(&cwd, &batch);
            if !changed.is_empty() {
                // A tree change invalidates the git snapshot: the next
                // listing (or numstat) rebuilds it.
                core.git_snapshots.lock().remove(&id);
                core.emit(Event::FileTreeChanged {
                    workspace: ws.id,
                    changed,
                });
            }
        })?;
        Ok(())
    }

    /// The workspace's plan watcher (design: the plan file is the source
    /// of truth): the `.tau/plans` root, started from the first
    /// `open_workspace`; a batch re-reads the changed plan files into the
    /// core's state cache. `workspace_close` drops the entry (the
    /// debouncer stops on drop), so the map tracks the open workspaces.
    pub(crate) fn start_plan_watcher(&self, workspace: &Workspace) -> Result<(), CoreError> {
        let cwd = Path::new(&workspace.cwd);
        if !cwd.is_dir() {
            return Ok(());
        }
        let mut map = self.plan_watchers.lock();
        if map.contains_key(&workspace.id) {
            return Ok(());
        }
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE)?;
        watcher.add(&cwd.join(".tau").join("plans"));
        map.insert(workspace.id.clone(), watcher);
        drop(map);
        let Some(core) = self.self_arc() else {
            return Ok(());
        };
        let id = workspace.id.clone();
        spawn_watcher_consumer(&core, rx, move |core, batch| {
            let Some(ws) = core.workspaces.lock().get(&id).cloned() else {
                return;
            };
            refresh_plan_batch(&core, &ws, &batch);
        })?;
        Ok(())
    }

    /// Register a dir on the workspace's tree watcher (visibility-driven,
    /// #115): a `file_list` means the pane is showing the dir, so watch it.
    /// No-op if the workspace has no tree watcher yet.
    pub(crate) fn watch_tree_dir(&self, workspace: &Workspace, dir: &Path) {
        let mut map = self.tree_watchers.lock();
        if let Some(watcher) = map.get_mut(&workspace.id) {
            watcher.watch_path(dir);
        }
    }

    /// Drop a dir from the workspace's tree watcher (visibility-driven,
    /// #115): a `file_unlist` means the pane collapsed the dir.
    pub(crate) fn unwatch_tree_dir(&self, workspace: &Workspace, dir: &Path) {
        let mut map = self.tree_watchers.lock();
        if let Some(watcher) = map.get_mut(&workspace.id) {
            watcher.unwatch_path(dir);
        }
    }
}

/// The watcher's consumer task (ticket #31): a dedicated thread with its
/// own current-thread runtime drains the batch channel and runs the
/// per-batch handler. Both call sites are outside a runtime (the home
/// watcher is built before the app's `.run()`, the project watcher from a
/// command), where a bare `tokio::spawn` panics; a consumer pinned to the
/// ambient runtime would die with it. The `Weak` keeps the task from
/// pinning the core — the test's drop joins the thread through the
/// watcher's `stop()`. An empty batch is a rescan trigger: a consumer that
/// cares about which paths changed treats it as every watched root being
/// affected.
pub(crate) fn spawn_watcher_consumer(
    core: &Arc<Core>,
    mut rx: mpsc::UnboundedReceiver<Batch>,
    handler: impl Fn(Arc<Core>, Batch) + Send + 'static,
) -> Result<(), CoreError> {
    let weak = Arc::downgrade(core);
    std::thread::Builder::new()
        .name("tau-watcher".into())
        .spawn(move || -> Result<(), String> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .map_err(|e| format!("watcher runtime build failed: {e}"))?;
            rt.block_on(async move {
                while let Some(batch) = rx.recv().await {
                    let Some(core) = weak.upgrade() else {
                        break;
                    };
                    handler(core, batch);
                }
            });
            Ok(())
        })
        .map_err(|e| CoreError(format!("watcher thread spawn failed: {e}")))?;
    Ok(())
}

/// A batch's plan updates (design: the watcher is the parser's only call
/// site): each changed `.md` under the workspace's plans dir re-reads that
/// session's plan file. An empty batch is a rescan trigger (the design's
/// lost-events case) — every plan file in the dir is re-read, so a lost
/// event self-heals on the next rescan.
pub(crate) fn refresh_plan_batch(core: &Core, ws: &Workspace, batch: &Batch) {
    let cwd = Path::new(&ws.cwd);
    if batch.is_empty() {
        if let Ok(rd) = std::fs::read_dir(cwd.join(".tau").join("plans")) {
            for ent in rd.flatten() {
                if let Some(sid) = crate::plan::session_of(&ent.path(), cwd) {
                    core.refresh_plan(cwd, &sid, false);
                }
            }
        }
        return;
    }
    for path in batch {
        if let Some(sid) = crate::plan::session_of(path, cwd) {
            core.refresh_plan(cwd, &sid, false);
        }
    }
}

/// A batch's config-file paths (#337): the watched layer dir carries the
/// sessions, skills, and plans too, so only a batch naming `config.toml`
/// means config. An empty batch is a rescan trigger: the layer is treated as
/// changed, so a lost event self-heals on the next rescan (the read costs two
/// file reads).
pub(crate) fn batch_carries_config_file(batch: &Batch) -> bool {
    batch.is_empty()
        || batch.iter().any(|p| {
            p.file_name()
                .is_some_and(|n| n == std::ffi::OsStr::new(crate::config::CONFIG_FILE))
        })
}

/// The drop is the test teardown for the watchers a close did not stop;
/// it joins each debouncer thread (the `Watcher`'s own drop does the
/// joining).
impl Drop for Core {
    fn drop(&mut self) {
        self.home_watcher.lock().take();
        self.system_config_watcher.lock().take();
        self.project_watchers.lock().clear();
        self.config_watchers.lock().clear();
        self.tree_watchers.lock().clear();
        self.plan_watchers.lock().clear();
        self.plans.lock().clear();
    }
}

/// One debounced batch's stale dir paths, workspace-relative (the files
/// pane, ticket #32): each changed path contributes itself (if it is a
/// dir) and its parent (the dir that now lists it), the root
/// reported as `.`. An empty batch is a rescan trigger
/// (the design's lost-events case): the root is stale. A path we cannot
/// attribute to the workspace (the OS resolved a symlink the cwd string
/// does not carry, e.g. macOS `/var` → `/private/var`) marks the whole
/// tree stale: a superset, and the client coalesces it to one refetch
/// wave — silence would leave the pane permanently blind.
pub(crate) fn tree_changed_dirs(cwd: &str, batch: &Batch) -> Vec<String> {
    let cwd = Path::new(cwd);
    if batch.is_empty() {
        return vec![".".to_string()];
    }
    let mut out = Vec::new();
    let mut unmatched = false;
    for path in batch {
        let Ok(rel) = path.strip_prefix(cwd) else {
            unmatched = true;
            continue;
        };
        let components = rel.components();
        let mut parts: Vec<String> = components
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        out.push(if parts.is_empty() {
            ".".into()
        } else {
            parts.join("/")
        });
        parts.pop();
        out.push(if parts.is_empty() {
            ".".into()
        } else {
            parts.join("/")
        });
    }
    if unmatched {
        out.push(".".to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// One directory's listing (the files pane, ticket #32): paths
/// workspace-relative (the root is `.`), dirs first, then
/// name — the pane's top-down reading order. A vanished dir lists empty: a
/// refetch after a delete is how the tree forgets it.
pub(crate) fn list_dir(cwd: &Path, dir: &Path) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for ent in read.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        let path = ent.path();
        let is_dir = path.is_dir();
        let size = if is_dir {
            0
        } else {
            ent.metadata().map_or(0, |m| m.len())
        };
        let rel = path.strip_prefix(cwd).unwrap_or(path.as_path());
        let rel = if rel.as_os_str().is_empty() {
            ".".into()
        } else {
            rel.to_string_lossy().into_owned()
        };
        out.push(FileEntry {
            name,
            path: rel,
            dir: is_dir,
            size,
            ignored: false,
            git: None,
        });
    }
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    out
}
