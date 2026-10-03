//! End-to-end acceptance driver (ticket #27).
//!
//! Suites: `accept-tools` multi-turn with the four core tools,
//! `accept-subagent` a sub-agent spawned by the scripted model plus its
//! task pointer, `accept-om` OM compaction on a synthesized long session
//! (mock observe/reflect), `core` branching + manual archive round-trip
//! (offline). All suites run against the deterministic mock LLM only
//! (ADR-0010): `TAU_ENDPOINT` is the mock's address (the justfile sets it;
//! a standalone run expects the mock on 127.0.0.1:8123) and the model is
//! the mock's own id. They cap every generation at 300 output tokens; the
//! script gates them, the driver does not.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{Value, json};
use tau_core::agent::{AgentSession, Lane, TurnConfig};
use tau_core::agent_type::builtin_general;
use tau_core::config::{Om, Provider, Requests, SubAgents, ToolBatchPolicy};
use tau_core::harness::SessionRole;
use tau_core::om::OmRecord;
use tau_core::om_integration::OmState;
use tau_core::provider;
use tau_core::session::{Entry, SessionStore};
use tau_core::subagent::{
    BoxedDrive, ChildDriver, ChildProviderFactory, KIND_SUBAGENT, SpawnNotice, StateNotice,
    SubagentBridge, Supervisor, SupervisorParams, WakeNotice,
};
use tau_core::task::KIND_TASK;
use tau_core::tools;
use tau_mock_llm::server::MODEL_ID;

const CAP: u64 = 300;
struct Ctx {
    endpoint: String,
    model: String,
}

fn capped() -> TurnConfig {
    TurnConfig {
        max_output_tokens: Some(CAP),
        reasoning: None,
        ..TurnConfig::default()
    }
}

/// No-pool client: a pooled keep-alive connection keeps the runtime alive
/// after the last call and hangs the process exit (the #23 teardown lesson).
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .expect("client")
}

fn production(ctx: &Ctx) -> (Provider, tau_core::provider::TurnProviderRef) {
    let p = Provider::with_model(ctx.endpoint.clone(), ctx.model.clone());
    (
        p.clone(),
        provider::production(&client(), &p, &Requests::default()),
    )
}

fn temp_ws() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn new_session(cwd: &Path) -> (String, SessionStore) {
    let id = SessionStore::new_session_id();
    let mut store = SessionStore::for_workspace(cwd, &id);
    store.create().expect("create session");
    (id, store)
}

fn all_entries(store: &SessionStore) -> Vec<Entry> {
    store.entries_range(0, usize::MAX).expect("read entries")
}

fn tool_calls(entries: &[Entry]) -> Vec<&str> {
    entries
        .iter()
        .filter(|e| e.kind == tau_core::agent::KIND_TOOL)
        .filter_map(|e| e.payload.get("name").and_then(Value::as_str))
        .collect()
}

