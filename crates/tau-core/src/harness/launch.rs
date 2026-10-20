//! The session constructor (R2): one path from a session store to a live
//! `AgentSession`. The role carries the wiring defaults the five
//! construction sites used to assemble by hand — the bridge↔supervisor
//! cycle, the parameter assembly, and the post-construction hook wiring
//! (kind-string → event shaping) all live here, not at the call sites.

use super::{
    AgentSession, Arc, ChildProviderFactory, Config, Core, Event, ForwardingChildFactory, HashMap,
    Mutex, OmStatusKind, Path, PathBuf, ProtocolError, SessionParams, SessionStore,
    SessionSubagentBridge, SubagentBridge, Supervisor, SupervisorParams, TurnChildDriver,
    TurnConfig, Workspace, context, derive_turn, entry_to_view, last_model_note, tools,
};
use crate::config::ToolBatchPolicy;
use crate::om::OmRecord;
use crate::om_integration::OmState;
use crate::provider::{ToolSpec, TurnProviderRef};
use crate::session::Entry;

/// The session's role: the wiring defaults `launch` applies.
#[derive(Clone)]
pub enum SessionRole {
    /// A top-level session. With the core (and no pre-built supervisor)
    /// the constructor builds the supervisor from the config and wires the
    /// full event set; with a pre-built supervisor (the test seam) it
    /// adopts the supervisor's values and wires no events.
    Root {
        core: Option<Arc<Core>>,
        workspace: Option<Workspace>,
        config: Option<Config>,
        provider: TurnProviderRef,
        supervisor: Option<Arc<Supervisor>>,
        /// The parent's own system prompt when it differs from the
        /// supervisor's (the acceptance driver's seam: the supervisor's
        /// prompt is the child's); `None` = adopt the supervisor's.
        system_prompt: Option<String>,
        /// Replace the root session's base prompt line (the headless frame
        /// seam, `session/new` `base_prompt`); `None` keeps the default.
        base_prompt: Option<String>,
        /// The config's first provider, resolved by the caller (`build_live`
        /// resolves it for the live shell); the supervisor's child factory
        /// and bridge take it — no second resolution (ticket #43).
        first_provider: Option<(String, crate::config::Provider)>,
    },
    /// A sub-agent session: the child's link; no supervisor, no events. The
    /// values the child inherits (cwd, turn config, OM) come from the
    /// supervisor.
    Child {
        supervisor: Arc<Supervisor>,
        provider: TurnProviderRef,
        system_prompt: String,
        model: String,
        tools: Vec<ToolSpec>,
        record: OmRecord,
        handle: String,
    },
    /// A bare session (the test kits): a canned provider; no supervisor,
    /// no child link, no events.
    Bare {
        provider: TurnProviderRef,
        system_prompt: String,
        model: String,
        tools: Vec<ToolSpec>,
        cwd: PathBuf,
        turn: TurnConfig,
        tool_batch_on_force: ToolBatchPolicy,
        /// The OM state; `None` = OM disabled (ticket #43: the acceptance
        /// driver's live-om suite carries its own).
        om: Option<OmState>,
        /// The OM model; empty = the session's model.
        om_model: String,
        /// The completion gate (spec §5.3); `None` = no gate.
        gates: Option<crate::config::Gates>,
    },
}

