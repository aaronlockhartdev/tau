//! The session constructor (R2): one path from a session store to a live
//! `AgentSession`. The role carries the wiring defaults the five
//! construction sites used to assemble by hand — the bridge↔supervisor
//! cycle, the parameter assembly, and the post-construction hook wiring
//! (kind-string → event shaping) all live here, not at the call sites.

use super::{
    AgentSession, Arc, ChildProviderFactory, Config, Core, Event, ForwardingChildFactory, HashMap,
    Mutex, OmStatusKind, Ordering, Path, PathBuf, ProtocolError, SessionParams, SessionStore,
    SessionSubagentBridge, SubagentBridge, Supervisor, SupervisorParams, TurnChildDriver,
    TurnConfig, ViewEntry, Workspace, context, derive_turn, entry_to_view, session_open, tools,
};
use crate::agent::{OmRoute, StreamDelta};
use crate::config::{ProviderRef, Route, ToolBatchPolicy};
use crate::om::OmRecord;
use crate::om_integration::OmState;
use crate::provider::{ToolSpec, TurnProviderRef};
use crate::session::Entry;
use crate::subagent::Inherited;

/// The session's role: the wiring defaults `launch` applies.
#[derive(Clone)]
pub enum SessionRole {
    /// A top-level session: the constructor builds the supervisor from the
    /// config, wires the full event set, and resolves the model. A root without
    /// a core is not a half-built root — it is the other role, `Seam`.
    Root {
        core: Arc<Core>,
        workspace: Workspace,
        config: Config,
        provider: TurnProviderRef,
        /// Replace the root session's base prompt line (the headless frame
        /// seam, `session/new` `base_prompt`); `None` keeps the default.
        base_prompt: Option<String>,
        /// The model this session opens on and the provider its id names,
        /// resolved by the caller (#336): `build_live` takes the record's last
        /// switch, else the config default, and routes that id. The endpoint,
        /// the model's facts, and the child factory all follow this route —
        /// never the provider the config's map sorts first. Resolved once for
        /// the live shell (ticket #43's one-resolution rule).
        route: Route,
    },
    /// A session around a caller-built supervisor, for tests that drive a
    /// parent without a core. It skips what a real open does: model resolution,
    /// config layering and context files, the skills catalogue, the bridge, the
    /// event wiring, and the open audit record. Production opens use `Root`.
    Seam {
        supervisor: Arc<Supervisor>,
        provider: TurnProviderRef,
        /// The parent's own prompt when it differs from the supervisor's (the
        /// supervisor's is the child's); `None` = adopt the supervisor's.
        system_prompt: Option<String>,
    },
    /// A sub-agent session: the child's link; no supervisor, no events. The
    /// values the child inherits (cwd, OM) come from the supervisor; the
    /// parent's current model and its facts arrive as the spawn's snapshot of
    /// the inherited pair, so a parent's move mid-spawn cannot split the
    /// child's view (#336).
    Child {
        supervisor: Arc<Supervisor>,
        provider: TurnProviderRef,
        system_prompt: String,
        model: String,
        tools: Vec<ToolSpec>,
        record: OmRecord,
        handle: String,
        /// The parent's turn options, as the spawn's snapshot of the pair saw
        /// them (#336). A type naming its own model gates with these until its
        /// first turn boundary re-derives them from its own model — S18.
        turn: TurnConfig,
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
        /// Where the memory passes run (#348); the test kits pass
        /// `OmRoute::Session`, which rides the session's own model.
        om_route: OmRoute,
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
        mut store: SessionStore,
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
                om_route,
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
                    om_route,
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
                turn,
            } => {
                let Inherited {
                    cwd,
                    om,
                    gates,
                    tool_batch_on_force,
                    hashline,
                    ..
                } = supervisor.inherited();
                // The plan file (design: session-scoped, the source of
                // truth): the child's own file, replacing any inherited section.
                let sid = store.id().to_owned();
                let core = supervisor.core().and_then(|w| w.upgrade());
                let (system_prompt, core) = plan_wiring(core.as_ref(), &system_prompt, &cwd, &sid);
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
                // The child rides its own model until its first turn boundary,
                // where the live config re-derives the route against the
                // provider its own model names (#348).

                let agent = Self::new(SessionParams {
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
                    om: Some(OmState::from_config(&om, record)),
                    om_route: OmRoute::Session,
                    hashline,
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
                base_prompt,
                route,
            } => root_session(
                &core,
                &workspace,
                &config,
                &route,
                store,
                provider,
                base_prompt.as_deref(),
            ),
            SessionRole::Seam {
                supervisor,
                provider,
                system_prompt,
            } => {
                let d = supervisor.inherited();
                let record =
                    OmState::load_record(&mut store).map_err(|e| ProtocolError::Other {
                        message: e.to_string(),
                    })?;
                // The plan file (design): the prompt's section and the state
                // re-injection exist only when the caller passes a prompt and
                // the supervisor carries a core (the seam carries none).
                let sid = store.id().to_owned();
                let (system_prompt, core) = match system_prompt {
                    Some(prompt) => {
                        let core = supervisor.core().and_then(|w| w.upgrade());
                        plan_wiring(core.as_ref(), &prompt, &d.cwd, &sid)
                    }
                    None => (d.system_prompt.clone(), None),
                };
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
                    // No check to have failed: the seam's gate rides the
                    // supervisor it adopts, and no fixture gives one a command.
                    gate_failed: false,
                    om: Some(OmState::from_config(&d.om, record)),
                    // The seam has no core to route with (the test seam): its
                    // passes ride the model it is built on (#348).
                    om_route: OmRoute::Session,
                    hashline: d.hashline,
                    subagents: Some(supervisor.clone()),
                    child: None,
                }));
                wire_plan_hook(&agent, core, &sid);
                supervisor.attach_parent(&agent);
                Ok(agent)
            }
        }
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
        core.refresh_plan(cwd, sid, true);
    }
    (sectioned, core)
}

