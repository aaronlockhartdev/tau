//! Session lifecycle: skill discovery at the session boundary, the live registration path (create / re-open), and the live-vs-disk lookups.

use super::*;

impl Core {
    /// The workspace's skill registry (ticket #28): the per-workspace
    /// cache; a workspace with no session opened yet is discovered on
    /// demand (skill_list at workspace open). The watcher refreshes the
    /// slot between opens (ticket #31), so a stale list never outlives a
    /// change.
    pub(crate) fn skills_of(&self, ws: &Workspace) -> Vec<SkillInfo> {
        if let Some(reg) = self.skills.lock().unwrap().get(&ws.id) {
            return reg.clone();
        }
        self.refresh_skills(ws);
        self.skills
            .lock()
            .unwrap()
            .get(&ws.id)
            .cloned()
            .unwrap_or_default()
    }

    /// Re-run discovery for `ws`, replace its registry slot, and emit the
    /// full-state `SkillListChanged` (ticket #31; the `task_changed`
    /// pattern — idempotent by construction, a lost batch self-heals on
    /// the next `skill_list`). Shared by the watcher's consumer and
    /// `build_live`: the slot is the one the dropdown and the `/skill:`
    /// lookup read. A running session's prompt is not touched (frozen at
    /// build); new sessions pick up the change at their own build.
    pub(crate) fn refresh_skills(&self, ws: &Workspace) -> Vec<crate::skills::Skill> {
        let skills = crate::skills::discover(
            self.system_dir.as_deref(),
            self.home.as_deref(),
            Path::new(&ws.cwd),
        );
        let reg: Vec<SkillInfo> = skills.iter().map(skill_info).collect();
        self.skills
            .lock()
            .unwrap()
            .insert(ws.id.clone(), reg.clone());
        self.emit(Event::SkillListChanged {
            workspace: ws.id.clone(),
            skills: reg,
        });
        skills
    }

    /// A leading `/skill:<name> [args]` expands at the message_send
    /// boundary, before the entry is recorded (ticket #28): the text
    /// becomes the expansion template (body + skill directory + the args
    /// line), and the skill's identity rides back for the payload marker.
    /// A misspelled name rejects the send — nothing is recorded. Any
    /// other leading `/…` is prose and passes through untouched.
    pub(crate) fn expand_skill(
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
        let text = crate::skills::expand(&skill.name, &crate::skills::body(&raw), &dir, args);
        Ok((text, Some((skill.name, skill.location))))
    }

    /// The one session-access seam (ADR-0005): the workspace's on-disk
    /// sessions, live dir and `archive/` alike, as one list — the file is
    /// the record, so every arm that needs disk truth goes through here
    /// (the session crate owns the header format; this call stamps the
    /// workspace id the harness owns).
    pub(crate) fn session_access(&self, workspace: &Workspace) -> Vec<SessionMeta> {
        crate::session::list_workspace(Path::new(&workspace.cwd))
            .into_iter()
            .map(|mut m| {
                m.workspace = workspace.id.clone();
                m
            })
            .collect()
    }

    /// A name free of collisions among the workspace's sessions (the file is
    /// the record, so the check is against the disk titles).
    pub(crate) fn fresh_session_name(&self, workspace: &Workspace) -> String {
        let titles = self
            .session_access(workspace)
            .iter()
            .filter_map(|m| m.title.clone())
            .collect::<Vec<_>>();
        unique_name(&titles, session_name)
    }

    /// The direct-archive refusal for a sub-agent: the invariant's route
    /// is to archive the parent, which archives the child with it — and
    /// the message must name the route that actually works — a closed
    /// parent in an open workspace archives from disk; one whose
    /// workspace is closed needs a reopen first.
    pub(crate) fn archive_refusal_for_child(
        &self,
        child: &str,
        workspace: &str,
        parent: &Option<String>,
    ) -> String {
        let Some(parent) = parent.as_deref() else {
            return format!(
                "session {child} is a sub-agent — archive its parent session to archive it"
            );
        };
        if self.sessions.lock().unwrap().contains_key(parent) {
            return format!(
                "session {child} is a sub-agent — archive its parent session {parent} to archive it"
            );
        }
        let Ok(ws) = self.workspace(workspace) else {
            return format!(
                "session {child} is a sub-agent — the parent session {parent} is not open — reopen it and archive it, which archives this sub-agent with it"
            );
        };
        let mut store = SessionStore::for_workspace(Path::new(&ws.cwd), parent);
        if store.path().exists() {
            let title = store
                .open()
                .ok()
                .and_then(|_| store.title().map(str::to_string));
            return format!(
                "session {child} is a sub-agent — the parent session {parent} ({}) is not open — reopen it and archive it, which archives this sub-agent with it",
                title.as_deref().unwrap_or(parent)
            );
        }
        if store.archive_path().exists() {
            return format!(
                "session {child} is a sub-agent — its parent session {parent} is archived — restore it, which restores this sub-agent with it"
            );
        }
        format!(
            "session {child} is a sub-agent — its parent session {parent} is not open and has no session file"
        )
    }

