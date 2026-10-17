//! The dispatch surface: the protocol's `Command` enum mapped onto the
//! core's state — the entry point the transports call (spec §8, ADR-0006).
//! The arms are grouped per domain in their own files; the entry routes.

use super::{
    Arc, BufRead, Command, CommandOutput, Core, FileText, Path, PathBuf, ProtocolError,
    ProviderInfo, Workspace, list_dir,
};

fn resolve_path(cwd: &str, path: &str) -> PathBuf {
    let full = PathBuf::from(path);
    if full.is_absolute() {
        full
    } else {
        Path::new(cwd).join(full)
    }
}

/// `file_read` (line-streamed, 1 MB cap — review N9): an offset lands
/// anywhere in the file, not just inside the first MB; the read stops at
/// the cap or end of file. A short file is not truncation: only the cap
/// marks lost content.
fn file_read(
    workspace: &Workspace,
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<CommandOutput, ProtocolError> {
    let path = resolve_path(&workspace.cwd, path);
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
    let body = taken.join("\n");
    Ok(CommandOutput::File {
        file: FileText {
            text: body,
            truncated,
        },
    })
}
impl Core {
    /// The workspace's cached git snapshot (file-tree plan, phase 2): a
    /// cache hit is a map read; a miss runs the three-spawn snapshot once
    /// and stores the result — including `None` for non-git workspaces.
    fn git_snapshot(&self, workspace: &Workspace) -> Option<super::git::GitSnapshot> {
        let mut cache = self
            .git_snapshots
            .lock()
            .expect("git snapshots: no panic while the lock is held");
        if let Some(v) = cache.get(&workspace.id) {
            return v.clone();
        }
        let fresh = super::git::git_snapshot(Path::new(&workspace.cwd)).ok();
        cache.insert(workspace.id.clone(), fresh.clone());
        fresh
    }

    pub fn dispatch(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match &cmd {
            Command::WorkspaceOpen { .. }
            | Command::WorkspaceList
            | Command::WorkspaceClose { .. } => self.dispatch_workspace(cmd),
            Command::SessionList { .. }
            | Command::SessionNew { .. }
            | Command::SessionRename { .. }
            | Command::SessionSetModel { .. }
            | Command::SessionOpen { .. }
            | Command::SessionClose { .. }
            | Command::SessionDelete { .. }
            | Command::SessionArchive { .. }
            | Command::SessionRestore { .. }
            | Command::SessionFork { .. }
            | Command::SessionBranch { .. }
            | Command::SessionSnapshot { .. }
            | Command::SessionEntries { .. }
            | Command::BlobRead { .. } => self.dispatch_session(cmd),
            Command::MessageSend { .. }
            | Command::MessageStop { .. }
            | Command::SubagentTypes
            | Command::SubagentList { .. }
            | Command::SubagentState { .. }
            | Command::SubagentSpawn { .. }
            | Command::SubagentMessage { .. }
            | Command::SubagentStop { .. } => self.dispatch_command(cmd),
            Command::SkillList { .. }
            | Command::ProviderList
            | Command::ProviderAdd { .. }
            | Command::ProviderSet { .. }
            | Command::ProviderDelete { .. }
            | Command::FileRead { .. }
            | Command::FileList { .. }
            | Command::FileUnlist { .. }
            | Command::GitStatus { .. }
            | Command::GitShow { .. } => self.dispatch_misc(cmd),
        }
    }

    fn dispatch_workspace(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match cmd {
            Command::WorkspaceOpen { cwd } => Ok(CommandOutput::Workspace {
                workspace: self.open_workspace(&cwd),
            }),
            Command::WorkspaceList => Ok(CommandOutput::Workspaces {
                workspaces: self
                    .workspaces
                    .lock()
                    .expect("workspaces map: no panic while the lock is held")
                    .values()
                    .cloned()
                    .collect(),
            }),
            Command::WorkspaceClose { workspace } => {
                self.close_workspace(&workspace)?;
                Ok(CommandOutput::None)
            }
            _ => unreachable!("dispatch routes the arm"),
        }
    }

    fn dispatch_misc(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match cmd {
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
                        models: p.models.keys().cloned().collect(),
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
                file_read(&workspace, &path, offset, limit)
            }
            Command::FileList { workspace, path } => {
                let workspace = self.workspace(&workspace)?;
                let dir = resolve_path(&workspace.cwd, &path);
                self.watch_tree_dir(&workspace, &dir);
                let mut files = list_dir(Path::new(&workspace.cwd), &dir);
                // The tree's git awareness (file-tree plan, phase 2): the
                // cached snapshot annotates ignore + change state.
                super::git::annotate(&mut files, self.git_snapshot(&workspace).as_ref());
                Ok(CommandOutput::Files { files })
            }

            Command::FileUnlist { workspace, path } => {
                let workspace = self.workspace(&workspace)?;
                let dir = resolve_path(&workspace.cwd, &path);
                self.unwatch_tree_dir(&workspace, &dir);
                Ok(CommandOutput::None)
            }
            Command::GitStatus { workspace } => {
                let workspace = self.workspace(&workspace)?;
                let status = self.git_snapshot(&workspace).ok_or(ProtocolError::Other {
                    message: "git status: no snapshot (not a git workspace?)".into(),
                })?;
                Ok(CommandOutput::GitStatus {
                    status: super::git::rebase_status(status.status, &status.offset),
                })
            }
            Command::GitShow { workspace, path } => {
                let workspace = self.workspace(&workspace)?;
                Ok(CommandOutput::GitShow {
                    text: super::git::git_show(Path::new(&workspace.cwd), &path)?,
                })
            }
            _ => unreachable!("dispatch routes the arm"),
        }
    }
}
