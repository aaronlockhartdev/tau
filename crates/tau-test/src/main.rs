//! End-to-end acceptance driver (ticket #27).
//!
//! Suites: `accept-tools` multi-turn with the four core tools,
//! `accept-subagent` a sub-agent spawned by the scripted model plus its
//! plan files, `accept-om` OM compaction on a synthesized long session
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
use tau_core::config::{Gates, Om, OmRetries, Provider, Requests, SubAgents, ToolBatchPolicy};
use tau_core::harness::SessionRole;
use tau_core::om::OmRecord;
use tau_core::om_integration::OmState;
use tau_core::provider;
use tau_core::session::{Entry, SessionStore};
use tau_core::subagent::{
    BoxedDrive, ChildDriver, ChildProviderFactory, ChildState, KIND_SUBAGENT, ReadoptNotice,
    ReclaimNotice, SpawnNotice, StateNotice, SubagentBridge, Supervisor, SupervisorParams,
    WakeNotice,
};
use tau_core::tools;
use tau_mock_llm::server::MODEL_ID;

mod tests_accept;

const CAP: u64 = 300;

use crate::tests_accept::{
    accept_gate, accept_om, accept_reclaim, accept_sender_labels, accept_subagent, accept_tools,
    core_offline,
};
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
const TOOLS_PROMPT: &str = "You have the tools read, write, edit, and bash. Use them as instructed; edit takes the 4-letter anchors from read output.";

/// accept-subagent's parent gets the sub-agent tools named too.
const SUBAGENT_PROMPT: &str = "You have the tools read, write, edit, bash, and the sub-agent tools (subagent_spawn, subagent_message, subagent_stop, subagent_state). Use them as instructed.";
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
            "accept-tools"
            | "accept-subagent"
            | "accept-reclaim"
            | "accept-om"
            | "accept-gate"
            | "accept-sender-labels"
            | "core" => {
                suite = args[i].as_str();
            }
            _ => {}
        }
        i += 1;
    }
    if suite.is_empty() {
        eprintln!(
            "usage: tau-test <accept-tools|accept-subagent|accept-reclaim|accept-om|accept-gate|accept-sender-labels|core>"
        );
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
        "accept-reclaim" => rt.block_on(accept_reclaim(&ctx)),
        "accept-om" => rt.block_on(accept_om(&ctx)),
        "accept-gate" => rt.block_on(accept_gate(&ctx)),
        "accept-sender-labels" => rt.block_on(accept_sender_labels(&ctx)),
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
