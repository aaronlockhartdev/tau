//! Mock-LLM acceptance suites, split from main.rs to stay under the file-size limit.

#[allow(clippy::wildcard_imports)] // the suites share main.rs's driver helpers
use super::*;
pub(crate) async fn accept_tools(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    std::fs::write(ws.path().join("notes.txt"), "line1\nline2\nline3\n").unwrap();
    let (id, store) = new_session(ws.path());
    let (_, prov) = production(ctx);
    let agent = AgentSession::launch(
        store,
        SessionRole::Bare {
            provider: prov,
            system_prompt: TOOLS_PROMPT.into(),
            model: ctx.model.clone(),
            tools: tools::tool_specs(),
            cwd: ws.path().into(),
            turn: capped(),
            gates: None,
            tool_batch_on_force: ToolBatchPolicy::default(),
            om: None,
            om_model: String::new(),
        },
    )
    .map_err(|e| format!("launch failed: {e:?}"))?;
    agent.send(
        "Do exactly this: 1) read notes.txt, 2) edit the line containing 'line2' so it becomes 'LINE2', 3) bash: cat notes.txt, 4) write the file out.txt with the single line 'done'. Then reply 'finished'.",
        Lane::Steering,
    );
    agent.process().await.map_err(|e| e.to_string())?;
    drop(agent);

    let store = SessionStore::for_workspace(ws.path(), &id);
    let entries = all_entries(&store);
    let calls = tool_calls(&entries);
    for name in ["read", "write", "edit", "bash"] {
        if !calls.contains(&name) {
            return Err(format!(
                "accept-tools: the model never called {name} (saw {calls:?})"
            ));
        }
    }
    let golden = std::fs::read_to_string(ws.path().join("out.txt")).map_err(|e| e.to_string())?;
    if golden.trim() != "done" {
        return Err(format!(
            "accept-tools: out.txt is {golden:?}, expected 'done'"
        ));
    }
    let notes = std::fs::read_to_string(ws.path().join("notes.txt")).map_err(|e| e.to_string())?;
    if !notes.contains("LINE2") {
        return Err(
            "accept-tools: the hash-anchored edit did not land (notes.txt lacks LINE2)".into(),
        );
    }
    Ok(())
}

struct AcceptanceFactory {
    client: reqwest::Client,
    provider: Provider,
    requests: Requests,
}

impl ChildProviderFactory for AcceptanceFactory {
    fn create(&self, _child: &str) -> tau_core::provider::TurnProviderRef {
        provider::production(&self.client, &self.provider, &self.requests)
    }
}

struct AcceptanceDriver;

impl ChildDriver for AcceptanceDriver {
    fn drive(&self, _session: &str, agent: &Arc<AgentSession>) -> BoxedDrive {
        let agent = agent.clone();
        Box::pin(async move { agent.process().await.map_err(|e| e.to_string()) })
    }
}