impl AgentSession {
    /// The in-crate construction path (R2): assembles the session for its
    /// role — provider, prompt, turn config, OM state, supervisor, child
    /// link, event hooks — so no call site spreads the parameters. Returns
    /// the `Arc` because the call sites all keep an `Arc` (the root's
    /// queue hook captures a `Weak` self-reference, not a strong one).
    #[allow(clippy::too_many_lines)] // one arm per session role; splitting is refactoring
    pub fn launch(
        store: SessionStore,
        role: SessionRole,
    ) -> Result<Arc<AgentSession>, ProtocolError> {
        match role {
            SessionRole::Bare {
                provider,
                system_prompt,
                model,
                tools,
                cwd,
                turn,
                tool_batch_on_force,
                om,
                om_model,
                gates,
            } => {
                // The plan file (design: session-scoped, the source of truth):
                // seeded at start. The bare role (test kits) has no core to seed through.
                crate::plan::seed(&cwd, store.id());
                let mut system_prompt = system_prompt;
                crate::gate::with_section(&mut system_prompt, &gates);
                Ok(Arc::new(Self::new(SessionParams {
                    store,
                    system_prompt,
                    model,
                    tools,
                    cwd,
                    provider,
                    tool_batch_on_force,
                    turn,
                    gates,
                    gate_failed: false,
                    om,
                    om_model,
                    // No `Config` in scope for the bare role (test kits):
                    // the default GC policy.
                    hashline: crate::hashline::pin_store::GcPolicy::default(),
                    subagents: None,
                    child: None,
                })))
            }
            SessionRole::Child {
                supervisor,
                provider,
                system_prompt,
                model,
                tools,
                record,
                handle,
            } => {
                let d = supervisor.inherited();
                // The plan file (design: session-scoped, the source of
                // truth): the child's own file, replacing any inherited section.
                let sid = store.id().to_owned();
                let core = supervisor.core().and_then(|w| w.upgrade());
                let (system_prompt, core) =
                    plan_wiring(core.as_ref(), &system_prompt, &d.cwd, &sid);
                // Sender labels (ADR-0001 supplement): the child's model
                // context decorates user entries with `Parent:` / `User:`
                // at assembly time; the prompt names the convention.
                let system_prompt = format!(
                    "{system_prompt}\n\n\
                     Messages you receive are labeled by sender: \
                     `Parent:` is the parent model directing you, `User:` \
                     is a human, and an unlabeled message is your own task \
                     (the spawn brief).",
                );
                let agent = Self::new(SessionParams {
                    store,
                    system_prompt,
                    model,
                    tools,
                    cwd: d.cwd,
                    provider,
                    tool_batch_on_force: d.tool_batch_on_force,
                    turn: d.turn,
                    gates: d.gates,
                    gate_failed: false,
                    om: Some(OmState::from_config(&d.om, record)),
                    om_model: d.om_model,
                    hashline: d.hashline,
                    subagents: None,
                    child: Some(Arc::new(crate::subagent::ChildLink { supervisor, handle })),
                });
                // The plan-state re-injection (design: turn boundaries and
                // post-compaction): the core's watcher-parsed state.
                wire_plan_hook(&agent, core, &sid);
                Ok(Arc::new(agent))
            }
            SessionRole::Root {
                core,
                workspace,
                config,
                provider,
                supervisor,
                system_prompt,
                base_prompt,
                first_provider,
            } => {
                let Some(sup) = supervisor else {
                    // Building the supervisor (the bridge cycle) needs the core's
                    // seams; a pre-built one (the test seam) needs none.
                    let (Some(core), Some(workspace), Some(config)) = (core, workspace, config)
                    else {
                        return Err(ProtocolError::Other {
                            message:
                                "root role: the core, workspace, and config are needed to build the supervisor"
                                    .into(),
                        });
                    };
                    let Some(first) = first_provider else {
                        return Err(ProtocolError::Other {
                            message: "no providers configured; add a [providers.x] section".into(),
                        });
                    };
                    return root_session(
                        &core,
                        &workspace,
                        &config,
                        &first,
                        store,
                        provider,
                        base_prompt.as_deref(),
                    );
                };
                // A pre-built supervisor (the test seam): adopt its values,
                // wire no events.
                Self::root_with_supervisor(&sup, store, system_prompt, provider, None)
            }
        }
    }

