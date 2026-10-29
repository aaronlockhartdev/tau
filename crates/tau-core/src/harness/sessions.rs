//! Session lifecycle: skill discovery at the session boundary, the live registration path (create / re-open), and the live-vs-disk lookups.

use super::thinking::thinking_adjusted_max_output;
use super::{
    AgentSession, Arc, AtomicBool, Config, Core, Event, ForwardingProvider, LiveSession,
    MAX_TITLE_LEN, ModelAddress, Mutex, Ordering, Path, PathBuf, ProtocolError, SessionMeta,
    SessionRole, SessionStore, Skeleton, SkillInfo, TurnConfig, Value, ViewEntry, Workspace,
    session_inner, session_name, session_open, skill_info, unique_name,
};
use crate::config::{ProviderRef, Route};
use crate::diagnostics::Diagnostics;
use tau_protocol::EntryKind;
impl Core {
    /// The workspace's skill registry (ticket #28): the per-workspace
    /// cache; a workspace with no session opened yet is discovered on
    /// demand (`skill_list` at workspace open). The watcher refreshes the
    /// slot between opens (ticket #31), so a stale list never outlives a
    /// change.
    pub(crate) fn skills_of(&self, ws: &Workspace) -> Vec<SkillInfo> {
        if let Some(reg) = self.skills.lock().get(&ws.id) {
            return reg.clone();
        }
        self.refresh_skills(ws);
        self.skills.lock().get(&ws.id).cloned().unwrap_or_default()
    }

    /// Re-run discovery for `ws`, replace its registry slot, and emit the
    /// full-state `SkillListChanged` (ticket #31 — idempotent by
    /// construction, a lost batch self-heals on
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
        self.skills.lock().insert(ws.id.clone(), reg.clone());
        self.emit(Event::SkillListChanged {
            workspace: ws.id.clone(),
            skills: reg,
        });
        skills
    }

    /// A leading `/skill:<name> [args]` expands at the `message_send`
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
        let ws = self.workspace(&live.meta.lock().workspace)?;
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
            .map(std::path::Path::to_path_buf)
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
    pub(crate) fn session_access(workspace: &Workspace) -> Vec<SessionMeta> {
        crate::session::list_workspace(Path::new(&workspace.cwd))
            .into_iter()
            .map(|mut m| {
                workspace.id.clone_into(&mut m.workspace);
                m
            })
            .collect()
    }

    /// The (workspace, session-meta) pair owning a non-archived session,
    /// scanning every open workspace — the one home for that lookup.
    pub(crate) fn owning_session(&self, id: &str) -> Option<(Workspace, SessionMeta)> {
        self.workspaces.lock().values().find_map(|w| {
            Self::session_access(w)
                .into_iter()
                .find(|m| m.id == id && !m.archived)
                .map(|m| (w.clone(), m))
        })
    }

    /// A name free of collisions among the workspace's sessions (the file is
    /// the record, so the check is against the disk titles).
    pub(crate) fn fresh_session_name(workspace: &Workspace) -> String {
        let titles = Self::session_access(workspace)
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
        parent: Option<&str>,
    ) -> String {
        let Some(parent) = parent else {
            return format!(
                "session {child} is a sub-agent — archive its parent session to archive it"
            );
        };
        if self.sessions.lock().contains_key(parent) {
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
                .and_then(|()| store.title().map(str::to_string));
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
        base_prompt: Option<String>,
    ) -> Result<SessionMeta, ProtocolError> {
        if let Some(t) = &title {
            check_title_len(t)?;
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
            _ => Self::fresh_session_name(workspace),
        };
        store.set_title(&title).map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        self.build_live(workspace, store, Some(title), created, base_prompt)
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
        // #267: a turn that died between writing the assistant's calls and
        // writing their results leaves the transcript's last entry an
        // assistant with uncovered `call_id`s — the invariant "every call
        // gets a tool entry" is broken and the model would re-see an
        // open call. Settle it at the open boundary: mint the missing
        // results as errors, so a reopen heals the record before the next
        // turn assembles its context. A healthy file (a tool entry after
        // the last assistant) matches nothing and stays byte-identical.
        repair_dangling_tool_calls(&mut store)?;
        let title = store.title().map(str::to_string);
        let created = store.created();
        self.build_live(workspace, store, title, created, None)
    }

    /// The shared live-registration path (fresh create and re-open): the
    /// constructor builds the session around the given store (R2); this
    /// keeps the provider the live shell binds and the registration.
    pub(crate) fn build_live(
        &self,
        workspace: &Workspace,
        mut store: SessionStore,
        title: Option<String>,
        created: u64,
        base_prompt: Option<String>,
    ) -> Result<SessionMeta, ProtocolError> {
        let parent = store.parent().map(str::to_string);
        let config = self.workspace_config(workspace);
        // #336: the endpoint follows the model this session opens on — its
        // record's last switch, else the config default — and the provider its
        // id names, never the provider the config's map sorts first.
        let opened = session_open::facts(&mut store)?;
        let route = open_route(&config, opened.as_ref())?;
        let self_arc = self.self_arc().expect("session_new on a built core");
        // The live shell's provider (the stream-forwarding seam); the
        // constructor takes the same resolved provider for the supervisor's
        // child factory (one resolution, ticket #43).
        let inner = self
            .root_provider
            .clone()
            .unwrap_or_else(|| session_inner(&self.client, &route.provider, &config.requests));
        let provider = Arc::new(ForwardingProvider::new(
            inner,
            self.events_tx.clone(),
            self.pipe.clone(),
            workspace.id.clone(),
            store.id().to_owned(),
            Arc::new(AtomicBool::new(false)),
        ));
        let agent = AgentSession::launch(
            store,
            SessionRole::Root {
                core: self_arc,
                workspace: workspace.clone(),
                config,
                provider: provider.clone(),
                base_prompt,
                route,
            },
        )?;
        let meta = SessionMeta {
            id: provider.session.clone(),
            workspace: workspace.id.clone(),
            title,
            parent,
            created,
            leaf: None,
            model: Some(agent.model()),
            usage: None,
            archived: false,
        };
        let live = Arc::new(LiveSession {
            meta: Mutex::new(meta.clone()),
            agent,
            stop: provider.stop.clone(),
            turn: AtomicBool::new(false),
            provider,
            cwd: PathBuf::from(&workspace.cwd),
            skeleton: Mutex::new(Skeleton::default()),
        });
        let id = live.meta.lock().id.clone();
        self.sessions.lock().insert(id.clone(), live.clone());
        Ok(meta)
    }
}