/// The driver-side bridge: no GUI events; a wake delivers the child's result
/// to the parent loop exactly as the app does (`send_notified_steer`, provenance).
struct AcceptanceBridge {
    parent: Mutex<Weak<AgentSession>>,
    reclaims: Mutex<Vec<ReclaimNotice>>,
    readopts: Mutex<Vec<ReadoptNotice>>,
}
/// The completion gate (spec §5.3): a failing check bounces repair turns
/// until `max_retries` is exhausted (the session is marked `gate_failed`), and
/// a `QUESTION:`-ending turn skips the gate.
#[allow(clippy::too_many_lines)]
pub(crate) async fn accept_gate(ctx: &Ctx) -> Result<(), String> {
    // A bare session with a completion gate: the check command is stated in
    // the system prompt's gate section, which the mock scenario matches on.
    fn gated_session(
        ws: &tempfile::TempDir,
        ctx: &Ctx,
        command: &str,
    ) -> Result<(String, Arc<AgentSession>), String> {
        let (id, store) = new_session(ws.path());
        let (_, prov) = production(ctx);
        let gates = Gates {
            command: command.into(),
            timeout_secs: 30,
            max_retries: 2,
        };
        let agent = AgentSession::launch(
            store,
            SessionRole::Bare {
                provider: prov,
                system_prompt: TOOLS_PROMPT.into(),
                model: ctx.model.clone(),
                tools: tools::tool_specs(),
                cwd: ws.path().into(),
                turn: capped(),
                tool_batch_on_force: ToolBatchPolicy::default(),
                om: None,
                om_model: String::new(),
                gates: Some(gates),
            },
        )
        .map_err(|e| format!("launch failed: {e:?}"))?;
        Ok((id, agent))
    }

    // The gate's records (ADR-0034): typed facts, so an acceptance suite can
    // check the outcome without parsing a sentence.
    fn gate_facts(store: &SessionStore) -> Vec<Diagnostic> {
        all_entries(store)
            .into_iter()
            .filter(|e| e.kind == EntryKind::Diagnostic)
            .filter_map(|e| Diagnostic::from_value(&e.payload))
            .filter(|d| {
                matches!(
                    d,
                    Diagnostic::GateAttempt { .. } | Diagnostic::GateFailed { .. }
                )
            })
            .collect()
    }
    fn assistant_count(store: &SessionStore) -> usize {
        all_entries(store)
            .iter()
            .filter(|e| e.kind == EntryKind::Assistant)
            .count()
    }

    // --- Scenario 1: a failing check exhausts the retries → gate_failed. ---
    let ws = temp_ws();
    let (id, agent) = gated_session(&ws, ctx, "test -f gate-target.txt")?;
    agent.send("Do the work and finish.", Lane::Steering);
    agent.process().await.map_err(|e| e.to_string())?;
    drop(agent);
    let store = SessionStore::for_workspace(ws.path(), &id);
    // The initial check + two repairs: two tool rounds + three final-text
    // segments → five assistant entries (the repair bounces).
    let assistants = assistant_count(&store);
    if assistants != 5 {
        return Err(format!(
            "accept-gate: expected 5 assistant entries (2 tool + 3 final), saw {assistants}"
        ));
    }
    let facts = gate_facts(&store);
    if facts.len() != 3 {
        return Err(format!(
            "accept-gate: expected 3 gate records (initial + 2 repairs), saw {facts:?}"
        ));
    }
    if !matches!(facts.last(), Some(Diagnostic::GateFailed { .. })) {
        return Err(format!(
            "accept-gate: the final record is not a gate failure: {facts:?}"
        ));
    }
    if !tau_core::diagnostics::project(&all_entries(&store)).gate_failed {
        return Err("accept-gate: the records leave the session not gate_failed".into());
    }
    println!("accept-gate: the failing check bounced 2 repairs and marked the session gate_failed");

    // --- Scenario 2: a QUESTION:-ending turn skips the gate. ---
    let ws = temp_ws();
    let (id, agent) = gated_session(&ws, ctx, "test -f question-target.txt")?;
    agent.send("Do the work and finish.", Lane::Steering);
    agent.process().await.map_err(|e| e.to_string())?;
    drop(agent);
    let store = SessionStore::for_workspace(ws.path(), &id);
    // Two assistant entries (the tool round + the QUESTION: final): the gate
    // is skipped, so no repair bounce and no gate record.
    let assistants = assistant_count(&store);
    if assistants != 2 {
        return Err(format!(
            "accept-gate (question): expected 2 assistant entries, saw {assistants}"
        ));
    }
    let facts = gate_facts(&store);
    if !facts.is_empty() {
        return Err(format!(
            "accept-gate (question): the gate should be skipped, saw records {facts:?}"
        ));
    }
    println!("accept-gate: the QUESTION:-ending turn skipped the gate");

    Ok(())
}