/// The root assembly: the prompt, the bridge↔supervisor cycle, the session,
/// and the event hooks. The model and the provider its id names arrive
/// pre-resolved: `build_live` routes once for the live shell (#336, on ticket
/// #43's one-resolution rule).
#[allow(clippy::too_many_lines)] // one root-assembly path; splitting is refactoring
fn root_session(
    core: &Arc<Core>,
    workspace: &Workspace,
    config: &Config,
    route: &Route,
    mut store: SessionStore,
    provider: TurnProviderRef,
    base_prompt: Option<&str>,
) -> Result<Arc<AgentSession>, ProtocolError> {
    // The open walks the session file twice. `build_live` projected it once to
    // route the model (#336); this projection answers the two questions only
    // the assembly asks: the gate state to resume, and the records this open
    // owes (`record_at_open` below, spec §8). The repeat is cheap: a loaded
    // store serves `branch` from the in-memory log `open` filled
    // (`session/log.rs`), so the second projection is a list walk, not a
    // re-parse of the file. Its model is the one `build_live` routed.
    let facts = session_open::facts(&mut store)?;
    // The turn's provider options (spec §12, #35): resolved from the merged
    // config and this model's facts on the provider that declares them, once,
    // at registration.
    let model = route.model.as_str();
    let turn = derive_turn(config, &route.provider, model, store.id());

    // Where the memory passes run (#348), from the config this open read: a
    // session with no `[om].om_model` rides the model above; each pass stamps
    // the model it addressed into the memory record, so the record names the
    // endpoint the memory runs on, configured or ridden.
    let om_route = core.om_route(
        config,
        ProviderRef {
            name: &route.name,
            provider: &route.provider,
        },
    );

    let cwd = PathBuf::from(&workspace.cwd);

    // The per-session OM record (ticket #22): reconstructed from the
    // file on open; a fresh session starts with the default record.
    let record = OmState::load_record(&mut store).map_err(|e| ProtocolError::Other {
        message: e.to_string(),
    })?;
    let gate_failed = facts.as_ref().is_some_and(|f| f.gate_failed);
    session_open::record_at_open(&mut store, facts.as_ref(), model, &turn)?;

    // The loop assembles no context of its own: base prompt + context
    // files (spec §10) are built here, once, at session creation.
    let files = context::resolve_prompt_files(&cwd, core.home.as_deref());
    // Precedence (ticket #77): an explicit `base_prompt` (the ACP headless
    // frame) wins, then a resolved `SYSTEM.md`, then the default frame.
    let system_prompt = context::assemble(
        base_prompt
            .or(files.system.as_deref())
            .unwrap_or("You are Tau, a coding agent."),
        &context::discover(&cwd, &core.system_dir_of()),
        files.append.as_ref(),
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
            config: config.clone(),
            provider: route.provider.clone(),
            provider_name: route.name.clone(),
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
        provider: route.provider.clone(),
        requests: config.requests.clone(),
        sup: Mutex::new(None),
        factory: bridge_factory,
    });
    let subagent_bridge: Arc<dyn SubagentBridge> = bridge.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: store.id().to_owned(),
        cwd: cwd.clone(),
        provider: factory,
        model: model.to_owned(),
        system_prompt: system_prompt.clone(),
        om: config.om.clone(),
        tool_batch_on_force: config.requests.tool_batch_on_force,
        turn: turn.clone(),
        gates: config.gates.clone(),
        hashline: gc_policy(config),
        caps: config.subagents.clone(),
        types: crate::agent_type::discover(core.system_dir.as_deref(), &cwd),
        depth: 0,
        bridge: subagent_bridge,
        driver: Arc::new(TurnChildDriver {
            core: Arc::downgrade(core),
        }),
        core: Some(Arc::downgrade(core)),
        // The whole log by design (#302): these are the spawn-provenance
        // records of this session's children, and a spawn record lives on the
        // branch that was current when the child spawned — a parent that
        // forked afterwards leaves that record off the active chain.
        parent_entries: Some(
            store
                .entries_range_cached(0, usize::MAX)
                .unwrap_or_default()
                .into_iter()
                .map(|(e, _)| e)
                .collect(),
        ),
    });
    bridge.sup.lock().replace(Arc::downgrade(&sup));
    let sid = store.id().to_owned();
    let agent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt,
        model: model.to_owned(),
        tools: tools::surface::root_specs(),
        cwd,
        provider,
        tool_batch_on_force: config.requests.tool_batch_on_force,
        turn,
        gates: config.gates.clone(),
        gate_failed,
        om: Some(OmState::from_config(&config.om, record)),
        om_route,
        hashline: gc_policy(config),
        subagents: Some(sup.clone()),
        child: None,
    }));
    wire_events(
        core,
        &workspace.id,
        &sid,
        &agent,
        Path::new(&workspace.cwd).join(".tau").as_path(),
    );
    sup.attach_parent(&agent);
    Ok(agent)
}

