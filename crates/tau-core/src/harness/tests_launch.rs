//! Direct construction tests for the session constructor (R2): the role's
//! wiring defaults, without a dispatch round-trip.

use super::{
    AgentSession, Config, Core, CoreBuilder, Event, SessionRole, SessionStore,
    testkit::{collect_events, open_ws, providers},
};
use crate::agent_type::builtin_general;
use crate::config::{Gates, Om, Provider, SubAgents, ToolBatchPolicy};
use crate::subagent::testkit::{CannedFactory, TestBridge, TestDriver};
use crate::subagent::{Supervisor, SupervisorParams};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

fn canned_provider() -> crate::provider::TurnProviderRef {
    crate::provider::canned("data: [DONE]\n\n")
}

fn fresh_store(cwd: &std::path::Path, id: &str) -> SessionStore {
    let mut store = SessionStore::for_workspace(cwd, id);
    store.create().unwrap();
    store
}

#[tokio::test]
async fn launch_root_builds_the_supervisor_and_wires_the_events() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());
    let collected = collect_events(&core);

    let config = core.system_config();
    let first = config
        .providers
        .iter()
        .next()
        .map(|(name, p)| (name.clone(), p.clone()))
        .expect("the test config carries the dev provider");
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core.clone()),
            workspace: Some(ws.clone()),
            config: Some(config),
            provider: canned_provider(),
            supervisor: None,
            system_prompt: None,
            base_prompt: None,
            first_provider: Some(first),
        },
    )
    .unwrap();

    // The role's wiring defaults: the supervisor is built from the config
    // (the root's child surface) and the model is resolved from it.
    assert!(agent.subagents().is_some());
    assert_eq!(agent.model(), "model");

    // The entry hook is wired: an append rides the channel as an
    // EntryUpsert (ADR-0008's file-line tee).
    agent
        .append_entry("user", serde_json::json!({ "text": "hi" }))
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let seen = collected.lock().unwrap().iter().any(
            |e| matches!(e, Event::EntryUpsert { session, .. } if session == &agent_sid(&agent)),
        );
        if seen {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no EntryUpsert for the launched session within 5 s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn agent_sid(agent: &AgentSession) -> String {
    // The store is consumed by the constructor: the id is re-read through
    // the session's own store.
    agent.with_store(|s| s.id().to_owned())
}

#[tokio::test]
async fn launch_root_without_providers_is_a_config_error() {
    let core = CoreBuilder::custom(std::collections::BTreeMap::new())
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());

    let err = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core),
            workspace: Some(ws),
            config: Some(Config::default()),
            provider: canned_provider(),
            supervisor: None,
            system_prompt: None,
            base_prompt: None,
            first_provider: None,
        },
    )
    .err()
    .expect("launch fails without providers");
    assert!(
        matches!(
            err,
            tau_protocol::ProtocolError::Other { ref message }
                if message == "no providers configured; add a [providers.x] section"
        ),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn launch_root_with_a_modelless_provider_names_it() {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "dev".to_owned(),
        Provider {
            base_url: "http://127.0.0.1:9/v1".into(),
            key_env: String::new(),
            models: std::collections::BTreeMap::new(),
        },
    );
    let core = CoreBuilder::custom(providers).build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());

    let config = core.system_config();
    let first = config
        .providers
        .iter()
        .next()
        .map(|(name, p)| (name.clone(), p.clone()))
        .expect("the test config carries the dev provider");
    let err = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core.clone()),
            workspace: Some(ws),
            config: Some(config),
            provider: canned_provider(),
            supervisor: None,
            system_prompt: None,
            base_prompt: None,
            first_provider: Some(first),
        },
    )
    .err()
    .expect("launch fails with a modelless provider");
    assert!(
        matches!(
            err,
            tau_protocol::ProtocolError::Other { ref message }
                if message == "provider dev has no models"
        ),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn launch_root_session_is_freed_when_dropped() {
    // The ticket #43 lifetime fix: the queue hook and the supervisor's
    // parent capture are `Weak`, so the last external `Arc` frees the
    // session (a strong self-capture would keep it alive).
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ws = open_ws(&core, dir.path()).await;
    let store = fresh_store(dir.path(), &SessionStore::new_session_id());
    let config = core.system_config();
    let first = config
        .providers
        .iter()
        .next()
        .map(|(name, p)| (name.clone(), p.clone()))
        .expect("the test config carries the dev provider");
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core.clone()),
            workspace: Some(ws.clone()),
            config: Some(config),
            provider: canned_provider(),
            supervisor: None,
            system_prompt: None,
            base_prompt: None,
            first_provider: Some(first),
        },
    )
    .unwrap();
    assert!(
        Arc::try_unwrap(agent).is_ok(),
        "the session must be droppable"
    );
}

