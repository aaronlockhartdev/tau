//! End-to-end acceptance driver (ticket #27).
//!
//! Legs: `b` multi-turn with the four core tools (live), `c` a sub-agent
//! spawned by the model plus its task pointer (live), `d` OM compaction on a
//! synthesized long session (live observe/reflect), `e` branching + manual
//! archive round-trip (offline). Live legs read `TAU_ENDPOINT` / `TAU_MODEL`
//! (or `--endpoint` / `--model`) and cap every generation at 300 output
//! tokens; the script gates them, the driver does not.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{Value, json};
use tau_core::agent::{AgentSession, Lane, SessionParams, TurnConfig};
use tau_core::agent_type::builtin_general;
use tau_core::config::{Om, Provider, Requests, SubAgents};
use tau_core::om::OmRecord;
use tau_core::om_integration::OmState;
use tau_core::provider;
use tau_core::session::{Entry, SessionStore};
use tau_core::subagent::{
    BoxedDrive, ChildDriver, ChildProviderFactory, SpawnNotice, StateNotice, SubagentBridge,
    Supervisor, SupervisorParams, WakeNotice,
};
use tau_core::task::KIND_TASK;
use tau_core::tools;

const CAP: u64 = 300;

struct Ctx {
    endpoint: String,
    model: String,
}