/// The root's hashline GC policy (config carries both fields verbatim).
fn gc_policy(config: &Config) -> crate::hashline::pin_store::GcPolicy {
    crate::hashline::pin_store::GcPolicy {
        retention_days: config.hashline.retention_days,
        gc_on_start: config.hashline.gc_on_start,
    }
}

/// The root's five event hooks: the `om_status` kind-string shaping, the
/// queue projection, the streaming-delta tee (#303), and the two entry tees
/// (ADR-0008's file-line upserts).
pub(super) fn wire_events(
    core: &Arc<Core>,
    workspace_id: &str,
    sid: &str,
    agent: &Arc<AgentSession>,
    blob_root: &Path,
) {
    // The plan-state hook (design: the plan file is the source of
    // truth): the core's watcher-parsed state, re-injected at each
    // assembly. A `Weak` keeps the hook from pinning the core.
    wire_plan_hook(agent, Some(Arc::clone(core)), sid);
    // The core's om_status hook
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
        // The tool's call phase and the discrete cards re-emit their entry on
        // the wire; same mapping, same channel as the queue's events. The
        // streaming assistant has its own delta hook above (#303).
        let u_tx = tx.clone();
        let u_ws = ws.clone();
        let u_sid = sid.clone();
        // #265: the live upsert inlines sidecar-blob payloads, so every
        // consumer (GUI live path, ACP) sees the full payload without a
        // client-side `BlobRead` follow-up.
        let upsert = UpsertView::new(blob_root.to_path_buf());
        let upsert2 = UpsertView::new(blob_root.to_path_buf());
        agent.set_entry_upsert_hook(Some(Arc::new(move |entry: &Entry| {
            let _ = u_tx.try_send(Event::EntryUpsert {
                workspace: u_ws.clone(),
                session: u_sid.clone(),
                entry: upsert.entry(entry),
            });
        })));
        // The streaming assistant's live growth (#303): one delta per frame,
        // carrying the new characters. A pipe overflow drops a delta and the
        // `seq` gap makes it visible to the consumer; the committed entry
        // settles the card either way, so the record is never at stake.
        let d_tx = tx.clone();
        let d_ws = ws.clone();
        let d_sid = sid.clone();
        agent.set_stream_delta_hook(Some(Arc::new(move |delta: &StreamDelta| {
            let _ = d_tx.try_send(Event::StreamDelta {
                workspace: d_ws.clone(),
                session: d_sid.clone(),
                id: delta.id.clone(),
                seq: delta.seq,
                text: delta.text.clone(),
                reasoning: delta.reasoning.clone(),
            });
        })));
        agent.set_entry_event_hook(Some(Arc::new(move |entry: &Entry| {
            let _ = tx.try_send(Event::EntryUpsert {
                workspace: ws.clone(),
                session: sid.clone(),
                entry: upsert2.entry(entry),
            });
        })));
    }
    // Background-job completion (#53): the wake a sub-agent's report gets —
    // steering-lane notification + queue projection + turn start.
    wire_job_hook(core, workspace_id, sid, agent);
}