#[test]
fn launch_seam_session_is_freed_when_dropped() {
    // The seam flavour of the same fix: the pre-built supervisor holds
    // its parent as a `Weak` (ticket #43), so the driver's `Arc` is the
    // only strong reference.
    let dir = tempfile::tempdir().unwrap();
    let store = fresh_store(dir.path(), "parent");
    let bridge: Arc<dyn crate::subagent::SubagentBridge> = Arc::new(TestBridge::default());
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: Arc::new(CannedFactory {
            scripts: vec![],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
            instructions: Arc::new(Mutex::new(Vec::new())),
        }),
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: crate::agent::TurnConfig::default(),
        gates: None,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        types: vec![builtin_general()],
        depth: 0,
        bridge,
        driver: Arc::new(TestDriver),
        core: None,
        parent_entries: None,
    });
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: canned_provider(),
            supervisor: Some(sup.clone()),
            system_prompt: None,
            base_prompt: None,
            first_provider: None,
        },
    )
    .unwrap();
    assert!(
        Arc::try_unwrap(agent).is_ok(),
        "the session must be droppable"
    );
}

#[tokio::test]
async fn a_reopened_session_resumes_gate_failed_from_the_record() {
    // A session that ended `gate_failed` resumes that state on re-open: the
    // record is the single source of truth (spec §5.3).
    let dir = tempfile::tempdir().unwrap();
    let store = fresh_store(dir.path(), "parent");
    let bridge: Arc<dyn crate::subagent::SubagentBridge> = Arc::new(TestBridge::default());
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: Arc::new(CannedFactory {
            scripts: vec![],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
            instructions: Arc::new(std::sync::Mutex::new(Vec::new())),
        }),
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: crate::agent::TurnConfig::default(),
        gates: Some(Gates {
            command: "false".into(),
            max_retries: 1,
            ..Gates::default()
        }),
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        caps: SubAgents::default(),
        types: vec![builtin_general()],
        depth: 0,
        bridge,
        driver: Arc::new(TestDriver),
        core: None,
        parent_entries: None,
    });
    // The first life: the check fails through its one repair.
    let write = |id: &str| {
        crate::agent::testkit::sse(
            "",
            &[(
                "write".into(),
                id.into(),
                r#"{"path":"a.txt","content":"x"}"#.into(),
            )],
        )
    };
    let provider = Arc::new(crate::agent::testkit::ScriptedProvider::new(vec![
        write("c1"),
        crate::agent::testkit::sse("done", &[]),
        write("c2"),
        crate::agent::testkit::sse("done", &[]),
    ]));
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider,
            supervisor: Some(sup.clone()),
            system_prompt: None,
            base_prompt: None,
            first_provider: None,
        },
    )
    .unwrap();
    agent.send("work", crate::agent::Lane::FollowUp);
    agent.process().await.unwrap();
    assert!(agent.gate_failed(), "retries exhausted → gate_failed");
    drop(agent);

    // The re-open: a fresh session around the same store resumes the state.
    let reopened = AgentSession::launch(
        SessionStore::for_workspace(dir.path(), "parent"),
        SessionRole::Root {
            core: None,
            workspace: None,
            config: None,
            provider: canned_provider(),
            supervisor: Some(sup),
            system_prompt: None,
            base_prompt: None,
            first_provider: None,
        },
    )
    .unwrap();
    assert!(reopened.gate_failed(), "gate_failed survives the re-open");
    assert!(reopened.gate_failed(), "gate_failed survives the re-open");
}

