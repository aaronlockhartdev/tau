use super::*;
use crate::provider::{self, ResponseRequest, TurnSink};
use crate::session::SessionStore;
use std::time::Duration;

#[derive(Default)]
pub(crate) struct TestBridge {
    pub(crate) spawns: Mutex<Vec<SpawnNotice>>,
    pub(crate) states: Mutex<Vec<StateNotice>>,
    pub(crate) wakes: Mutex<Vec<WakeNotice>>,
}

impl SubagentBridge for TestBridge {
    fn spawned(&self, n: &SpawnNotice) {
        self.spawns.lock().unwrap().push(n.clone());
    }
    fn state(&self, n: &StateNotice) {
        self.states.lock().unwrap().push(n.clone());
    }
    fn wake(&self, n: &WakeNotice) {
        self.wakes.lock().unwrap().push(n.clone());
    }
}

pub(crate) struct TestDriver;

impl ChildDriver for TestDriver {
    fn drive(&self, _session: &str, agent: &Arc<AgentSession>) -> BoxedDrive {
        let agent = Arc::clone(agent);
        Box::pin(async move { agent.process().await.map_err(|e| e.to_string()) })
    }
}

/// Per-child canned scripts: the k-th child created gets scripts[k]
/// (falls back to the last script); delays likewise (default: none).
pub(crate) struct CannedFactory {
    pub(crate) scripts: Vec<Vec<String>>,
    pub(crate) delays: Vec<Duration>,
    pub(crate) created: AtomicUsize,
    /// Provider calls across every child (the quiescence test's oracle).
    pub(crate) calls: Arc<AtomicUsize>,
}

impl ChildProviderFactory for CannedFactory {
    fn create(&self, _id: &str) -> TurnProviderRef {
        let slot = self.created.fetch_add(1, Ordering::SeqCst);
        let scripts = self
            .scripts
            .get(slot)
            .or(self.scripts.last())
            .cloned()
            .unwrap_or_default();
        let pre_delay = self.delays.get(slot).copied().unwrap_or(Duration::ZERO);
        Arc::new(CannedChildProvider {
            scripts,
            index: AtomicUsize::new(0),
            pre_delay,
            calls: self.calls.clone(),
        })
    }
}

pub(crate) struct CannedChildProvider {
    scripts: Vec<String>,
    index: AtomicUsize,
    pre_delay: Duration,
    calls: Arc<AtomicUsize>,
}
impl CannedChildProvider {
    fn next(&self) -> String {
        // No modulo: an exhausted script ends in bare turns, so a
        // child's scripted turn always terminates.
        let i = self.index.fetch_add(1, Ordering::SeqCst);
        self.scripts.get(i).cloned().unwrap_or_else(|| sse("", &[]))
    }
}
impl provider::TurnProvider for CannedChildProvider {
    fn call<'a>(
        &self,
        _req: &ResponseRequest,
        sink: &'a mut dyn TurnSink,
    ) -> provider::ProviderTurn<'a> {
        let body = self.next();
        self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = self.pre_delay;
        let (events, calls) = provider::decode_stream(&body).unwrap();
        Box::pin(async move {
            let mut result = provider::TurnResult::default();
            let mut accepted = 0usize;
            for (i, event) in events.iter().cloned().enumerate() {
                // A mid-stream pause: the first event lands, then the
                // stream stalls long enough for a stop to cut it.
                if i == 1 && delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
                if !sink.event(event.clone()) {
                    break;
                }
                provider::fold_event(&event, &mut result);
                accepted += 1;
            }
            if accepted == events.len() {
                result.completed = events
                    .iter()
                    .any(|e| matches!(e, provider::TurnEvent::Completed(_)));
            }
            result.calls = calls
                .iter()
                .filter(|(i, _)| *i <= accepted)
                .map(|(_, c)| c.clone())
                .collect();
            Ok(result)
        })
    }
}