    /// The archive's post-detach re-check (ADR-0005): a wake that grabbed
    /// the session's Arc before the detach can CAS its turn in the window
    /// — a refused archive re-inserts the detached session and mutates
    /// nothing, so the file never moves mid-write.
    pub(crate) fn archive_turn_recheck(
        &self,
        session: &str,
        live: &Arc<LiveSession>,
    ) -> Result<(), ProtocolError> {
        if live.turn.load(Ordering::SeqCst) {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.to_owned(), live.clone());
            return Err(ProtocolError::Other {
                message: format!(
                    "session {session} started a turn while archiving — stop it and retry"
                ),
            });
        }
        Ok(())
    }

    pub(crate) fn session_new(
        &self,
        workspace: &Workspace,
        title: Option<String>,
    ) -> Result<SessionMeta, ProtocolError> {
        // The same cap as a rename: the archive listing reads the header
        // line bounded, so an oversized title would drop the entry from
        // the list. The check runs before the file is created — a refused
        // request mutates nothing.
        if let Some(t) = &title
            && t.chars().count() > MAX_TITLE_LEN
        {
            return Err(ProtocolError::Other {
                message: format!(
                    "title is {} characters — the maximum is {MAX_TITLE_LEN}",
                    t.chars().count()
                ),
            });
        }
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
    pub(crate) fn session_reopen(
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
    pub(crate) fn build_live(
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
        // A session that picked a non-default model (session_set_model) keeps
        // it across close and re-open: the file's last `model:` note is the
        // record (a fresh session has none and takes the provider default).
        let model = last_model_note(&mut store).unwrap_or(model);
        let cwd = PathBuf::from(&workspace.cwd);

        // The per-session OM record (ticket #22): reconstructed from the
        // file on open; a fresh session starts with the default record.
        let record = crate::om_integration::OmState::load_record(&mut store).map_err(|e| {
            ProtocolError::Other {
                message: e.to_string(),
            }
        })?;

        // The loop assembles no context of its own: base prompt + context
        // files (spec §10) are built here, once, at session creation.
        let layers = context::discover(&cwd, &self.system_dir_of());
        let mut system_prompt = context::assemble("You are Tau, a coding agent.", &layers);

        // Skills (tickets #28/#31): discovery runs at session open/reopen
        // — the catalog is frozen at that moment (a running session's prompt
        // is not re-derived; new sessions pick up watcher changes at their
        // own build). The catalog is the last layer — after the context
        // files, so a user AGENTS.md is never drowned — and a spawned child
        // inherits it through this prompt.
        let skills = self.refresh_skills(workspace);
        if let Some(catalog) = crate::skills::catalog(&skills) {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&catalog);
        }

        let provider = ForwardingProvider {
            inner: session_inner(&self.client, &provider, &config.requests),
            tx: self.events_tx.clone(),
            pipe: self.pipe.clone(),
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
                Arc::new(ForwardingChildFactory {
                    client: self.client.clone(),
                    provider: first_provider.clone(),
                    requests: config.requests.clone(),
                    tx: self.events_tx.clone(),
                    pipe: self.pipe.clone(),
                    workspace: workspace.id.clone(),
                })
            });
        // The bridge and the supervisor reference each other: build the
        // bridge with an empty weak and patch it in after construction.
        let bridge = Arc::new(SessionSubagentBridge {
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
            types: crate::agent_type::discover(self.system_dir.as_deref(), &cwd),
            depth: 0,
            bridge: bridge.clone() as Arc<dyn SubagentBridge>,
            driver: Arc::new(TurnChildDriver {
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
            om: Some(crate::om_integration::OmState::from_config(
                &config.om, record,
            )),
            om_model: config.om.om_model.clone(),
            subagents: Some(sup.clone()),
            child: None,
        }));
        // The core's om_status hook takes the kind string; this closure
        // shapes it into the protocol event on the shared channel.
        {
            let tx = self.events_tx.clone();
            let ws = workspace.id.clone();
            let sid = provider.session.clone();
            agent.set_om_status_hook(Some(Arc::new(move |kind: &str| {
                let _ = tx.try_send(Event::OmStatus {
                    workspace: ws.clone(),
                    session: sid.clone(),
                    kind: match kind {
                        "observing" => OmStatusKind::Observing,
                        "reflecting" => OmStatusKind::Reflecting,
                        _ => OmStatusKind::Idle,
                    },
                });
            })));
        }
        let meta = SessionMeta {
            id: provider.session.clone(),
            workspace: workspace.id.clone(),
            title,
            parent,
            created,
            leaf: None,
            model: Some(model),
            usage: None,
            archived: false,
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
}

/// The live-turn refusal shared by the header-writers (rename, fork,
/// branch) and the delete: the same guard as the archive (ADR-0005).
pub(crate) fn running_turn_refusal(session: &str) -> ProtocolError {
    ProtocolError::Other {
        message: format!("session {session} is running — stop it first"),
    }
}