// The live upsert hook's blob inlining (#265): an entry whose payload was
// extracted to a sidecar blob carries the decoded payload on the wire, not
// the blobless view.

fn upsert_view(dir: &std::path::Path) -> super::launch::UpsertView {
    super::launch::UpsertView::new(dir.join(".tau"))
}

fn blob_entry(dir: &tempfile::TempDir, id: &str, payload: &serde_json::Value) {
    let bytes = payload.to_string().into_bytes();
    let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&bytes));
    let compressed = zstd::encode_all(&bytes[..], 3).unwrap();
    let blobs = dir.path().join(".tau").join("blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(id), compressed).unwrap();
    let entry = crate::session::Entry {
        id: id.into(),
        parent: None,
        timestamp: 1,
        kind: "tool".into(),
        payload: serde_json::Value::Null,
        blob: Some(crate::session::BlobRef {
            id: id.into(),
            size: u64::try_from(bytes.len()).unwrap(),
            hash,
        }),
        first_kept_entry_id: None,
        crc: None,
    };
    std::fs::write(
        dir.path().join("entry.json"),
        serde_json::to_string(&entry).unwrap(),
    )
    .unwrap();
}

#[test]
fn upsert_inlines_a_blob_backed_entry() {
    let dir = tempfile::tempdir().unwrap();
    let payload = serde_json::json!({ "name": "bash", "output": "x".repeat(120_000) });
    blob_entry(&dir, "e1", &payload);

    let entry: crate::session::Entry =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("entry.json")).unwrap())
            .unwrap();
    let view = upsert_view(dir.path()).entry(&entry);

    assert_eq!(view.payload, payload, "the decoded payload is inlined");
    assert_eq!(
        view.blob.as_ref().map(|b| b.id.as_str()),
        Some("e1"),
        "the ref stays"
    );
}

#[test]
fn upsert_leaves_a_small_entry_alone() {
    let dir = tempfile::tempdir().unwrap();
    let payload = serde_json::json!({ "text": "hi" });
    let entry = crate::session::Entry {
        id: "e2".into(),
        parent: None,
        timestamp: 1,
        kind: "user".into(),
        payload: payload.clone(),
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    };
    let view = upsert_view(dir.path()).entry(&entry);
    assert_eq!(view.payload, payload);
    assert!(view.blob.is_none());
}

#[test]
fn upsert_falls_through_when_the_blob_file_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let entry = crate::session::Entry {
        id: "e3".into(),
        parent: None,
        timestamp: 1,
        kind: "tool".into(),
        payload: serde_json::Value::Null,
        blob: Some(crate::session::BlobRef {
            id: "e3".into(),
            size: 10,
            hash: "0000000000000000".into(),
        }),
        first_kept_entry_id: None,
        crc: None,
    };
    // No blob file on disk: the view is the blobless one, no panic.
    let view = upsert_view(dir.path()).entry(&entry);
    assert!(view.payload.is_null());
    assert_eq!(view.blob.as_ref().map(|b| b.id.as_str()), Some("e3"));
}