    /// A pre-built supervisor (the test seam): adopt the supervisor's values,
    /// wire no events.
    fn root_with_supervisor(
        sup: &Arc<Supervisor>,
        mut store: SessionStore,
        system_prompt: Option<String>,
        provider: TurnProviderRef,
        _base_prompt: Option<String>,
    ) -> Result<Arc<AgentSession>, ProtocolError> {
        let d = sup.inherited();
        let record = OmState::load_record(&mut store).map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?;
        // The plan file (design): the prompt's section and the state
        // re-injection exist only when the caller passes a prompt and
        // the supervisor carries a core (the test seams carry none).
        let sid = store.id().to_owned();
        let (system_prompt, core) = match system_prompt {
            Some(prompt) => {
                let core = sup.core().and_then(|w| w.upgrade());
                plan_wiring(core.as_ref(), &prompt, &d.cwd, &sid)
            }
            None => (d.system_prompt.clone(), None),
        };
        let gate_failed = resume_gate_failed(&mut store)?;
        let agent = Arc::new(Self::new(SessionParams {
            store,
            system_prompt,
            model: d.model,
            tools: tools::surface::root_specs(),
            cwd: d.cwd,
            provider,
            tool_batch_on_force: d.tool_batch_on_force,
            turn: d.turn,
            gates: d.gates,
            gate_failed,
            om: Some(OmState::from_config(&d.om, record)),
            om_model: d.om_model,
            hashline: d.hashline,
            subagents: Some(sup.clone()),
            child: None,
        }));
        wire_plan_hook(&agent, core, &sid);
        sup.attach_parent(&agent);
        Ok(agent)
    }
}

/// The plan-state re-injection hook (design: turn boundaries and
/// post-compaction): the core's watcher-parsed state, upgraded per call.
/// `None` (a core-less test seam) wires nothing.
fn wire_plan_hook(agent: &AgentSession, core: Option<Arc<Core>>, sid: &str) {
    if let Some(core) = core {
        let core = Arc::downgrade(&core);
        let sid = sid.to_owned();
        agent.set_plan_hook(Some(Arc::new(move || {
            core.upgrade().and_then(|core| core.plan_state(&sid))
        })));
    }
}

/// The gate state a reopened session resumes from the record (spec §5.3):
/// the newest terminal gate outcome on the branch.
fn resume_gate_failed(store: &mut SessionStore) -> Result<bool, ProtocolError> {
    crate::gate::initial_gate_failed(store).map_err(|e| ProtocolError::Other {
        message: e.to_string(),
    })
}

/// The plan wiring the supervisor-linked roles share (design: the plan
/// The plan wiring every session role shares (design: the plan file is
/// the source of truth): seed the session's file, point the prompt at the
/// session's own file (replacing any inherited section), and — when a
/// core is attached — observe the file's current state so the cache is
/// warm for the first assembly. A core-less seam (the test kits) passes
/// the prompt through with only the path section.
fn plan_wiring(
    core: Option<&Arc<Core>>,
    prompt: &str,
    cwd: &Path,
    sid: &str,
) -> (String, Option<Arc<Core>>) {
    crate::plan::seed(cwd, sid);
    // The section names the session's own plan file (seeded
    // unconditionally above): it belongs in the prompt whether or not a
    // core is attached — the core only adds the state cache on top.
    let sectioned = crate::plan::with_path(prompt, &crate::plan::path(cwd, sid));
    let core = core.cloned();
    if let Some(core) = &core {
        core.refresh_plan(cwd, sid);
    }
    (sectioned, core)
}

