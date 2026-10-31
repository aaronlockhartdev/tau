//! #348: the OM round-trips are addressed at the provider their `om_model` id
//! names (#336's rule, spec §6.7), not at the provider the session happens to
//! ride. The discriminator is the endpoint: each provider's `base_url` is a
//! capture server that records the requests it receives, so a test names the
//! provider that answered the Observer and the model key it was asked for.
//!
//! The session's own provider is `zzz` in every leg, so an OM call that lands
//! there is the pre-#348 route riding `ForwardingProvider::route`.

use super::*;
use crate::harness::testkit::*;

use std::io::Read as _;
use std::io::Write as _;

/// One captured request: the model its `model` field names.
fn captured_model(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("model")?
        .as_str()
        .map(str::to_owned)
}

/// The capture server: every request body is recorded, every call answers the
/// `answer` text as a completed stream. One thread per connection, one
/// connection per call (the client's pool is idle-free in tests).
fn capture_server(answer: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let std_addr = format!("http://{addr}/v1");
    let thread_seen = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            // A failed accept is the OS, not the harness: a server that stops
            // listening answers nothing, and a silent memory pass reads as a
            // missing record.
            let Ok(mut stream) = stream else { continue };
            let seen = Arc::clone(&thread_seen);
            let answer = answer.clone();
            std::thread::spawn(move || {
                let mut buf = [0_u8; 8192];
                let mut body = Vec::new();
                let mut content_length: Option<usize> = None;
                loop {
                    let read = stream.read(&mut buf).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    body.extend_from_slice(&buf[..read]);
                    let text = String::from_utf8_lossy(&body).into_owned();
                    if content_length.is_none()
                        && let Some(line) = text.lines().find(|l| {
                            l.trim_start()
                                .to_ascii_lowercase()
                                .starts_with("content-length:")
                        })
                    {
                        content_length = line
                            .split(':')
                            .nth(1)
                            .and_then(|v| v.trim().parse::<usize>().ok());
                    }
                    if body.len() >= content_length.unwrap_or(usize::MAX) {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&body).into_owned();
                let payload = request.find("\r\n\r\n").map_or_else(String::new, |i| {
                    String::from_utf8_lossy(&request.as_bytes()[i + 4..]).into_owned()
                });
                seen.lock().push(payload);
                let sse = format!(
                    "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{answer}\"}}\n\n\
                     data: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n\
                     data: [DONE]\n\n"
                );
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
                    sse.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(sse.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    (std_addr, seen)
}

/// A two-provider config: the session rides `zzz` (the reasoning script's
/// capture server), `aaa` — the map's first entry — is the OM endpoint.
struct Fixture {
    system: tempfile::TempDir,
    project: tempfile::TempDir,
    /// The provider the session's model is declared on.
    zzz_base: String,
    /// The provider an `om_model` naming `aaa` is addressed at.
    aaa_base: String,
    zzz_seen: Arc<Mutex<Vec<String>>>,
    aaa_seen: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        // The answer is parseable as observations: with no `om_model` the Observer
        // is addressed at zzz, so zzz's server answers the memory pass too.
        let (zzz_base, zzz_seen) = capture_server("<observations>ridden</observations>".into());
        let (aaa_base, aaa_seen) = capture_server("<observations>routed</observations>".into());
        Self {
            system: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
            zzz_base,
            aaa_base,
            zzz_seen,
            aaa_seen,
        }
    }

    /// The config, with the `[om]` block's thresholds tuned so one message
    /// activates the Observer and `om` spells the model line (empty = no
    /// `om_model` at all).
    fn write_config(&self, om: &str) {
        let model_line = if om.is_empty() {
            String::new()
        } else {
            format!("om_model = \"{om}\"\n")
        };
        let raw = format!(
            r#"
[providers.aaa]
base_url = "{aaa}"

[providers.aaa.models.om-model]
context_window = 32768
[providers.aaa.models.shared]
context_window = 32768

[providers.zzz]
base_url = "{zzz}"

[providers.zzz.models.main-model]
context_window = 32768
[providers.zzz.models.shared]
context_window = 32768

[generation]
default_model = "zzz/main-model"

[om]
{model_line}observe_threshold = 1
reflect_threshold = 100000
buffer_increment = 100000
buffer_tokens = 0
"#,
            aaa = self.aaa_base,
            zzz = self.zzz_base,
        );
        write_config_fixture(self.system.path(), &raw);
    }

    /// The core over that config, and a root session on `zzz`.
    async fn start(&self, om: &str) -> (Arc<Core>, String) {
        self.write_config(om);
        let core = CoreBuilder::default_system()
            .unwrap()
            .with_system_dir(self.system.path().to_path_buf())
            .with_home(self.system.path().to_path_buf())
            .build()
            .unwrap();
        let ws = open_ws(&core, self.project.path()).await;
        let id = match core
            .dispatch(Command::SessionNew {
                workspace: ws.id.clone(),
                title: None,
                base_prompt: None,
            })
            .unwrap()
        {
            CommandOutput::Session { session } => session.id,
            other => panic!("expected a session: {other:?}"),
        };
        (core, id)
    }
}

/// The turn settles: the session is idle and its file holds an assistant entry.
async fn settle(core: &Arc<Core>, session: &str) {
    let live = core.live(session).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let idle = !live.turn.load(Ordering::SeqCst);
        let mut store = SessionStore::for_workspace(&live.cwd, session);
        let answered = store
            .open()
            .ok()
            .and_then(|()| store.entries_range(0, usize::MAX).ok())
            .is_some_and(|entries| entries.iter().any(|e| e.kind == EntryKind::Assistant));
        if idle && answered {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn never settled"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The brief a child is spawned with: text no other transcript in the fixture
/// contains, so a request body is attributable to the child.
const BRIEF: &str = "inspect the parser";
/// The message `send` delivers, for the same reason on the parent's side.
const SEND_TEXT: &str = "work through this task carefully";
/// The observer prompt's opening: marks a request as a memory pass, not a model
/// call.
const MEMORY_INSTRUCTIONS: &str = "You are the memory system of a coding agent";

/// Deliver `text` to `session` and settle its turn.
async fn send_text(core: &Arc<Core>, session: &str, text: &str) {
    core.dispatch(Command::MessageSend {
        session: session.into(),
        text: text.into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    settle(core, session).await;
}

/// Send one message whose size crosses the observe threshold.
async fn send(core: &Arc<Core>, session: &str) {
    send_text(core, session, SEND_TEXT).await;
}

/// The diagnostic payloads of a session's file, as JSON.
fn diagnostics(cwd: &Path, session: &str) -> Vec<Value> {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Diagnostic)
        .map(|e| e.payload.clone())
        .collect()
}

/// The `om_failed` records' `(stage, reason)`, in append order.
fn om_failures(cwd: &Path, session: &str) -> Vec<(String, String)> {
    diagnostics(cwd, session)
        .iter()
        .filter(|p| p["code"] == "om_failed")
        .map(|p| {
            (
                p["stage"].as_str().unwrap_or_default().to_owned(),
                p["reason"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The newest `om` record's model, waited for: a memory pass runs at the turn's
/// end, so its record lands after the turn is idle — reading the file once
/// races it.
async fn om_record_model_settled(cwd: &Path, session: &str) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(model) = om_record_model(cwd, session) {
            return Some(model);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The newest `om` record's model field: the name the pass wrote.
fn om_record_model(cwd: &Path, session: &str) -> Option<String> {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .rev()
        .find(|e| e.kind == EntryKind::Om)
        .and_then(|e| e.payload.get("om_model"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The positive: `om_model = "aaa/om-model"` while the session rides `zzz`.
/// The Observer call goes to **aaa** — the provider the id names — carrying the
/// prefix-stripped key, and its observation lands.
#[tokio::test]
async fn an_om_model_prefixed_id_is_addressed_at_the_provider_it_names() {
    let f = Fixture::new();
    let (core, session) = f.start("aaa/om-model").await;
    send(&core, &session).await;

    let aaa = f.aaa_seen.lock().clone();
    assert_eq!(
        captured_model(aaa.first().map(String::as_str).unwrap_or_default())
            .as_deref()
            .unwrap_or_default(),
        "om-model",
        "aaa (the provider the id names) answered the Observer with the \
         prefix-stripped key; the OM call rode the session's provider instead: \
         aaa saw {aaa:?}"
    );
    let zzz = f.zzz_seen.lock().clone();
    assert!(
        zzz.iter().all(|body| body.contains("You are Tau")),
        "every call at zzz is a main turn, never a memory pass: {zzz:?}"
    );
    assert_eq!(
        om_record_model(f.project.path(), &session).as_deref(),
        Some("om-model"),
        "the durable record names the model that answered"
    );
}

/// A bare key declared by exactly one provider is routed there (§6.7), and the
/// prefix-less spelling changes nothing about the endpoint.
#[tokio::test]
async fn a_bare_om_model_key_is_addressed_at_the_provider_that_declares_it() {
    let f = Fixture::new();
    let (core, session) = f.start("om-model").await;
    send(&core, &session).await;

    let aaa = f.aaa_seen.lock().clone();
    assert!(
        !aaa.is_empty(),
        "the Observer reached the provider that declares `om-model`"
    );
    assert_eq!(
        captured_model(&aaa[0]).as_deref(),
        Some("om-model"),
        "the bare key reaches the wire unchanged: {aaa:?}"
    );
}

/// The negative: a prefix naming no configured provider is a refusal, never a
/// silent ride on the session's provider. The refusal is durable and names the
/// config key.
#[tokio::test]
async fn an_om_model_prefix_naming_no_provider_is_refused_and_recorded() {
    let f = Fixture::new();
    let (core, session) = f.start("nope/om-model").await;
    send(&core, &session).await;

    assert!(
        f.aaa_seen.lock().is_empty(),
        "a refusal sends nothing to the provider that names nothing"
    );
    let zzz = f.zzz_seen.lock().clone();
    assert!(
        zzz.iter().all(|body| body.contains("You are Tau")),
        "a refused `om_model` never rides the session's provider: {zzz:?}"
    );
    let failures = om_failures(f.project.path(), &session);
    assert!(
        failures.iter().any(|(stage, reason)| {
            stage == "route" && reason.contains("om.om_model") && reason.contains("nope/om-model")
        }),
        "the dead route is a durable record naming the config key and the id: {failures:?}"
    );
}

/// The negative: an id whose key the routed provider does not declare is the
/// same refusal (the pre-#348 path asked for it and spent the retry budget on
/// a permanent answer).
#[tokio::test]
async fn an_om_model_naming_an_undeclared_model_is_refused() {
    let f = Fixture::new();
    let (core, session) = f.start("aaa/ghost").await;
    send(&core, &session).await;

    assert!(
        f.aaa_seen.lock().is_empty(),
        "a key the provider does not declare is never asked for"
    );
    let failures = om_failures(f.project.path(), &session);
    assert!(
        failures
            .iter()
            .any(|(stage, reason)| stage == "route" && reason.contains("ghost")),
        "the refusal names the id it could not resolve: {failures:?}"
    );
}

/// #337 × #348: the reload reaches the OM route. The session opens with no
/// `om_model` (OM riding its own provider), the file gains one, and the next
/// turn's Observer is addressed at the provider the new id names.
#[tokio::test]
async fn a_config_move_reaches_the_om_route() {
    let f = Fixture::new();
    let (core, session) = f.start("").await;
    send(&core, &session).await;
    assert!(
        f.aaa_seen.lock().is_empty(),
        "no `om_model` configured: nothing reaches aaa"
    );

    f.write_config("aaa/om-model");
    core.reload_system_config();
    send(&core, &session).await;

    let aaa = f.aaa_seen.lock().clone();
    assert_eq!(
        captured_model(aaa.first().map(String::as_str).unwrap_or_default())
            .as_deref()
            .unwrap_or_default(),
        "om-model",
        "the reloaded `om_model` is routed at the provider its id names: aaa saw \
         {aaa:?}"
    );
}

/// The empty case is explicit: with no `om_model`, OM rides the session's own
/// address (spec §4) and the record says so, once — a log reading a configured
/// cheap-model setup is not the fact.
#[tokio::test]
async fn an_empty_om_model_stamps_the_record_with_the_model_it_rides() {
    let f = Fixture::new();
    let (core, session) = f.start("").await;
    send(&core, &session).await;

    assert_eq!(
        om_record_model_settled(f.project.path(), &session)
            .await
            .as_deref(),
        Some("main-model"),
        "the record names the model OM is riding"
    );
    let zzz = f.zzz_seen.lock().clone();
    assert_eq!(
        captured_model(zzz.last().map(String::as_str).unwrap_or_default()).as_deref(),
        Some("main-model"),
        "riding means the memory pass is addressed at the session's provider with the \
         key it declares: zzz saw {zzz:?}"
    );
    assert!(
        f.aaa_seen.lock().is_empty(),
        "no call reached a provider the config did not name for the memory passes"
    );
}

/// A `.md` agent type naming `model` as its own (spec §5.5), written before the
/// parent opens: the root assembly discovers types at launch.
fn write_agent_type(project: &Path, name: &str, model: &str) {
    let dir = project.join(".tau").join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: reviews\nmodel: {model}\n---\nYou review.\n"),
    )
    .unwrap();
}

fn spawn(core: &Arc<Core>, parent: &str, agent_type: &str) -> String {
    match core
        .dispatch(Command::SubagentSpawn {
            session: parent.into(),
            agent_type: agent_type.into(),
            brief: BRIEF.into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent.child,
        other => panic!("expected a subagent: {other:?}"),
    }
}

/// An endpoint received a request for `model`.
fn saw_model(seen: &Arc<Mutex<Vec<String>>>, model: &str) -> bool {
    seen.lock()
        .iter()
        .any(|payload| captured_model(payload).as_deref() == Some(model))
}

/// What an endpoint was asked for, model by model — the readable form for an
/// assertion message, where a payload dump would bury the answer.
fn models_seen(seen: &Arc<Mutex<Vec<String>>>) -> Vec<Option<String>> {
    seen.lock()
        .iter()
        .map(|payload| captured_model(payload))
        .collect()
}

/// The child leg end to end (#348 with #336): the parent rides `zzz`, the
/// child's agent type names a model of `aaa`, and `om_model` is a bare key both
/// providers declare. §6.7 answers that key per session, so the parent's memory
/// pass is addressed at zzz and the child's at aaa. The capture servers are the
/// discriminator: the factory's own route is a `Configured { model }`, which
/// names the key, not the endpoint — a pass that travelled to the parent's
/// provider would be invisible in the unit leg.
#[tokio::test]
async fn a_child_and_its_parent_answer_a_contested_om_key_at_their_own_providers() {
    let f = Fixture::new();
    write_agent_type(f.project.path(), "reviewer", "aaa/om-model");
    let (core, parent) = f.start("shared").await;
    let child = spawn(&core, &parent, "reviewer");
    send(&core, &parent).await;

    assert_eq!(
        stamped_om_model(f.project.path(), &child).await.as_deref(),
        Some("shared"),
        "the child's record names the key its pass was addressed with"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !(saw_memory_pass(&f.aaa_seen, "shared", BRIEF)
        && saw_memory_pass(&f.zzz_seen, "shared", SEND_TEXT))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the parent's pass at zzz and the child's at aaa: zzz saw {:?}, aaa saw {:?}",
            models_seen(&f.zzz_seen),
            models_seen(&f.aaa_seen)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_model(&f.aaa_seen, "om-model"),
        "the child's model calls ran at aaa, so the `shared` request there is \
         the child's memory pass: {:?}",
        models_seen(&f.aaa_seen)
    );
}

/// An endpoint ran a memory pass for `model` over a transcript containing `text`:
/// the model names the address, the transcript says which session ran there.
fn saw_memory_pass(seen: &Arc<Mutex<Vec<String>>>, model: &str, text: &str) -> bool {
    seen.lock().iter().any(|payload| {
        captured_model(payload).as_deref() == Some(model)
            && payload.contains(MEMORY_INSTRUCTIONS)
            && payload.contains(text)
    })
}

/// The newest `om` record's model, waited for past the unstamped ones: the
/// activation record names no model, and it lands before the pass's record.
async fn stamped_om_model(cwd: &Path, session: &str) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(model) = om_record_models(cwd, session).last().cloned().flatten()
            && !model.is_empty()
        {
            return Some(model);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Every `om` record's model field, in append order.
fn om_record_models(cwd: &Path, session: &str) -> Vec<Option<String>> {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Om)
        .map(|e| {
            e.payload
                .get("om_model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// The newest `om` record's model, waited for past `before` records.
async fn om_record_model_after(cwd: &Path, session: &str, before: usize) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let models = om_record_models(cwd, session);
        if models.len() > before {
            return models.last().cloned().flatten();
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A child already alive follows a config move: its route is re-derived at its
/// own turn boundary, so the pass after the edit is addressed where the new id
/// names it. The launch-time value is not the claim — a child whose route was
/// frozen at spawn keeps asking for the key it opened on, which is what this
/// leg refuses to accept (#348, #337, #347).
#[tokio::test]
async fn a_config_move_reaches_a_child_that_is_already_alive() {
    let f = Fixture::new();
    write_agent_type(f.project.path(), "reviewer", "aaa/om-model");
    let (core, parent) = f.start("shared").await;
    let child = spawn(&core, &parent, "reviewer");
    send(&core, &parent).await;

    let opened = om_record_models(f.project.path(), &child).len();
    assert_eq!(
        stamped_om_model(f.project.path(), &child).await.as_deref(),
        Some("shared"),
        "the child opens on a pass stamped with the contested key"
    );

    f.write_config("aaa/om-model");
    core.reload_system_config();
    // A pass runs at a turn's end only once the buffer crosses `observe`, so the
    // turn after the edit says enough to be worth reflecting on.
    send_text(&core, &child, &BRIEF.repeat(200)).await;

    assert_eq!(
        om_record_model_after(f.project.path(), &child, opened)
            .await
            .as_deref(),
        Some("om-model"),
        "the pass after the edit is stamped with the model the new id selects"
    );
    assert!(
        saw_memory_pass(&f.aaa_seen, "om-model", BRIEF),
        "the child's pass after the edit asks the provider the new id names for \
         its key: {:?}",
        models_seen(&f.aaa_seen)
    );
}