/// The pi prompt-file seam (ticket #77): `SYSTEM.md` replaces the default
/// base prompt, and `APPEND_SYSTEM.md` composes on top of it — both at
/// `root_session`'s assembly point, before the plan and skills layers.
fn prompt_files_core() -> (Arc<Core>, std::path::PathBuf, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Leak the tempdirs for the test's lifetime: the paths must outlive the
    // launched session's prompt assembly.
    let home = std::mem::ManuallyDrop::new(home);
    let cwd = std::mem::ManuallyDrop::new(cwd);
    let core = CoreBuilder::custom(providers())
        .with_home(home.path().to_path_buf())
        .build()
        .unwrap();
    (core, home.path().to_path_buf(), cwd.path().to_path_buf())
}

#[tokio::test]
async fn root_session_system_md_replaces_the_default_and_append_composes() {
    let (core, home, cwd) = prompt_files_core();
    std::fs::create_dir_all(cwd.join(".pi")).unwrap();
    std::fs::write(cwd.join(".pi").join("SYSTEM.md"), "be a project agent").unwrap();
    std::fs::write(
        cwd.join(".pi").join("APPEND_SYSTEM.md"),
        "project extra rules",
    )
    .unwrap();
    let ws = open_ws(&core, &cwd).await;
    let store = fresh_store(&cwd, &SessionStore::new_session_id());
    let config = core.system_config();
    let first = config
        .providers
        .iter()
        .next()
        .map(|(name, p)| (name.clone(), p.clone()))
        .expect("the test config carries the dev provider");
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core.clone()),
            workspace: Some(ws),
            config: Some(config),
            provider: canned_provider(),
            supervisor: None,
            system_prompt: None,
            base_prompt: None,
            first_provider: Some(first),
        },
    )
    .unwrap();
    let prompt = agent.system_prompt();
    // Replaced: the default frame is gone, the project SYSTEM.md is the base.
    assert!(!prompt.contains("You are Tau, a coding agent."), "{prompt}");
    assert!(prompt.starts_with("be a project agent"), "{prompt}");
    // The append composes on top, in document order after the base.
    assert!(prompt.contains("project extra rules"), "{prompt}");
    assert!(
        prompt.find("be a project agent").unwrap() < prompt.find("project extra rules").unwrap(),
        "the append section lands after the replaced base: {prompt}"
    );
    let _ = home; // keep the tempdir alive for the session's lifetime
}

#[tokio::test]
async fn root_session_headless_frame_is_not_clobbered_by_system_md() {
    // The ACP yolo path passes its headless frame as `base_prompt`: a project
    // SYSTEM.md must not replace it, but APPEND_SYSTEM.md still composes on
    // top (the ticket's precedence rule).
    const FRAME: &str = "You are Tau, an autonomous coding agent running headless.";
    let (core, _home, cwd) = prompt_files_core();
    std::fs::create_dir_all(cwd.join(".pi")).unwrap();
    std::fs::write(cwd.join(".pi").join("SYSTEM.md"), "be a project agent").unwrap();
    std::fs::write(
        cwd.join(".pi").join("APPEND_SYSTEM.md"),
        "project extra rules",
    )
    .unwrap();
    let ws = open_ws(&core, &cwd).await;
    let store = fresh_store(&cwd, &SessionStore::new_session_id());
    let config = core.system_config();
    let first = config
        .providers
        .iter()
        .next()
        .map(|(name, p)| (name.clone(), p.clone()))
        .expect("the test config carries the dev provider");
    let agent = AgentSession::launch(
        store,
        SessionRole::Root {
            core: Some(core),
            workspace: Some(ws),
            config: Some(config),
            provider: canned_provider(),
            supervisor: None,
            system_prompt: None,
            base_prompt: Some(FRAME.to_owned()),
            first_provider: Some(first),
        },
    )
    .unwrap();
    let prompt = agent.system_prompt();
    assert!(
        prompt.starts_with(FRAME),
        "the headless frame survives: {prompt}"
    );
    assert!(
        !prompt.contains("be a project agent"),
        "SYSTEM.md must not replace it: {prompt}"
    );
    assert!(
        prompt.contains("project extra rules"),
        "the append still composes: {prompt}"
    );
}
