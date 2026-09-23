//! The shared harness test kit: the canned provider set, the scripted bodies, the child stubs, the session plumbing, and the bounded event waits.

use std::sync::atomic::AtomicUsize;

use super::*;

pub(crate) fn providers() -> BTreeMap<String, crate::config::Provider> {
    let mut m = BTreeMap::new();
    m.insert(
        "dev".into(),
        crate::config::Provider {
            base_url: "http://127.0.0.1:9/v1".into(),
            key_env: String::new(),
            models: vec!["model".into()],
        },
    );
    m
}

/// A session wired to `inner` (canned in tests, production in the
/// live test), registered with the core the way `SessionNew` does.
pub(crate) fn manual_session(
    core: &Arc<Core>,
    workspace: &Workspace,
    inner: TurnProviderRef,
    turn: TurnConfig,
) -> Arc<LiveSession> {
    let tmp = workspace.cwd.clone();
    let cwd = PathBuf::from(&tmp);
    let mut store = SessionStore::for_workspace(&cwd, &SessionStore::new_session_id());
    store.create().unwrap();
    let created = store.created();
    let provider = Arc::new(ForwardingProvider {
        inner,
        tx: core.events_tx.clone(),
        workspace: workspace.id.clone(),
        session: store.id().to_owned(),
        stop: Arc::new(AtomicBool::new(false)),
        call_seq: AtomicU64::new(0),
        calls: Arc::new(Mutex::new(Vec::new())),
        completed: Arc::new(Mutex::new(HashMap::new())),
    });
    let agent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "You are Tau, a coding agent.".into(),
        model: "model".into(),
        tools: tools::tool_specs(),
        cwd: cwd.clone(),
        provider: provider.clone(),
        tool_batch_on_force: Default::default(),
        turn,
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    let live = Arc::new(LiveSession {
        meta: Mutex::new(SessionMeta {
            id: provider.session.clone(),
            workspace: workspace.id.clone(),
            title: None,
            parent: None,
            created,
            leaf: None,
            model: Some("model".into()),
            usage: None,
            archived: false,
        }),
        agent: agent.clone(),
        stop: provider.stop.clone(),
        queue: Mutex::new(Vec::new()),
        turn: AtomicBool::new(false),
        provider,
        cwd,
    });
    core.sessions
        .lock()
        .unwrap()
        .insert(live.meta.lock().unwrap().id.clone(), live.clone());
    live
}

pub(crate) fn canned_body() -> String {
    let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    for frame in [
        r#"{"type":"response.output_text.delta","delta":"partial"}"#,
        r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking"}"#,
        r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"bash","arguments":"{\"command\":\"ls\"}"}}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#,
    ] {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    body
}

/// A child provider that replays one canned body per call (the
/// scripted sub-agent turns of the dispatch tests).
pub(crate) struct CannedChild {
    pub(crate) body: String,
    pub(crate) index: AtomicUsize,
}
impl TurnProvider for CannedChild {
    fn call<'a>(&self, _req: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let body = self.body.clone();
        self.index.fetch_add(1, Ordering::SeqCst);
        let (events, calls) = provider::decode_stream(&body).unwrap();
        Box::pin(async move {
            let mut result = provider::TurnResult::default();
            for event in &events {
                if !sink.event(event.clone()) {
                    break;
                }
                provider::fold_event(event, &mut result);
            }
            result.calls = calls.into_iter().map(|(_, c)| c).collect();
            Ok(result)
        })
    }
}
pub(crate) struct CannedChildFactory {
    pub(crate) body: String,
}
impl ChildProviderFactory for CannedChildFactory {
    fn create(&self, _child: &str) -> TurnProviderRef {
        Arc::new(CannedChild {
            body: self.body.clone(),
            index: AtomicUsize::new(0),
        })
    }
}

/// A child provider that sleeps between events: the turn stays
/// running long enough to observe (the archive's running-child
/// refusal, review N4).
pub(crate) struct SlowChild {
    pub(crate) body: String,
    pub(crate) delay_ms: u64,
}
impl TurnProvider for SlowChild {
    fn call<'a>(&self, _req: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let body = self.body.clone();
        let delay = self.delay_ms;
        Box::pin(async move {
            let (events, calls) = provider::decode_stream(&body).unwrap();
            let mut result = provider::TurnResult::default();
            for event in &events {
                if !sink.event(event.clone()) {
                    break;
                }
                provider::fold_event(event, &mut result);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            result.calls = calls.into_iter().map(|(_, c)| c).collect();
            Ok(result)
        })
    }
}
pub(crate) struct SlowChildFactory {
    pub(crate) body: String,
    pub(crate) delay_ms: u64,
}
impl ChildProviderFactory for SlowChildFactory {
    fn create(&self, _child: &str) -> TurnProviderRef {
        Arc::new(SlowChild {
            body: self.body.clone(),
            delay_ms: self.delay_ms,
        })
    }
}