/// (name, call_id, arguments-json) → a canned SSE body.
pub(crate) fn sse(text: &str, calls: &[(String, String, String)]) -> String {
    let mut body = String::new();
    for (name, call_id, args) in calls {
        let item = json!({
            "id": call_id, "type": "function_call", "name": name,
            "call_id": call_id, "arguments": args,
        });
        body.push_str(&format!(
            "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\n"
        ));
    }
    if !text.is_empty() {
        let delta = json!({ "type": "response.output_text.delta", "delta": text });
        body.push_str(&format!("data: {delta}\n\n"));
    }
    body.push_str(
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
    );
    body.push_str("data: [DONE]\n\n");
    body
}

pub(crate) fn notify_call(
    call_id: &str,
    text: &str,
    done: bool,
    output: Option<Value>,
    waiting: Option<&str>,
) -> (String, String, String) {
    let mut args = json!({ "text": text });
    if done {
        args["done"] = json!(true);
    }
    if let Some(o) = output {
        args["output"] = o;
    }
    if let Some(w) = waiting {
        args["waiting_on"] = json!(w);
    }
    ("parent_notify".into(), call_id.into(), args.to_string())
}

/// A parent session + supervisor wired to the test seams. The parent
/// agent is scripted (its provider); the children use the factory.
pub(crate) fn harness(
    dir: &std::path::Path,
    parent_bodies: Vec<String>,
    child_scripts: Vec<Vec<String>>,
    caps: SubAgents,
) -> (Arc<Supervisor>, Arc<TestBridge>) {
    let (sup, bridge, _) = harness_full(dir, parent_bodies, child_scripts, vec![], caps);
    (sup, bridge)
}

pub(crate) fn harness_full(
    dir: &std::path::Path,
    parent_bodies: Vec<String>,
    child_scripts: Vec<Vec<String>>,
    delays: Vec<Duration>,
    caps: SubAgents,
) -> (Arc<Supervisor>, Arc<TestBridge>, Arc<CannedFactory>) {
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: child_scripts,
        delays,
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let provider: Arc<dyn ChildProviderFactory> = factory.clone();
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.to_path_buf(),
        provider,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        caps,
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: Arc::clone(&bridge) as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
    });
    // A scripted parent loop (the parent is a full session).
    let mut store = SessionStore::for_workspace(dir, "parent");
    store.create().unwrap();
    let parent_provider = Arc::new(ScriptedProvider::new(parent_bodies));
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::agent_tool_specs(),
        cwd: dir.to_path_buf(),
        provider: parent_provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: None,
        om_model: String::new(),
        subagents: Some(Arc::clone(&sup)),
        child: None,
    }));
    sup.attach_parent(parent);
    (sup, bridge, factory)
}

/// A scripted parent provider: one canned body per call.
pub(crate) struct ScriptedProvider {
    bodies: Vec<String>,
    index: AtomicUsize,
}
impl ScriptedProvider {
    pub(crate) fn new(bodies: Vec<String>) -> Self {
        Self {
            bodies,
            index: AtomicUsize::new(0),
        }
    }
}
impl provider::TurnProvider for ScriptedProvider {
    fn call<'a>(
        &self,
        req: &ResponseRequest,
        sink: &'a mut dyn TurnSink,
    ) -> provider::ProviderTurn<'a> {
        let i = self.index.fetch_add(1, Ordering::SeqCst);
        let body = self
            .bodies
            .get(i % self.bodies.len())
            .cloned()
            .unwrap_or_else(|| sse("", &[]));
        provider::canned(&body).call(req, sink)
    }
}

pub(crate) fn spawn_args(brief: &str, mode: &str) -> Value {
    json!({ "type": "general", "brief": brief, "context_mode": mode })
}

pub(crate) fn entry_by_event(
    dir: &std::path::Path,
    session: &str,
    event: &str,
) -> Option<crate::session::Entry> {
    let mut store = SessionStore::for_workspace(dir, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == KIND_SUBAGENT && e.payload["event"] == json!(event))
}

pub(crate) fn wait_for(cond: impl Fn() -> bool) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