/// The root assembly: the model resolution, the prompt, the
/// bridge↔supervisor cycle, the session, and the four event hooks. The
/// first provider arrives pre-resolved — the caller already resolved it
/// (ticket #43).
#[allow(clippy::too_many_lines)] // one root-assembly path; splitting is refactoring
fn root_session(
    core: &Arc<Core>,
    workspace: &Workspace,
    config: &Config,
    first: &(String, crate::config::Provider),
    mut store: SessionStore,
    provider: TurnProviderRef,
    base_prompt: Option<&str>,
) -> Result<Arc<AgentSession>, ProtocolError> {
    let (name, first) = first;
    // `generation.default_model` wins; an empty or unknown value falls
    // back to the provider's first model (BTreeMap order).
    let default = config.generation.default_model.clone();
    let model = if !default.is_empty() && first.models.contains_key(&default) {
        default
    } else {
        first
            .models
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| ProtocolError::Other {
                message: format!("provider {name} has no models"),
            })?
    };
    // A session that picked a non-default model (session_set_model) keeps
    // it across close and re-open: the file's last `model:` note is the
    // record (a fresh session has none and takes the provider default).
    let model = last_model_note(&mut store).unwrap_or(model);
    // The turn's provider options (spec §12, #35): resolved from the
    // merged config + this model's facts, once, at registration.
    let turn = derive_turn(config, first, &model, store.id());
    let cwd = PathBuf::from(&workspace.cwd);

    // The per-session OM record (ticket #22): reconstructed from the
    // file on open; a fresh session starts with the default record.
    let record = OmState::load_record(&mut store).map_err(|e| ProtocolError::Other {
        message: e.to_string(),
    })?;
    let gate_failed = resume_gate_failed(&mut store)?;

    // The loop assembles no context of its own: base prompt + context
    // files (spec §10) are built here, once, at session creation.
    let layers = context::discover(&cwd, &core.system_dir_of());
    let system_prompt = context::assemble(
        base_prompt.unwrap_or("You are Tau, a coding agent."),
        &layers,
    );

    // The plan file (design: the source of truth): seeded at session
    // start (create-if-missing — a re-opened session keeps the model's
    // plan), its exact path injected into the prompt, and the parsed
    // state refreshed so the first assembly carries it. The section
    // lands in the supervisor's prompt too: a spawned child inherits it,
    // then replaces it with its own path. It lands before the skills
    // catalog: the catalog is the last layer (tests_skills).
    let sid = store.id().to_owned();
    let (mut system_prompt, _) = plan_wiring(Some(core), &system_prompt, &cwd, &sid);
    crate::gate::with_section(&mut system_prompt, &config.gates);

    // Skills (tickets #28/#31): discovery runs at session open/reopen
    // — the catalog is frozen at that moment (a running session's prompt
    // is not re-derived; new sessions pick up watcher changes at their
    // own build). The catalog is the last layer — after the context
    // files, so a user AGENTS.md is never drowned — and a spawned child
    // inherits it through this prompt.
    let skills = core.refresh_skills(workspace);
    if let Some(catalog) = crate::skills::catalog(&skills) {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&catalog);
    }

    // The bridge and the supervisor reference each other: build the
    // bridge with an empty weak and patch it in after construction.
    // The child factory: a custom (test) one, or the default forwarding one.
    // The default is kept concrete so the bridge can share its per-child
    // provider with the child's agent (one provider, as the root has — #175).
    let (bridge_factory, factory): (
        Option<Arc<ForwardingChildFactory>>,
        Arc<dyn ChildProviderFactory>,
    ) = if let Some(custom) = core.child_factory.clone() {
        (None, custom)
    } else {
        let forwarding = Arc::new(ForwardingChildFactory {
            client: core.client.clone(),
            provider: first.clone(),
            requests: config.requests.clone(),
            tx: core.events_tx.clone(),
            pipe: core.pipe.clone(),
            workspace: workspace.id.clone(),
            providers: Mutex::new(HashMap::new()),
        });
        (Some(Arc::clone(&forwarding)), forwarding)
    };
    let bridge = Arc::new(SessionSubagentBridge {
        core: Arc::clone(core),
        workspace: workspace.id.clone(),
        client: core.client.clone(),
        provider: first.clone(),
        requests: config.requests.clone(),
        sup: Mutex::new(None),
        factory: bridge_factory,
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: store.id().to_owned(),
        cwd: cwd.clone(),
        provider: factory,
        model: model.clone(),
        system_prompt: system_prompt.clone(),
        om: config.om.clone(),
        om_model: config.om.om_model.clone(),
        tool_batch_on_force: config.requests.tool_batch_on_force,
        turn: turn.clone(),
        gates: config.gates.clone(),
        hashline: crate::hashline::pin_store::GcPolicy {
            retention_days: config.hashline.retention_days,
            gc_on_start: config.hashline.gc_on_start,
        },
        caps: config.subagents.clone(),
        types: crate::agent_type::discover(core.system_dir.as_deref(), &cwd),
        depth: 0,
        bridge: subagent_bridge,
        driver: Arc::new(TurnChildDriver {
            core: Arc::downgrade(core),
        }),
        core: Some(Arc::downgrade(core)),
    });
    bridge
        .sup
        .lock()
        .expect("supervisor bridge: no panic while the lock is held")
        .replace(Arc::downgrade(&sup));
    let sid = store.id().to_owned();
    let agent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt,
        model,
        tools: tools::surface::root_specs(),
        cwd,
        provider,
        tool_batch_on_force: config.requests.tool_batch_on_force,
        turn,
        gates: config.gates.clone(),
        gate_failed,
        om: Some(OmState::from_config(&config.om, record)),
        om_model: config.om.om_model.clone(),
        hashline: crate::hashline::pin_store::GcPolicy {
            retention_days: config.hashline.retention_days,
            gc_on_start: config.hashline.gc_on_start,
        },
        subagents: Some(sup.clone()),
        child: None,
    }));
    wire_events(core, &workspace.id, &sid, &agent);
    sup.attach_parent(&agent);
    Ok(agent)
}