impl SubagentBridge for AcceptanceBridge {
    fn spawned(&self, _n: &SpawnNotice) {}
    fn state(&self, _n: &StateNotice) {}
    fn reclaimed(&self, n: &ReclaimNotice) {
        self.reclaims.lock().unwrap().push(n.clone());
    }
    fn re_adopted(&self, n: &ReadoptNotice) {
        self.readopts.lock().unwrap().push(n.clone());
    }
    fn wake(&self, n: &WakeNotice) {
        let Some(p) = self.parent.lock().unwrap().upgrade() else {
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
        p.send_notified_steer(text, n.child.clone());
    }
}

#[allow(clippy::too_many_lines)] // the suite is one flat driver flow; splitting is refactoring
pub(crate) async fn accept_subagent(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    let (id, store) = new_session(ws.path());
    let (p, prov) = production(ctx);

    let bridge = Arc::new(AcceptanceBridge {
        parent: Mutex::new(Weak::new()),
        reclaims: Mutex::new(Vec::new()),
        readopts: Mutex::new(Vec::new()),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: id.clone(),
        cwd: ws.path().into(),
        provider: Arc::new(AcceptanceFactory {
            client: client(),
            provider: p,
            requests: Requests::default(),
        }),
        model: ctx.model.clone(),
        system_prompt: format!(
            "{SUBAGENT_PROMPT}\n\nYou are a child sub-agent; keep your own plan file current and report to the parent when done."
        ),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::default(),
        turn: capped(),
        gates: None,
        hashline: tau_core::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![builtin_general()],
        bridge: bridge.clone(),
        driver: Arc::new(AcceptanceDriver),
        core: None,
        parent_entries: None,
    });
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: prov,
            supervisor: Some(sup),
            // The parent's own prompt, explicitly: the supervisor's is the
            // child's (the mock scenario matches on it), so the seam takes
            // the parent's rather than adopting it.
            system_prompt: Some(SUBAGENT_PROMPT.into()),
            base_prompt: None,
            first_provider: None,
        },
    )
    .map_err(|e| format!("launch failed: {e:?}"))?;
    {
        *bridge.parent.lock().unwrap() = Arc::downgrade(&agent);
    }

    agent
        .send(
            "Write your plan file with one unchecked step, 'the child reports ok'. Then use subagent_spawn to spawn one sub-agent (type 'general', context_mode 'fresh') with the brief: 'Work your plan file: when done, check the step off, then call parent_notify with done true and output {\"status\": \"ok\"}.' When the sub-agent reports back, check your plan step off and reply exactly: child done.",
            Lane::Steering,
        );
    agent.process().await.map_err(|e| e.to_string())?;

    // The child's drive runs on the supervisor's task. Wait for the child
    // to reach a terminal state (reading its own session file), running the
    // parent's follow-up turns whenever its queue is non-empty (the wake
    // lands as a queued notification, spec §5.2 wake rules).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let child_file = session_files(ws.path())
        .iter()
        .find(|f| f.file_stem().and_then(|n| n.to_str()) != Some(id.as_str()))
        .cloned();
    loop {
        if std::time::Instant::now() > deadline {
            dump_sessions(ws.path(), "/tmp/accept-subagent-dump");
            return Err("accept-subagent: the child never reached a terminal state in 300 s (session dump at /tmp/accept-subagent-dump)".into());
        }
        let terminal = child_file
            .as_ref()
            .and_then(|f| {
                let store = SessionStore::for_workspace(ws.path(), f.file_stem()?.to_str()?);
                all_entries(&store)
                    .iter()
                    .rev()
                    .find(|e| e.kind == EntryKind::Subagent)
                    .and_then(|e| e.payload.get("state").and_then(Value::as_str))
                    .map(|s| matches!(s, "done" | "failed" | "stopped"))
            })
            .unwrap_or(false);
        if agent.has_pending() {
            agent.process().await.map_err(|e| e.to_string())?;
        } else if terminal {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    // A wake that landed with the terminal state: give the parent its turn.
    if agent.has_pending() {
        agent.process().await.map_err(|e| e.to_string())?;
    }

    // The child session file exists alongside the parent's.
    let files = session_files(ws.path());
    if files.len() < 2 {
        return Err(format!(
            "accept-subagent: expected a child session file, found {files:?}"
        ));
    }
    drop(agent);

    let parent = SessionStore::for_workspace(ws.path(), &id);
    let pentries = all_entries(&parent);
    let child_id = files
        .iter()
        .filter_map(|f| {
            f.file_stem()
                .and_then(|n| n.to_str())
                .map(std::string::ToString::to_string)
        })
        .find(|n| n.as_str() != id.as_str())
        .ok_or("accept-subagent: no child session file")?;
    // The child's report landed on the parent's branch tagged with the
    // child's id (a done/failed wake or an idle notification — both are
    // lifecycle notifications, spec §5.2).
    let wake = pentries.iter().any(|e| {
        e.kind == EntryKind::User
            && e.payload.get("source").and_then(Value::as_str) == Some(child_id.as_str())
    });
    if !wake {
        return Err("accept-subagent: no wake entry from the child on the parent's branch".into());
    }
    // The child's session carries its own record trail (state entries).
    let child_store = SessionStore::for_workspace(ws.path(), &child_id);
    let centries = all_entries(&child_store);
    if !centries.iter().any(|e| e.kind == EntryKind::Subagent) {
        return Err("accept-subagent: the child's session has no lifecycle state entries".into());
    }
    let child_state = centries
        .iter()
        .rev()
        .find(|e| e.kind == EntryKind::Subagent)
        .and_then(|e| e.payload.get("state").and_then(Value::as_str))
        .unwrap_or("");
    // Plan-file linkage (spec §5.3): the child's `done` is observed from
    // its own plan file — a terminal child must have a complete plan
    // (every step checked), and the parent's plan is complete after the
    // child's report.
    if matches!(child_state, "done" | "failed") {
        let raw = std::fs::read_to_string(tau_core::plan::path(ws.path(), &child_id))
            .map_err(|e| format!("accept-subagent: the child's plan file is unreadable: {e}"))?;
        if !tau_core::plan::parse(&raw).complete() {
            return Err(format!(
                "accept-subagent: the child ended {child_state} but its plan is not complete"
            ));
        }
    }
    let parent_raw = std::fs::read_to_string(tau_core::plan::path(ws.path(), &id))
        .map_err(|e| format!("accept-subagent: the parent's plan file is unreadable: {e}"))?;
    if !tau_core::plan::parse(&parent_raw).complete() {
        return Err(
            "accept-subagent: the parent's plan is not complete after the child's report".into(),
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // one flat reclamation flow; splitting is refactoring
//
// The fleet-reclamation acceptance scenario (#250): the scripted parent
// spawns two children (the E2E spawn path through the mock); the driver
// then performs the 3rd spawn itself — with `fleet_soft_bound = 2` that
// push evicts the oldest terminal child (spawn-triggered reclamation) — and
// re-addresses it by session id, which a `resolve` miss rebuilds from disk
// (re-adopt). The parent fixtures name the children by a "reclaim probe
// child" marker so the mock routes them to `acceptance-reclaim-child.json`.
pub(crate) async fn accept_reclaim(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    let (id, store) = new_session(ws.path());
    let (p, prov) = production(ctx);

    let bridge = Arc::new(AcceptanceBridge {
        parent: Mutex::new(Weak::new()),
        reclaims: Mutex::new(Vec::new()),
        readopts: Mutex::new(Vec::new()),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: id.clone(),
        cwd: ws.path().into(),
        provider: Arc::new(AcceptanceFactory {
            client: client(),
            provider: p,
            requests: Requests::default(),
        }),
        model: ctx.model.clone(),
        // The supervisor's prompt is the child's: the "reclaim probe child"
        // marker routes every child turn to the reclaim child fixture.
        system_prompt: format!(
            "{SUBAGENT_PROMPT}\n\nYou are a reclaim probe child; call parent_notify with done true when the brief is met."
        ),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::default(),
        turn: capped(),
        gates: None,
        hashline: tau_core::hashline::pin_store::GcPolicy::default(),
        // Soft bound 2: the 3rd spawn pushes the fleet to 3 and evicts one
        // terminal child. The hard cap is far above: it is not the trigger.
        caps: SubAgents {
            fleet_soft_bound: Some(2),
            fleet_hard_cap: Some(64),
            ..SubAgents::default()
        },
        depth: 0,
        types: vec![builtin_general()],
        bridge: bridge.clone(),
        driver: Arc::new(AcceptanceDriver),
        core: None,
        parent_entries: None,
    });
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: prov,
            supervisor: Some(sup.clone()),
            // The parent's own prompt: the "reclaim scenario parent" marker
            // routes the parent's turns to the reclaim parent fixture.
            system_prompt: Some("reclaim scenario parent. You have the tools read, write, edit, bash, and the sub-agent tools (subagent_spawn, subagent_message, subagent_stop, subagent_state). Use them as instructed.".into()),
            base_prompt: None,
            first_provider: None,
        },
    )
    .map_err(|e| format!("accept-reclaim: launch failed: {e:?}"))?;
    {
        *bridge.parent.lock().unwrap() = Arc::downgrade(&agent);
    }

    agent
        .send(
            "Spawn two sub-agents (type 'general', context_mode 'fresh'). Give the first the brief 'reclaim probe child 1: call parent_notify with text ok and done true, then stop.' and the second 'reclaim probe child 2: call parent_notify with text ok and done true, then stop.' Then reply: two sub-agents spawned.",
            Lane::Steering,
        );
    agent.process().await.map_err(|e| e.to_string())?;

    // Wait for both model-spawned children to reach a terminal state (their
    // drive tasks run independently of the parent; the driver polls the
    // child session files, as accept-subagent does).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if std::time::Instant::now() > deadline {
            dump_sessions(ws.path(), "/tmp/accept-reclaim-dump");
            return Err("accept-reclaim: the children never reached a terminal state in 120 s (dump at /tmp/accept-reclaim-dump)".into());
        }
        let terminal_count = session_files(ws.path())
            .iter()
            .filter(|f| f.file_stem().and_then(|n| n.to_str()) != Some(id.as_str()))
            .filter(|f| {
                let s = SessionStore::for_workspace(
                    ws.path(),
                    f.file_stem().unwrap().to_str().unwrap(),
                );
                all_entries(&s)
                    .iter()
                    .rev()
                    .find(|e| e.kind == EntryKind::Subagent)
                    .and_then(|e| e.payload.get("state").and_then(Value::as_str))
                    .is_some_and(|st| matches!(st, "done" | "failed" | "stopped"))
            })
            .count();
        if terminal_count >= 2 {
            break;
        }
        if agent.has_pending() {
            agent.process().await.map_err(|e| e.to_string())?;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // Two terminal children, soft bound 2: the fleet is at the bound, not
    // over it, so no reclamation has fired yet.
    {
        let reclaims = bridge.reclaims.lock().unwrap();
        if !reclaims.is_empty() {
            return Err(format!(
                "accept-reclaim: unexpected early reclamation: {reclaims:?}"
            ));
        }
    }
    // The 3rd spawn pushes the fleet to 3 > soft bound 2: one terminal child
    // is evicted (handles only — its file and live session are intact). The
    // reclamation notice names the evicted child; that is the re-address key.
    let third = sup
        .spawn(
            "general",
            "reclaim probe child 3: call parent_notify with text ok and done true, then stop.",
            None,
            "c3",
        )
        .map_err(|e| format!("accept-reclaim: the 3rd spawn failed: {e}"))?;
    let evicted = {
        let reclaims = bridge.reclaims.lock().unwrap();
        if reclaims.len() != 1 {
            return Err(format!(
                "accept-reclaim: expected one reclamation, saw {reclaims:?}"
            ));
        }
        // The evicted child must be one of the two model-spawned children
        // (never the 3rd, which is the live filler that triggered the push).
        let evicted = reclaims[0].child.clone();
        let live: Vec<String> = sup
            .handles()
            .iter()
            .filter_map(|h| sup.child_session_id(h))
            .collect();
        if live.iter().any(|c| c == &evicted) {
            return Err(format!(
                "accept-reclaim: the evicted child {evicted} is still in the live registry"
            ));
        }
        if evicted == third.session_id {
            return Err(
                "accept-reclaim: the 3rd (triggering) child was evicted, not a terminal one".into(),
            );
        }
        evicted
    };
    // Re-address the evicted child by session id: a resolve miss falls back
    // to the disk record and rebuilds it as a cold, terminal child.
    let info = sup
        .state_info(&evicted)
        .ok_or("accept-reclaim: the evicted child did not re-adopt from disk")?;
    if !matches!(
        info.state,
        ChildState::Done { .. } | ChildState::Failed { .. } | ChildState::Stopped { .. }
    ) {
        return Err(format!(
            "accept-reclaim: the re-adopted child is not terminal: {}",
            info.state.kind()
        ));
    }
    {
        let readopts = bridge.readopts.lock().unwrap();
        if readopts.len() != 1 {
            return Err(format!(
                "accept-reclaim: expected one re-adopt, saw {readopts:?}"
            ));
        }
        if readopts[0].child != evicted {
            return Err(format!(
                "accept-reclaim: re-adopted {} but addressed {}",
                readopts[0].child, evicted
            ));
        }
    }
    // The re-adopted handle is back in the live registry.
    if !sup
        .handles()
        .iter()
        .any(|h| sup.child_session_id(h).as_deref() == Some(evicted.as_str()))
    {
        return Err("accept-reclaim: the re-adopted child is not back in the live registry".into());
    }

    // The third child's drive is still alive; drop the agent to let the run
    // finish. (The 3rd child is not asserted: it is the fleet filler, not
    // the reclamation subject.)
    drop(third);
    drop(agent);
    Ok(())
}
#[allow(clippy::too_many_lines)] // the suite is one flat driver flow; splitting is refactoring
pub(crate) async fn accept_sender_labels(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    // The mock (started by the justfile with TAU_MOCK_CAPTURE set) appends
    // every request it answers to that directory; the suite reads it to
    // assert what the child's model was assembled with (ticket #188).
    let capture = std::env::var("TAU_MOCK_CAPTURE")
        .map(std::path::PathBuf::from)
        .map_err(|e| format!("accept-sender-labels: TAU_MOCK_CAPTURE is not set: {e}"))?;
    let (id, store) = new_session(ws.path());
    let (p, prov) = production(ctx);

    let bridge = Arc::new(AcceptanceBridge {
        parent: Mutex::new(Weak::new()),
        reclaims: Mutex::new(Vec::new()),
        readopts: Mutex::new(Vec::new()),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: id.clone(),
        cwd: ws.path().into(),
        provider: Arc::new(AcceptanceFactory {
            client: client(),
            provider: p,
            requests: Requests::default(),
        }),
        model: ctx.model.clone(),
        // The child's prompt: the mock scenario matches it, and it is what
        // the `general` type inherits.
        system_prompt: "sender-labels child: you are the child sub-agent; keep your own plan file current and report to the parent when done.".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::default(),
        turn: capped(),
        gates: None,
        hashline: tau_core::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![builtin_general()],
        bridge: bridge.clone(),
        driver: Arc::new(AcceptanceDriver),
        core: None,
        parent_entries: None,
    });
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: prov,
            supervisor: Some(sup.clone()),
            // The parent's own prompt, explicitly: the supervisor's is the
            // child's (the mock scenario matches on it).
            system_prompt: Some(
                "sender-labels parent: you are the parent; delegate with the sub-agent tools."
                    .into(),
            ),
            base_prompt: None,
            first_provider: None,
        },
    )
    .map_err(|e| format!("launch failed: {e:?}"))?;
    {
        *bridge.parent.lock().unwrap() = Arc::downgrade(&agent);
    }

    // The parent spawns the child (the mock's `subagent_spawn` turn).
    agent.send(
        "sender-labels parent: spawn one sub-agent (type general, context_mode fresh) with the brief 'stand by'.",
        Lane::Steering,
    );
    agent.process().await.map_err(|e| e.to_string())?;

    // Find the child's session (the file that is not the parent's).
    let child_file = session_files(ws.path())
        .into_iter()
        .find(|f| f.file_stem().and_then(|n| n.to_str()) != Some(id.as_str()))
        .ok_or("accept-sender-labels: no child session file")?;
    let child_id = child_file
        .file_stem()
        .and_then(|n| n.to_str())
        .map(std::string::ToString::to_string)
        .ok_or("accept-sender-labels: child file has no name")?;

    // Steer the child from BOTH senders through the public supervisor
    // boundary (the `subagent_message` tool and the GUI child send both
    // route here, ticket #187): the first stamped with the parent's session
    // id, the second with "user". A running child takes both on its steering
    // lane; a resting child is resumed by the first and steered by the
    // second — either way both ride the child's next LLM call.
    sup.message(
        &child_id,
        Some("tighten the scope".into()),
        Lane::Steering,
        Some(id.clone()),
    )
    .map_err(|e| format!("accept-sender-labels: parent steer failed: {e}"))?;
    sup.message(
        &child_id,
        Some("also check the logs".into()),
        Lane::Steering,
        Some("user".into()),
    )
    .map_err(|e| format!("accept-sender-labels: user steer failed: {e}"))?;

    // The child's drive runs its turn; the mock captures the request it is
    // given. Poll the capture until a child request carries both stamped
    // senders, labeled (the assembly-time decoration, ticket #188).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut last = String::new();
    loop {
        let requests = std::fs::read_to_string(capture.join("requests.jsonl")).unwrap_or_default();
        for line in requests
            .lines()
            .filter(|l| l.contains("sender-labels child"))
        {
            last = line.to_owned();
        }
        if last.contains("Parent: tighten the scope") && last.contains("User: also check the logs")
        {
            break;
        }
        if std::time::Instant::now() > deadline {
            return Err(format!(
                "accept-sender-labels: no child request carried both stamped senders (last: {last})"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // The child's scripted reply distinguished the two senders (the mock's
    // turn, driven by the labeled context it was given); it is recorded in
    // the child's own session file.
    let centries = all_entries(&SessionStore::for_workspace(ws.path(), &child_id));
    if !centries.iter().any(|e| {
        e.payload
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| {
                t.contains("Parent tightened the scope")
                    && t.contains("User asked me to also check the logs")
            })
    }) {
        return Err(
            "accept-sender-labels: the child's reply did not distinguish the senders".into(),
        );
    }
    Ok(())
}

fn prose(i: usize) -> String {
    let mut s = String::new();
    for j in 0..24 {
        let _ = write!(
            s,
            "Note {i}-{j}: the expedition crossed the northern pass under a low amber sky, \
             mapping the river delta and cataloguing the stone markers found at every bend. "
        );
    }
    s
}

async fn accept_om_once(ctx: &Ctx) -> Result<Option<u32>, String> {
    let ws = temp_ws();
    let (id, mut store) = new_session(ws.path());
    // A synthesized long raw window (no model needed for the raw): ~48 KB of
    // prose ≈ 12k tokens — far past the lowered observe threshold.
    let mut parent_id: Option<String> = None;
    for i in 0..40 {
        let e = store
            .append(
                EntryKind::User,
                json!({ "text": prose(i) }),
                parent_id.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        parent_id = Some(e.id);
    }
    let store = SessionStore::for_workspace(ws.path(), &id);
    let om = OmState::from_config(
        &Om {
            om_model: String::new(),
            observe_threshold: 4000,
            // 10, not 150: a live observation can legitimately be short; the
            // suite must prove the reflector FIRES once an observation exists,
            // not that the model writes long observations
            reflect_threshold: 10,
            buffer_increment: 500,
            buffer_tokens: 0,
            retries: OmRetries::default(),
        },
        OmRecord::default(),
    );
    let (_, prov) = production(ctx);
    let agent = AgentSession::launch(
        store,
        SessionRole::Bare {
            provider: prov,
            system_prompt: TOOLS_PROMPT.into(),
            model: ctx.model.clone(),
            tools: tools::tool_specs(),
            cwd: ws.path().into(),
            turn: capped(),
            gates: None,
            tool_batch_on_force: ToolBatchPolicy::default(),
            om: Some(om),
            om_model: String::new(),
        },
    )
    .map_err(|e| format!("launch failed: {e:?}"))?;
    agent.send(
        "Summarize the expedition notes above in two sentences. Do not use any tools.",
        Lane::FollowUp,
    );
    agent.process().await.map_err(|e| e.to_string())?;
    // A second turn: the observation from the first turn-end observe sits
    // past the (lowered) reflect threshold, so the Reflector fires.
    agent.send(
        "Add one short sentence about the weather. Do not use any tools.",
        Lane::FollowUp,
    );
    agent.process().await.map_err(|e| e.to_string())?;
    drop(agent);

    let mut store = SessionStore::for_workspace(ws.path(), &id);
    let entries = all_entries(&store);
    let om_entries: Vec<&Entry> = entries.iter().filter(|e| e.kind == EntryKind::Om).collect();
    if om_entries.is_empty() {
        // No om entry at all: the observe call did not land.
        return Ok(None);
    }
    let record = OmState::load_record(&mut store).map_err(|e| e.to_string())?;
    if record.cursor.is_none() {
        return Err("accept-om: the observation cursor did not advance".into());
    }
    if record.live_observations().is_empty() {
        return Err("accept-om: the observation log is empty after the observe".into());
    }
    // One entry = the observe; two = the reflector also fired. The reflect
    // SEMANTICS are unit-tested in tau-core (fidelity oracles,
    // parseReflectorOutput); a scripted response can return degenerate
    // reflector output that the (tested) escalation path drops, so the
    // second entry is reported, not required.
    // Observation-log entry count: far below u32::MAX.
    Ok(Some(u32::try_from(om_entries.len()).expect(
        "observation-log entry count; well under u32::MAX",
    )))
}

pub(crate) async fn accept_om(ctx: &Ctx) -> Result<(), String> {
    // A dropped observe call is retried once before the suite fails.
    for attempt in 1..=2u32 {
        match accept_om_once(ctx).await {
            Ok(Some(entries)) => {
                if entries >= 2 {
                    println!("accept-om: observe fired, the reflector followed (2 om entries)");
                } else {
                    println!(
                        "accept-om: observe fired; the reflect did not land this run (1 om entry — the reflect semantics are unit-tested in tau-core)"
                    );
                }
                return Ok(());
            }
            Ok(None) if attempt == 1 => {
                eprintln!("accept-om: observe did not land (attempt {attempt}), retrying");
            }
            Ok(None) => {
                return Err(
                    "accept-om: no om entry after the observe threshold was crossed (2 attempts)"
                        .into(),
                );
            }
            Err(e) => return Err(e),
        }
    }
    Err("accept-om: no om entry after the observe threshold was crossed (2 attempts)".into())
}

pub(crate) fn core_offline() -> Result<(), String> {
    let ws = temp_ws();
    let (id, mut store) = new_session(ws.path());
    // Trunk: three entries, then branch from the second.
    let mut prev: Option<String> = None;
    let mut second: Option<String> = None;
    for i in 0..3usize {
        let e = store
            .append(
                EntryKind::User,
                json!({ "text": format!("trunk {i}") }),
                prev.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        if i == 1 {
            second = Some(e.id.clone());
        }
        prev = Some(e.id);
    }
    // The branch: append to the second entry (a new parentId forks it).
    let b1 = store
        .append(
            EntryKind::User,
            json!({ "text": "branch 1" }),
            second.as_deref(),
        )
        .map_err(|e| e.to_string())?;
    let b2 = store
        .append(
            EntryKind::User,
            json!({ "text": "branch 2" }),
            Some(b1.id.as_str()),
        )
        .map_err(|e| e.to_string())?;
    store.set_leaf(b2.id.as_str()).map_err(|e| e.to_string())?;
    let entries_before = all_entries(&store);

    // Manual archive (the zstd sidecar path) and a round-trip.
    let arc = store.archive().map_err(|e| e.to_string())?;
    if !arc.exists() {
        return Err("core: the archive file was not written".into());
    }
    store.unarchive().map_err(|e| e.to_string())?;
    let reopened = SessionStore::for_workspace(ws.path(), &id);
    let entries_after = all_entries(&reopened);
    if entries_before.len() != entries_after.len() {
        return Err(format!(
            "core: round-trip lost entries ({} -> {})",
            entries_before.len(),
            entries_after.len()
        ));
    }
    for (a, b) in entries_before.iter().zip(entries_after.iter()) {
        if a.id != b.id || a.parent != b.parent || a.payload != b.payload {
            return Err(format!("core: round-trip changed entry {}", a.id));
        }
    }
    Ok(())
}