/// The provider set's first entry (`BTreeMap` order): the address a session
/// starts from, before any model id is routed. `None` is the config error the
/// GUI shows — a workspace with no provider can open no session.
fn first_provider(config: &Config) -> Result<ProviderRef<'_>, ProtocolError> {
    config
        .providers
        .iter()
        .next()
        .map(|(name, provider)| ProviderRef { name, provider })
        .ok_or_else(|| ProtocolError::Other {
            message: "no providers configured; add a [providers.x] section".into(),
        })
}

/// `generation.default_model`, taken by its §6.7 route: a default written with
/// its provider's prefix names that provider; one written bare is taken where
/// it is declared. An empty default, a prefix naming no provider, and a key no
/// provider declares are all the config's empty-value contract — the first
/// provider's first model.
fn default_model(config: &Config, first: ProviderRef<'_>) -> Result<String, ProtocolError> {
    let default = config.generation.default_model.as_str();
    if !default.is_empty()
        && let Ok(owner) = config.route_model(default, first)
        && owner
            .provider
            .models
            .contains_key(Config::model_key_for(owner.name, default))
    {
        return Ok(default.to_owned());
    }
    first
        .provider
        .models
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| ProtocolError::Other {
            message: format!("provider {} has no models", first.name),
        })
}

/// The model a session opens on and the provider that owns it (#336): the model
/// its record last named, else the config's default, routed by its id — so the
/// endpoint, the model's facts, and the request all answer to the one provider
/// the id names, not the provider the config's map sorts first.
pub(crate) fn open_route(
    config: &Config,
    record: Option<&Diagnostics>,
) -> Result<Route, ProtocolError> {
    let first = first_provider(config)?;
    let model = record
        .and_then(|r| r.model.clone())
        .map_or_else(|| default_model(config, first), Ok)?;
    let owner = config.route_model(&model, first)?;
    Ok(Route {
        model,
        name: owner.name.to_owned(),
        provider: owner.provider.clone(),
    })
}

/// The provider a model switch is addressed at (#336). `previous` is the model
/// the session carries: a bare key is looked up against the provider it is
/// already on (the prefix is what disambiguates between providers, so a key
/// several providers declare stays where it is), and an id whose prefix names no
/// configured provider is refused — it never travels silently to whichever
/// provider the config's map sorts first.
pub(crate) fn switch_route<'c>(
    config: &'c Config,
    model: &str,
    previous: &str,
) -> Result<ProviderRef<'c>, ProtocolError> {
    let first = first_provider(config)?;
    let current = config.provider_for(previous).unwrap_or(first);
    config
        .route_model(model, current)
        .map_err(|unowned| ProtocolError::Other {
            message: format!("model {model}: {unowned}"),
        })
}

