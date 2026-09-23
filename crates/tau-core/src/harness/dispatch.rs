//! The dispatch surface: the protocol's `Command` enum mapped onto the
//! core's state — the entry point the transports call (spec §8, ADR-0006).
//! The arms are grouped per domain in their own files; the entry routes.

use super::*;

impl Core {
    pub fn dispatch(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match &cmd {
            Command::WorkspaceOpen { .. } | Command::WorkspaceList => self.dispatch_workspace(cmd),
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
            | Command::SessionEntries { .. } => self.dispatch_session(cmd),
            Command::MessageSend { .. }
            | Command::MessageStop { .. }
            | Command::SubagentTypes
            | Command::SubagentList { .. }
            | Command::SubagentState { .. }
            | Command::SubagentSpawn { .. }
            | Command::SubagentMessage { .. }
            | Command::SubagentStop { .. }
            | Command::TaskCreate { .. }
            | Command::TaskUpdate { .. }
            | Command::TaskAssign { .. }
            | Command::TaskEvidence { .. }
            | Command::TaskCancel { .. } => self.dispatch_command(cmd),
            Command::SkillList { .. }
            | Command::ProviderList
            | Command::ProviderAdd { .. }
            | Command::ProviderSet { .. }
            | Command::ProviderDelete { .. }
            | Command::FileRead { .. }
            | Command::FileList { .. } => self.dispatch_misc(cmd),
        }
    }

    fn dispatch_workspace(self: &Arc<Self>, cmd: Command) -> Result<CommandOutput, ProtocolError> {
        match cmd {
            Command::WorkspaceOpen { cwd } => Ok(CommandOutput::Workspace {
                workspace: self.open_workspace(&cwd)?,
            }),
            Command::WorkspaceList => Ok(CommandOutput::Workspaces {
                workspaces: self.workspaces.lock().unwrap().values().cloned().collect(),
            }),
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
            Command::FileList { workspace, path } => {
                let workspace = self.workspace(&workspace)?;
                let full = PathBuf::from(&path);
                let dir = if full.is_absolute() {
                    full
                } else {
                    Path::new(&workspace.cwd).join(full)
                };
                Ok(CommandOutput::Files {
                    files: list_dir(Path::new(&workspace.cwd), &dir),
                })
            }
            _ => unreachable!("dispatch routes the arm"),
        }
    }
}