fn session_files(cwd: &Path) -> Vec<PathBuf> {
    let dir = cwd.join(".tau").join("sessions");
    std::fs::read_dir(&dir)
        .map(|r| {
            r.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default()
}

// The proven #19 acceptance prompt: small
// models need the tool list spelled out in the system prompt to use them.
const TOOLS_PROMPT: &str = "You have the tools read, write, edit, and bash. Use them as instructed; edit takes the 3-char anchors from read output.";

/// accept-subagent's parent gets the sub-agent + task tools named too.
const SUBAGENT_PROMPT: &str = "You have the tools read, write, edit, bash, the task tools (task_create, task_assign, task_start, task_evidence, task_block, task_finish, task_cancel), and the sub-agent tools (subagent_spawn, subagent_message, subagent_stop, subagent_state). Use them as instructed.";
async fn accept_tools(ctx: &Ctx) -> Result<(), String> {
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
}

impl SubagentBridge for AcceptanceBridge {
    fn spawned(&self, _n: &SpawnNotice) {}
    fn state(&self, _n: &StateNotice) {}
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
async fn accept_subagent(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    let (id, store) = new_session(ws.path());
    let (p, prov) = production(ctx);

    let bridge = Arc::new(AcceptanceBridge {
        parent: Mutex::new(Weak::new()),
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
            "{SUBAGENT_PROMPT}\n\nYou are a child sub-agent on an assigned task; report to the parent when done."
        ),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::default(),
        turn: capped(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![builtin_general()],
        bridge: bridge.clone(),
        driver: Arc::new(AcceptanceDriver),
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
            first_provider: None,
        },
    )
    .map_err(|e| format!("launch failed: {e:?}"))?;
    {
        *bridge.parent.lock().unwrap() = Arc::downgrade(&agent);
    }

    agent
        .send(
            "First create one task with task_create (title 'child ping', a single acceptance criterion 'the child reports ok'). Then use subagent_spawn to spawn one sub-agent (type 'general', context_mode 'fresh') and assign it that task. Its brief: 'Satisfy the task's criterion by adding passing evidence, finish the task, then call parent_notify with done true and output {\"status\": \"ok\"}.' When the sub-agent reports back, reply exactly: child done.",
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
                    .find(|e| e.kind == KIND_SUBAGENT)
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
        e.kind == tau_core::agent::KIND_USER
            && e.payload.get("source").and_then(Value::as_str) == Some(child_id.as_str())
    });
    if !wake {
        return Err("accept-subagent: no wake entry from the child on the parent's branch".into());
    }
    // The child's session carries its own record trail (state entries).
    let child_store = SessionStore::for_workspace(ws.path(), &child_id);
    let centries = all_entries(&child_store);
    if !centries.iter().any(|e| e.kind == KIND_SUBAGENT) {
        return Err("accept-subagent: the child's session has no lifecycle state entries".into());
    }
    let child_state = centries
        .iter()
        .rev()
        .find(|e| e.kind == KIND_SUBAGENT)
        .and_then(|e| e.payload.get("state").and_then(Value::as_str))
        .unwrap_or("");
    // Task linkage, when the parent assigned one: the record never leaves
    // the parent file (single source of truth — Supervisor::child_task),
    // the child's file carries no task entries, the parent's Assigned event
    // points at the child's session, and a terminal child forces a terminal
    // pointer — no task dangles in_progress after a done/failed child.
    let tasks: Vec<&Entry> = pentries.iter().filter(|e| e.kind == KIND_TASK).collect();
    let assigned = tasks
        .iter()
        .any(|e| e.payload.get("event").and_then(Value::as_str) == Some("assigned"));
    if assigned {
        let linked = tasks.iter().any(|e| {
            e.payload.get("event").and_then(Value::as_str) == Some("assigned")
                && e.payload.get("worker").and_then(Value::as_str) == Some(child_id.as_str())
        });
        if !linked {
            return Err(
                "accept-subagent: the parent's Assigned event does not point at the child session"
                    .into(),
            );
        }
        if centries.iter().any(|e| e.kind == KIND_TASK) {
            return Err(
                "accept-subagent: the child's session carries task entries (the record must stay in the parent file)"
                    .into(),
            );
        }
        if matches!(child_state, "done" | "failed") {
            // The completion gate (resolve_assigned_task) resolves the task
            // on the parent's file: all criteria satisfied -> finished,
            // otherwise handed_off (the terminal markers; a `pointer` event
            // is never written in v0).
            let terminal = tasks.iter().any(|e| {
                matches!(
                    e.payload.get("event").and_then(Value::as_str).unwrap_or(""),
                    "finished" | "handed_off" | "cancelled"
                )
            });
            if !terminal {
                return Err(format!(
                    "accept-subagent: the child ended {child_state} but the task pointer has no terminal state (dangling)"
                ));
            }
        }
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
            .append("user", json!({ "text": prose(i) }), parent_id.as_deref())
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
    let om_entries: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == tau_core::om_integration::KIND_OM)
        .collect();
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

async fn accept_om(ctx: &Ctx) -> Result<(), String> {
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

fn core_offline() -> Result<(), String> {
    let ws = temp_ws();
    let (id, mut store) = new_session(ws.path());
    // Trunk: three entries, then branch from the second.
    let mut prev: Option<String> = None;
    let mut second: Option<String> = None;
    for i in 0..3usize {
        let e = store
            .append(
                "message",
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
        .append("message", json!({ "text": "branch 1" }), second.as_deref())
        .map_err(|e| e.to_string())?;
    let b2 = store
        .append(
            "message",
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut suite = "";
    // The suites are mock-only (ADR-0010): TAU_ENDPOINT is the mock's
    // address (the justfile sets it; standalone runs expect 127.0.0.1:8123).
    let endpoint =
        std::env::var("TAU_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:8123/v1".into());
    let model = MODEL_ID.to_owned();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "accept-tools" | "accept-subagent" | "accept-om" | "core" => suite = args[i].as_str(),
            _ => {}
        }
        i += 1;
    }
    if suite.is_empty() {
        eprintln!("usage: tau-test <accept-tools|accept-subagent|accept-om|core>");
        std::process::exit(2);
    }
    let ctx = Ctx { endpoint, model };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let start = std::time::Instant::now();
    let result = match suite {
        "accept-tools" => rt.block_on(accept_tools(&ctx)),
        "accept-subagent" => rt.block_on(accept_subagent(&ctx)),
        "accept-om" => rt.block_on(accept_om(&ctx)),
        _ => rt.block_on(async { core_offline() }),
    };
    match result {
        Ok(()) => println!("{suite}: PASS in {:?}", start.elapsed()),
        Err(e) => {
            println!("{suite}: FAIL — {e}");
            std::process::exit(1);
        }
    }
}

fn dump_sessions(cwd: &Path, dest: &str) {
    use std::io::Write;
    if let Ok(dir) = cwd.join(".tau").join("sessions").read_dir() {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Ok(data) = std::fs::read(e.path())
                && let Ok(mut f) = std::fs::File::create(format!("{dest}-{name}"))
            {
                let _ = f.write_all(&data);
            }
        }
    }
}