/// #267: mint `status: "error"` tool entries for a dangling assistant — one
/// whose `calls` have no following `EntryKind::Tool` entry per `call_id` (the turn
/// died mid-execution). Runs at session re-open, before the store is
/// handed to `build_live`, so the repaired file is what every later turn
/// reads.
fn repair_dangling_tool_calls(store: &mut SessionStore) -> Result<(), ProtocolError> {
    use tau_protocol::payload::{AssistantPayload, ToolOutput, ToolPayload};

    let Some(leaf) = store.leaf().map_err(|e| ProtocolError::Other {
        message: e.to_string(),
    })?
    else {
        // A fresh file has no entries; nothing to repair.
        return Ok(());
    };
    let leaf = leaf.clone();
    if leaf.kind != EntryKind::Assistant {
        return Ok(());
    }

    let Ok(payload) = serde_json::from_value::<AssistantPayload>(leaf.payload.clone()) else {
        return Ok(());
    };
    if payload.calls.is_empty() {
        return Ok(());
    }
    let after = store
        .entries_since(&leaf.id)
        .map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
    let covered: std::collections::HashSet<&str> = after
        .iter()
        .filter(|e| e.kind == EntryKind::Tool)
        .filter_map(|e| e.payload.get("call_id").and_then(Value::as_str))
        .collect();
    for call in &payload.calls {
        if covered.contains(call.call_id.as_str()) {
            continue;
        }
        let entry_id = store.mint_id();
        store
            .append_entry(
                &entry_id,
                EntryKind::Tool,
                ToolPayload {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    args: Value::Null,
                    output: ToolOutput::Text(
                        "turn was interrupted before tool execution completed".to_owned(),
                    ),
                    ui_detail: None,
                    status: Some("error".to_owned()),
                }
                .to_value(),
                Some(&leaf.id),
            )
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?;
    }
    Ok(())
}

/// The session's per-turn provider options (spec §12, #35): sampling from
/// `generation`, model-specific facts (output cap, context window, reasoning
/// level) from the model's entry — a model with no facts falls back to the
/// global values. `provider` is the one the model's id routes to (#336), so a
/// model declared on the second provider resolves its facts, not a gate gap.
pub(crate) fn derive_turn(
    config: &Config,
    provider: &crate::config::Provider,
    model: &str,
    session: &str,
) -> TurnConfig {
    let key = config.model_key(model);
    let def = provider.models.get(key);
    let level = config
        .thinking
        .levels
        .get(key)
        .copied()
        .unwrap_or(config.thinking.level);
    let reasoning = match def.and_then(|d| d.reasoning) {
        // A model recorded as non-reasoning never carries the effort: the
        // server would reject it.
        Some(false) => None,
        _ => level.into(),
    };
    TurnConfig {
        max_output_tokens: thinking_adjusted_max_output(
            def.and_then(|d| d.max_tokens)
                .or(config.generation.max_tokens),
            def.and_then(|d| d.max_tokens),
            level,
            &config.thinking.budgets,
        ),
        reasoning,
        reasoning_summary: config.thinking.summary,
        temperature: config.generation.temperature,
        top_p: config.generation.top_p,
        frequency_penalty: config.generation.frequency_penalty,
        presence_penalty: config.generation.presence_penalty,
        prompt_cache: (config.cache.retention != crate::config::CacheRetention::None)
            .then(|| (session.to_owned(), config.cache.retention)),
        context_window: def.and_then(|d| d.context_window),
        image_max_bytes: Some(config.limits.image.max_bytes),
        image_max_pixels: Some(config.limits.image.max_pixels),
        // Resolved here, applied at the request: the id the session carries and
        // the key the endpoint is addressed by (§6.7).
        address: Some(ModelAddress {
            id: model.to_owned(),
            key: key.to_owned(),
        }),
    }
}

/// A session entry as its file-line view (ADR-0008): the object the
/// upsert stream carries and the GUI's id-keyed map stores.
pub(crate) fn entry_to_view(entry: &crate::session::Entry) -> ViewEntry {
    ViewEntry {
        id: entry.id.clone(),
        parent: entry.parent.clone(),
        kind: entry.kind.clone(),
        timestamp: entry.timestamp,
        payload: entry.payload.clone(),
        blob: entry
            .blob
            .as_ref()
            .map(|b| tau_protocol::snapshot::BlobRef {
                id: b.id.clone(),
                size: b.size,
                hash: b.hash.clone(),
            }),
        first_kept: entry.first_kept_entry_id.clone(),
    }
}

/// The title's cap, shared by new and rename: the archive listing reads
/// the header line bounded, so an oversized title would drop the entry
/// from the list. Both call sites run it before any mutation.
pub(crate) fn check_title_len(title: &str) -> Result<(), ProtocolError> {
    if title.chars().count() > MAX_TITLE_LEN {
        return Err(ProtocolError::Other {
            message: format!(
                "title is {} characters — the maximum is {MAX_TITLE_LEN}",
                title.chars().count()
            ),
        });
    }
    Ok(())
}

/// branch) and the delete: the same guard as the archive (ADR-0005).
pub(crate) fn running_turn_refusal(session: &str) -> ProtocolError {
    ProtocolError::Other {
        message: format!("session {session} is running — stop it first"),
    }
}