/// The root's four event hooks: the `om_status` kind-string shaping, the
/// queue projection, and the two entry tees (ADR-0008's file-line
/// upserts).
pub(super) fn wire_events(
    core: &Arc<Core>,
    workspace_id: &str,
    sid: &str,
    agent: &Arc<AgentSession>,
) {
    // The plan-state hook (design: the plan file is the source of
    // truth): the core's watcher-parsed state, re-injected at each
    // assembly. A `Weak` keeps the hook from pinning the core.
    {
        let core_weak = Arc::downgrade(core);
        let sid = sid.to_owned();
        agent.set_plan_hook(Some(Arc::new(move || {
            core_weak.upgrade().and_then(|core| core.plan_state(&sid))
        })));
    }
    // The core's om_status hook takes the kind string; this closure
    // shapes it into the protocol event on the shared channel.
    {
        let tx = core.events_tx.clone();
        let ws = workspace_id.to_owned();
        let sid = sid.to_owned();
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
    {
        let tx = core.events_tx.clone();
        let ws = workspace_id.to_owned();
        let sid = sid.to_owned();
        // The queue and upsert hooks get their own clones (the entry
        // hook moves these).
        let q_tx = tx.clone();
        let q_ws = ws.clone();
        let q_sid = sid.clone();
        let q_agent = Arc::downgrade(agent);
        agent.set_queue_event_hook(Some(Arc::new(move || {
            let q_agent = q_agent
                .upgrade()
                .expect("the queue hook is stored in the session it captures");
            let _ = q_tx.try_send(Event::Queue {
                workspace: q_ws.clone(),
                session: q_sid.clone(),
                items: q_agent.queued_items(),
            });
        })));
        // The wire-only re-emissions (the streaming assistant, the
        // tool's call phase) take the same mapping — same event, same
        // channel.
        let u_tx = tx.clone();
        let u_ws = ws.clone();
        let u_sid = sid.clone();
        agent.set_entry_upsert_hook(Some(Arc::new(move |entry: &Entry| {
            let _ = u_tx.try_send(Event::EntryUpsert {
                workspace: u_ws.clone(),
                session: u_sid.clone(),
                entry: entry_to_view(entry),
            });
        })));
        agent.set_entry_event_hook(Some(Arc::new(move |entry: &Entry| {
            let _ = tx.try_send(Event::EntryUpsert {
                workspace: ws.clone(),
                session: sid.clone(),
                entry: entry_to_view(entry),
            });
        })));
    }
}