fn capped() -> TurnConfig {
    TurnConfig {
        max_output_tokens: Some(CAP),
        reasoning: None,
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
    let p = Provider {
        base_url: ctx.endpoint.clone(),
        key_env: String::new(),
        models: vec![ctx.model.clone()],
    };
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
    store.open().expect("open session");
    (id, store)
}

fn all_entries(store: &SessionStore) -> Vec<Entry> {
    store.entries_range(0, usize::MAX).expect("read entries")
}

fn tool_calls(entries: &[Entry]) -> Vec<&str> {
    entries
        .iter()
        .filter(|e| e.kind == tau_core::agent::KIND_TOOL)
        .filter_map(|e| e.payload.get("tool").and_then(Value::as_str))
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

const PROMPT: &str =
    "You are Tau, a coding agent. Use the tools to do what is asked, exactly and minimally.";

// ---------------------------------------------------------------- leg b

async fn leg_b(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    std::fs::write(ws.path().join("notes.txt"), "line1\nline2\nline3\n").unwrap();
    let (id, store) = new_session(ws.path());
    let (_, prov) = production(ctx);
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: PROMPT.into(),
        model: ctx.model.clone(),
        tools: tools::tool_specs(),
        cwd: ws.path().into(),
        provider: prov,
        tool_batch_on_force: Default::default(),
        turn: capped(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send(
        "Do four things, one tool call each, in order: (1) read notes.txt; (2) write a file b.txt whose content is exactly 'b'; (3) edit b.txt so its content is exactly 'b2'; (4) run the bash command `echo done`. When all four are done reply exactly: all four done.",
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
                "leg b: the model never called {name} (saw {calls:?})"
            ));
        }
    }
    let golden = std::fs::read_to_string(ws.path().join("b.txt")).map_err(|e| e.to_string())?;
    if golden.trim() != "b2" {
        return Err(format!(
            "leg b: the golden file is {golden:?}, expected 'b2'"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- leg c

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
/// to the parent loop exactly as the app does (send_notified, provenance).
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
        p.send_notified(text, n.child.clone());
    }
}

async fn leg_c(ctx: &Ctx) -> Result<(), String> {
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
        system_prompt: PROMPT.into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: Default::default(),
        turn: capped(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![builtin_general()],
        bridge: bridge.clone(),
        driver: Arc::new(AcceptanceDriver),
    });
    let agent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: PROMPT.into(),
        model: ctx.model.clone(),
        tools: tools::agent_tool_specs(),
        cwd: ws.path().into(),
        provider: prov,
        tool_batch_on_force: Default::default(),
        turn: capped(),
        om: None,
        om_model: String::new(),
        subagents: Some(sup),
        child: None,
    }));
    {
        *bridge.parent.lock().unwrap() = Arc::downgrade(&agent);
    }

    agent
        .send(
            "First create one task with task_create (title 'child ping', a single acceptance criterion 'the child reports ok'). Then use subagent_spawn to spawn one sub-agent (type 'general', context_mode 'fresh') and assign it that task. Its brief: 'Satisfy the task's criterion by adding passing evidence, finish the task, then call parent_notify with done true and output {\"status\": \"ok\"}.' When the sub-agent reports back, reply exactly: child done.",
            Lane::Steering,
        );
    agent.process().await.map_err(|e| e.to_string())?;

    // The child's drive runs on the supervisor's task; the parent wakes via
    // the bridge (a queued notification). Run the parent's follow-up turns
    // while they are pending.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    while agent.has_pending() && std::time::Instant::now() < deadline {
        agent.process().await.map_err(|e| e.to_string())?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    if agent.has_pending() {
        return Err("leg c: the parent still has a pending wake after 180 s".into());
    }

    // The child session file exists alongside the parent's.
    let files = session_files(ws.path());
    if files.len() < 2 {
        return Err(format!(
            "leg c: expected a child session file, found {files:?}"
        ));
    }
    drop(agent);

    let parent = SessionStore::for_workspace(ws.path(), &id);
    let pentries = all_entries(&parent);
    // The wake landed on the parent's branch tagged with the child's id.
    let child_id = files
        .iter()
        .find_map(|f| {
            f.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
        })
        .filter(|n| n.as_str() != id.as_str())
        .ok_or("leg c: no child session file")?;
    let wake = pentries.iter().any(|e| {
        e.kind == tau_core::agent::KIND_USER
            && e.payload.get("source").and_then(Value::as_str) == Some(child_id.as_str())
    });
    if !wake {
        return Err("leg c: no wake entry from the child on the parent's branch".into());
    }
    // The task resolved through the done gate: the parent's copy is a
    // status pointer in a terminal state (completed / handed_off / blocked).
    let tasks: Vec<&Entry> = pentries.iter().filter(|e| e.kind == KIND_TASK).collect();
    let last = tasks.last().ok_or("leg c: no task entry on the parent")?;
    let status = last
        .payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !matches!(status, "completed" | "handed_off" | "blocked" | "done") {
        return Err(format!("leg c: the task pointer is stuck at {status:?}"));
    }
    // The child's own session carries the task as its live record.
    let child_store = SessionStore::for_workspace(ws.path(), &child_id);
    if !all_entries(&child_store)
        .iter()
        .any(|e| e.kind == KIND_TASK)
    {
        return Err(
            "leg c: the child's session has no task record (assignment never copied it)".into(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- leg d

fn prose(i: usize) -> String {
    let mut s = String::new();
    for j in 0..24 {
        s.push_str(&format!(
            "Note {i}-{j}: the expedition crossed the northern pass under a low amber sky, \
             mapping the river delta and cataloguing the stone markers found at every bend. "
        ));
    }
    s
}

async fn leg_d(ctx: &Ctx) -> Result<(), String> {
    let ws = temp_ws();
    let (id, mut store) = new_session(ws.path());
    // A synthesized long raw window (no model needed for the raw): ~48 KB of
    // prose ≈ 12k tokens — far past the lowered observe threshold.
    let mut prev: Option<String> = None;
    for i in 0..40 {
        let e = store
            .append("message", json!({ "text": prose(i) }), prev.as_deref())
            .map_err(|e| e.to_string())?;
        prev = Some(e.id);
    }
    let store = SessionStore::for_workspace(ws.path(), &id);
    let om = OmState::from_config(
        &Om {
            om_model: String::new(),
            observe_threshold: 4000,
            reflect_threshold: 250,
            buffer_increment: 500,
        },
        OmRecord::default(),
    );
    let (_, prov) = production(ctx);
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: PROMPT.into(),
        model: ctx.model.clone(),
        tools: tools::tool_specs(),
        cwd: ws.path().into(),
        provider: prov,
        tool_batch_on_force: Default::default(),
        turn: capped(),
        om: Some(om),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
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
    if om_entries.len() < 1 {
        return Err("leg d: no om entry after the observe threshold was crossed".into());
    }
    let record = OmState::load_record(&mut store).map_err(|e| e.to_string())?;
    if record.cursor.is_none() {
        return Err("leg d: the observation cursor did not advance".into());
    }
    if record.live_observations().is_empty() {
        return Err("leg d: the observation log is empty after a live observe".into());
    }
    if om_entries.len() < 2 {
        return Err("leg d: the reflector never fired (one om entry, expected two)".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- leg e

fn leg_e() -> Result<(), String> {
    let ws = temp_ws();
    let (id, mut store) = new_session(ws.path());
    // Trunk: three entries, then branch from the second.
    let mut prev: Option<String> = None;
    let mut second = String::new();
    for i in 0..3usize {
        let e = store
            .append(
                "message",
                json!({ "text": format!("trunk {i}") }),
                prev.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        if i == 1 {
            second = e.id.clone();
        }
        prev = Some(e.id);
    }
    // The branch: append to the second entry (a new parentId forks it).
    let b1 = store
        .append("message", json!({ "text": "branch 1" }), Some(&second))
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
        return Err("leg e: the archive file was not written".into());
    }
    store.unarchive().map_err(|e| e.to_string())?;
    let reopened = SessionStore::for_workspace(ws.path(), &id);
    let entries_after = all_entries(&reopened);
    if entries_before.len() != entries_after.len() {
        return Err(format!(
            "leg e: round-trip lost entries ({} -> {})",
            entries_before.len(),
            entries_after.len()
        ));
    }
    for (a, b) in entries_before.iter().zip(entries_after.iter()) {
        if a.id != b.id || a.parent != b.parent || a.payload != b.payload {
            return Err(format!("leg e: round-trip changed entry {}", a.id));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- main

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut leg = "";
    let mut endpoint = std::env::var("TAU_ENDPOINT")
        .unwrap_or_else(|_| "https://llms.aaronlockhart.dev/v1".into());
    let mut model = std::env::var("TAU_MODEL").unwrap_or_else(|_| "qwen3.8-27b".into());
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "b" | "c" | "d" | "e" => leg = args[i].as_str(),
            "--endpoint" => {
                i += 1;
                endpoint = args.get(i).cloned().unwrap_or_default();
            }
            "--model" => {
                i += 1;
                model = args.get(i).cloned().unwrap_or_default();
            }
            _ => {}
        }
        i += 1;
    }
    if leg.is_empty() {
        eprintln!("usage: tau-acceptance <b|c|d|e> [--endpoint URL] [--model NAME]");
        std::process::exit(2);
    }
    let ctx = Ctx { endpoint, model };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let start = std::time::Instant::now();
    let result = match leg {
        "b" => rt.block_on(leg_b(&ctx)),
        "c" => rt.block_on(leg_c(&ctx)),
        "d" => rt.block_on(leg_d(&ctx)),
        _ => rt.block_on(async { leg_e() }),
    };
    match result {
        Ok(()) => println!("leg {leg}: PASS in {:?}", start.elapsed()),
        Err(e) => {
            println!("leg {leg}: FAIL — {e}");
            std::process::exit(1);
        }
    }
}