/// One scripted turn with no `parent_notify`: the child ends the turn
/// (the nudge path), so a slow provider keeps it running.
pub(crate) fn plain_body() -> String {
    let data = r#"data: {"type":"response.output_text.delta","delta":"working on it"}"#;
    format!("{data}\n\ndata: [DONE]\n\n")
}

/// One scripted turn: parent_notify done with a structured output.
pub(crate) fn done_body() -> String {
    let call_id = "c1".to_string();
    let args = json!({
        "text": "the work is done",
        "done": true,
        "output": { "result": "ok" },
    });
    let item = json!({
        "id": call_id,
        "type": "function_call",
        "name": "parent_notify",
        "call_id": call_id,
        "arguments": args.to_string(),
    });
    let data = format!("data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}");
    format!("{data}\n\ndata: [DONE]\n\n")
}

/// Spawn one scripted child on the parent and wait for its done state
/// and the parent's wake turn to settle (a done child always wakes the
/// parent, ADR-0001; a running parent's archive is refused, so the
/// archive tests need the parent at rest). Bounded — a hung drive or
/// wake is a test failure, not a wait.
pub(crate) async fn spawn_done_child(core: &Arc<Core>, parent_id: &str) -> SubagentInfo {
    let info = match core
        .dispatch(Command::SubagentSpawn {
            session: parent_id.into(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent,
        other => panic!("expected a subagent: {other:?}"),
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last: String;
    loop {
        let out = core
            .dispatch(Command::SubagentState {
                handle: info.handle.clone(),
            })
            .unwrap();
        let state = match &out {
            CommandOutput::Subagent { subagent } => subagent.state.clone(),
            _ => String::new(),
        };
        if state == "done" {
            break;
        }
        last = state;
        if tokio::time::Instant::now() > deadline {
            panic!("the child never finished (last state: {last})");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // The done wake: wait for the parent's turn to start (the wake's
    // CAS) and then to settle.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let running = core
            .sessions
            .lock()
            .unwrap()
            .get(parent_id)
            .is_some_and(|l| l.turn.load(Ordering::SeqCst));
        if running {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("the done wake never started the parent's turn");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let running = core
            .sessions
            .lock()
            .unwrap()
            .get(parent_id)
            .is_some_and(|l| l.turn.load(Ordering::SeqCst));
        if !running {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("the parent's wake turn never settled");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    info
}

/// The tokio tests share this: open a workspace rooted at a temp dir.
pub(crate) async fn open_ws(core: &Arc<Core>, cwd: &std::path::Path) -> Workspace {
    match core
        .dispatch(Command::WorkspaceOpen {
            cwd: cwd.display().to_string(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace } => workspace,
        other => panic!("expected workspace: {other:?}"),
    }
}

pub(crate) async fn wait_for_user_entry(workspace: &Workspace, session: &str) -> Vec<Entry> {
    let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), session);
    store.open().unwrap();
    for _ in 0..100 {
        let entries = store.entries_range(0, 100).unwrap();
        if entries.iter().any(|e| e.kind == crate::agent::KIND_USER) {
            return entries;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    store.entries_range(0, 100).unwrap()
}

/// The pump into a collected-events `Mutex<Vec<Event>>` (the app's
/// transport stand-in, ADR-0006).
pub(crate) fn collect_events(core: &Arc<Core>) -> Arc<Mutex<Vec<Event>>> {
    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    tokio::spawn(crate::harness::pump::pump(Arc::clone(core), move |batch| {
        sink.lock().unwrap().extend(batch.iter().cloned());
    }));
    collected
}

/// Bounded wait (generous, 5 s) for the latest `SkillListChanged` for
/// state — the tests assert that, never the event sequence.
pub(crate) async fn wait_for_skill_list_changed(
    collected: &Arc<Mutex<Vec<Event>>>,
    ws: &str,
    ok: impl Fn(&[SkillInfo]) -> bool,
) -> Vec<SkillInfo> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(skills) = collected
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::SkillListChanged { workspace, skills } if workspace == ws => {
                    Some(skills.clone())
                }
                _ => None,
            })
            && ok(&skills)
        {
            return skills;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no qualifying skill_list_changed for {ws} within 5 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Bounded wait (generous, 5 s) for the latest `FileTreeChanged` for
/// `ws` whose stale dirs satisfy `ok`; returns that dir list.
pub(crate) async fn wait_for_file_tree_changed(
    collected: &Arc<Mutex<Vec<Event>>>,
    ws: &str,
    ok: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(changed) = collected
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::FileTreeChanged { workspace, changed } if workspace == ws => {
                    Some(changed.clone())
                }
                _ => None,
            })
            && ok(&changed)
        {
            return changed;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no qualifying file_tree_changed for {ws} within 5 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) fn write_skill_fixture(project: &Path, rel: &str, raw: &str) {
    let path = project.join(rel).join("SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, raw).unwrap();
}