/// The background-job completion hook (#53): the wake a sub-agent's report
/// gets — steering-lane notification + queue projection + turn start. A
/// closed session is gone from the map: no wake, no turn.
pub(super) fn wire_job_hook(
    core: &Arc<Core>,
    workspace_id: &str,
    sid: &str,
    agent: &Arc<AgentSession>,
) {
    let core = core.clone();
    let tx = core.events_tx.clone();
    let ws = workspace_id.to_string();
    let sid = sid.to_string();
    agent.set_job_notify_hook(Some(Arc::new(
        move |job_id: &str, code: Option<i32>, output_path: &Path| {
            let Some(live) = core.sessions.lock().get(&sid).cloned() else {
                return;
            };
            let status = match code {
                Some(n) => n.to_string(),
                None => "killed".to_owned(),
            };
            let output = output_path.display();
            live.agent.send_notified_steer(
            format!(
                "background job {job_id} ({output}): finished with exit {status}; output: {output}"
            ),
            format!("job:{job_id}"),
        );
            let _ = tx.try_send(Event::Queue {
                workspace: ws.clone(),
                session: sid.clone(),
                items: live.agent.queued_items(),
            });
            live.stop.store(false, Ordering::SeqCst);
            if live
                .turn
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
                && let Some(core) = core.self_arc()
            {
                tokio::spawn(crate::harness::turn::run_turn(core, live));
            }
        },
    )));
}

/// The live-upsert view (ADR-0008, #265): `entry_to_view` plus sidecar-blob
/// inlining. An entry whose payload was extracted to a blob (the 100 KB
/// threshold) carries the decoded payload on the wire; a failed read (the
/// blob file is gone) falls through to the blobless view, never an error.
pub(super) struct UpsertView {
    blob_root: PathBuf,
}

impl UpsertView {
    pub(super) fn new(blob_root: PathBuf) -> Self {
        Self { blob_root }
    }

    pub(super) fn entry(&self, entry: &Entry) -> ViewEntry {
        let mut view = entry_to_view(entry);
        if let Some(blob) = &view.blob {
            let core_ref = crate::session::BlobRef {
                id: blob.id.clone(),
                size: blob.size,
                hash: blob.hash.clone(),
            };
            if let Ok(bytes) = SessionStore::read_blob(&self.blob_root, &core_ref)
                && let Ok(payload) = serde_json::from_slice(&bytes)
            {
                view.payload = payload;
            }
        }
        view
    }
}
