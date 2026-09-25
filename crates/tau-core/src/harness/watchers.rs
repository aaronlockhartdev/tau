//! The watcher consumers: the home/project/tree watchers, the batch-to-stale-dirs mapping, the dir listing, and the teardown.

use super::*;

impl Core {
    /// The home-level roots are identical for every workspace (design #30):
    /// one global watcher; a batch re-discovers all open workspaces, each
    /// getting its own `SkillListChanged`. A `None` seam (the test shape)
    /// watches nothing — the watcher is a no-op.
    pub(crate) fn start_home_watcher(self: &Arc<Self>) {
        let mut roots = Vec::new();
        if let Some(dir) = &self.system_dir {
            roots.push(dir.join("skills"));
        }
        if let Some(home) = &self.home {
            roots.push(home.join(".agents").join("skills"));
        }
        if roots.is_empty() {
            return;
        }
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE);
        for root in &roots {
            watcher.add(root);
        }
        self.home_watcher.lock().unwrap().replace(watcher);
        let core = Arc::clone(self);
        spawn_watcher_consumer(core, rx, move |_core, _batch| {
            let core = _core;
            for ws in core.workspaces.lock().unwrap().values() {
                core.refresh_skills(ws);
            }
        });
    }

    /// The workspace's project roots (the `.tau/skills` + `.agents/skills`
    /// pair), watched from the first `open_workspace`; a batch re-discovers
    /// that workspace. `workspace_close` drops the entry (the debouncer
    /// stops on drop), so the map tracks the open workspaces. A missing
    /// project dir has no roots to watch.
    pub(crate) fn start_project_watcher(&self, workspace: &Workspace) {
        if !Path::new(&workspace.cwd).is_dir() {
            return;
        }
        let mut map = self.project_watchers.lock().unwrap();
        if map.contains_key(&workspace.id) {
            return;
        }
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE);
        let cwd = Path::new(&workspace.cwd);
        watcher.add(&cwd.join(".tau").join("skills"));
        watcher.add(&cwd.join(".agents").join("skills"));
        map.insert(workspace.id.clone(), watcher);
        drop(map);
        let Some(core) = self.self_arc() else {
            return;
        };
        let id = workspace.id.clone();
        spawn_watcher_consumer(core, rx, move |_core, _batch| {
            let core = _core;
            if let Ok(ws) = core.workspace(&id) {
                core.refresh_skills(&ws);
            }
        });
    }

    /// The workspace's tree watcher (the files pane, ticket #32): the second
    /// consumer of the shared `Watcher` plumbing — a per-workspace watch on
    /// the cwd with the design's exclusions. A batch maps to the stale dir
    /// paths (workspace-relative) and emits `FileTreeChanged`: the client
    /// refetches the affected listed dirs, and any fetch is a fresh read, so
    /// a lost batch self-heals on the next expand.
    pub(crate) fn start_tree_watcher(&self, workspace: &Workspace) {
        let cwd = Path::new(&workspace.cwd);
        if !cwd.is_dir() {
            return;
        }
        let mut map = self.tree_watchers.lock().unwrap();
        if map.contains_key(&workspace.id) {
            return;
        }
        let (mut watcher, rx) = Watcher::new(WATCH_DEBOUNCE);
        watcher.add_excluded(cwd, TREE_EXCLUDES);
        map.insert(workspace.id.clone(), watcher);
        drop(map);
        let Some(core) = self.self_arc() else {
            return;
        };
        let id = workspace.id.clone();
        let cwd = workspace.cwd.clone();
        spawn_watcher_consumer(core, rx, move |core, batch| {
            let Some(ws) = core.workspaces.lock().unwrap().get(&id).cloned() else {
                return;
            };
            let changed = tree_changed_dirs(&cwd, &batch);
            if !changed.is_empty() {
                core.emit(Event::FileTreeChanged {
                    workspace: ws.id,
                    changed,
                });
            }
        });
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
    core: Arc<Core>,
    mut rx: mpsc::UnboundedReceiver<Batch>,
    handler: impl Fn(Arc<Core>, Batch) + Send + 'static,
) {
    let weak = Arc::downgrade(&core);
    std::thread::Builder::new()
        .name("tau-watcher".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("the watcher runtime builds");
            rt.block_on(async move {
                while let Some(batch) = rx.recv().await {
                    let Some(core) = weak.upgrade() else {
                        break;
                    };
                    handler(core, batch);
                }
            });
        })
        .expect("the watcher thread spawns");
}

/// The drop is the test teardown for the watchers a close did not stop;
/// it joins each debouncer thread (the `Watcher`'s own drop does the
/// joining).
impl Drop for Core {
    fn drop(&mut self) {
        self.home_watcher.lock().unwrap().take();
        self.project_watchers.lock().unwrap().clear();
        self.tree_watchers.lock().unwrap().clear();
    }
}

/// One debounced batch's stale dir paths, workspace-relative (the files
/// pane, ticket #32): each changed path contributes itself (if it is a
/// dir) and its parent (the dir that now lists it), excluded subtrees
/// dropped, the root reported as `.`. An empty batch is a rescan trigger
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
        let rel = match path.strip_prefix(cwd) {
            Ok(r) => r,
            Err(_) => {
                unmatched = true;
                continue;
            }
        };
        let components = rel.components();
        // A change inside an excluded subtree is not the pane's business.
        if components
            .clone()
            .any(|c| matches!(c.as_os_str().to_str().unwrap_or_default(), n if TREE_EXCLUDES.contains(&n)))
        {
            continue;
        }
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

/// One directory's listing (the files pane, ticket #32): the excluded names
/// dropped, paths workspace-relative (the root is `.`), dirs first, then
/// name — the pane's top-down reading order. A vanished dir lists empty: a
/// refetch after a delete is how the tree forgets it.
pub(crate) fn list_dir(cwd: &Path, dir: &Path) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for ent in read.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if TREE_EXCLUDES.contains(&name.as_ref()) {
            continue;
        }
        let path = ent.path();
        let is_dir = path.is_dir();
        let size = if is_dir {
            0
        } else {
            ent.metadata().map(|m| m.len()).unwrap_or(0)
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
        });
    }
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    out
}
