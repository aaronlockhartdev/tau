//! The config's own failures, as records (#359): a layer the code cannot read
//! is named to the session that keeps running on its last good config. The
//! watcher's `ConfigRejected` event is the live GUI's banner; these are the
//! durable facts a session carries, minted at open and at the turn boundary.

use super::*;

use crate::config::CacheRetention;
use crate::harness::testkit::*;

/// The system layer: the provider whose model carries the gate's facts, and the
/// default model. No `[cache]` — the project layer's is what the boundary
/// re-reads, and its absence here makes the value the witness.
const SYSTEM: &str = r#"
[providers.dev]
base_url = "http://127.0.0.1:9/v1"

[providers.dev.models.model]
context_window = 32768
max_tokens = 8192

[generation]
default_model = "dev/model"
"#;

/// The project layer, valid. It re-declares `dev` — a `[providers]` table
/// replaces the whole map, so the model's facts travel with it — and sets
/// `[cache].retention` to `retention`.
fn project_layer(retention: &str) -> String {
    format!(
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n[providers.dev.models.model]\ncontext_window = 32768\nmax_tokens = 8192\n\n[cache]\nretention = \"{retention}\"\n"
    )
}

/// The same layer with a provider name the spelling rules refuse (#354): a name
/// carrying '/' makes an id's prefix ambiguous, so every read of this layer
/// fails. The `[cache]` value beside it is the edit that must stay inert.
fn broken_layer(retention: &str) -> String {
    format!(
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n[providers.dev.models.model]\ncontext_window = 32768\nmax_tokens = 8192\n\n[providers.\"dev/x\"]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n[cache]\nretention = \"{retention}\"\n"
    )
}

/// The `config_rejected` records a session carries: each one's path and reason,
/// in the order the store holds them. The whole log by design (#302): the count
/// of these records is the assertion.
fn config_records(cwd: &Path, session: &str) -> Vec<(String, String)> {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| {
            e.kind == EntryKind::Diagnostic
                && e.payload.get("code").and_then(Value::as_str) == Some("config_rejected")
        })
        .map(|e| {
            (
                e.payload
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                e.payload
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

/// The retention the session's live turn runs with — the `[cache]` value the
/// boundary re-derived, and the witness of which config the session is on.
fn retention(live: &Arc<LiveSession>) -> Option<CacheRetention> {
    live.agent
        .current_turn()
        .prompt_cache
        .map(|(_, retention)| retention)
}

/// A core over a system layer and a project layer, with the workspace opened on
/// the project.
async fn core_over(
    system: &tempfile::TempDir,
    project: &tempfile::TempDir,
    project_raw: &str,
) -> (Arc<Core>, Workspace) {
    write_config_fixture(system.path(), SYSTEM);
    write_config_fixture(&project.path().join(".tau"), project_raw);
    let core = CoreBuilder::default_system()
        .unwrap()
        .with_system_dir(system.path().to_path_buf())
        .with_home(system.path().to_path_buf())
        .build()
        .unwrap();
    let workspace = open_ws(&core, project.path()).await;
    (core, workspace)
}

/// The model a session carries, left the way `session_set_model` leaves it: the
/// boundary reads the agent's own model.
fn carry_model(live: &Arc<LiveSession>, model: &str) {
    live.meta.lock().model = Some(model.to_owned());
    live.agent.set_model(model.to_owned());
}
/// The merged config is cached from the last read, so a fixture that rewrote a
/// layer drops the cache and the next boundary reads the file. In the running
/// app the watcher does this (`reload_workspace_config`).
fn invalidate_config_cache(core: &Arc<Core>) {
    core.reload_system_config();
}

/// A session opened through dispatch — the path a user opens one.
fn new_session(core: &Arc<Core>, workspace: &Workspace) -> String {
    match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session.id,
        other => panic!("expected a session: {other:?}"),
    }
}

/// #359 at the turn boundary: a project layer the code cannot read records the
/// rejection once, names the file and what the loader said, and stays inert —
/// the session runs on the layer that reads (spec §12's reload failure path).
#[tokio::test]
async fn a_broken_layer_at_the_boundary_is_recorded_once_and_the_edit_is_inert() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    let live = manual_session(
        &core,
        &ws,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "model");
    let session = live.meta.lock().id.clone();
    let cwd = Path::new(&ws.cwd);

    core.rederive_turn(&live);
    assert_eq!(
        retention(&live),
        Some(CacheRetention::Short),
        "the layer the session opened with is live at the boundary"
    );
    assert!(
        config_records(cwd, &session).is_empty(),
        "a config that reads records nothing"
    );

    write_config_fixture(&project.path().join(".tau"), &broken_layer("long"));
    invalidate_config_cache(&core);
    core.rederive_turn(&live);
    let records = config_records(cwd, &session);
    assert_eq!(records.len(), 1, "the unreadable layer is recorded");
    assert!(
        records[0].0.ends_with(".tau/config.toml"),
        "the record names the file the user edits: {}",
        records[0].0
    );
    assert!(
        records[0].1.contains("dev/x") && records[0].1.contains("must not contain"),
        "the record repeats what the loader said: {}",
        records[0].1
    );
    assert_eq!(
        retention(&live),
        None,
        "the unreadable layer's `[cache]` edit is inert: the session runs on the layer that reads, whose `[cache]` is unset"
    );

    core.rederive_turn(&live);
    assert_eq!(
        config_records(cwd, &session).len(),
        1,
        "one record per broken layer, not one per turn"
    );

    write_config_fixture(&project.path().join(".tau"), &project_layer("long"));
    invalidate_config_cache(&core);
    core.rederive_turn(&live);
    assert_eq!(
        config_records(cwd, &session).len(),
        1,
        "the repair mints no new record"
    );
    assert_eq!(
        retention(&live),
        Some(CacheRetention::Long),
        "the repaired layer's value reaches the next turn"
    );
}

/// The latch is per file and reason: a second, different failure is a second
/// story.
#[tokio::test]
async fn a_second_broken_layer_with_a_new_reason_is_recorded_too() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    let live = manual_session(
        &core,
        &ws,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "model");
    let session = live.meta.lock().id.clone();
    let cwd = Path::new(&ws.cwd);

    write_config_fixture(&project.path().join(".tau"), &broken_layer("long"));
    invalidate_config_cache(&core);
    core.rederive_turn(&live);
    assert_eq!(config_records(cwd, &session).len(), 1);

    // A parse failure is a different story, and its record names the file.
    write_config_fixture(
        &project.path().join(".tau"),
        "[providers.dev\nbase_url = \"x\"",
    );
    invalidate_config_cache(&core);
    core.rederive_turn(&live);
    let records = config_records(cwd, &session);
    assert_eq!(records.len(), 2, "a new failure is a new record");
    assert!(
        records[1].1.contains("providers.dev"),
        "the parse failure names where it stopped: {}",
        records[1].1
    );
}

/// #359 at open: a config broken before the session opens is stated to it,
/// because the GUI's banner rides the watcher's event and a cold open never
/// fires the watcher.
#[tokio::test]
async fn a_broken_layer_at_open_is_recorded_as_the_session_opens() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &broken_layer("short")).await;
    let session = new_session(&core, &ws);
    let records = config_records(Path::new(&ws.cwd), &session);
    assert_eq!(
        records.len(),
        1,
        "a session opened on an unreadable config says so"
    );
    assert!(records[0].1.contains("dev/x"), "{}", records[0].1);
}

/// The negative beside it: a config that reads mints nothing at open.
#[tokio::test]
async fn a_readable_layer_at_open_records_nothing() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    let session = new_session(&core, &ws);
    assert!(
        config_records(Path::new(&ws.cwd), &session).is_empty(),
        "no record without a failure"
    );
}
/// The open's record and the boundary's are one story: a session opened on a
/// broken layer states it at open, and its first boundary under the same failure
/// adds nothing — a failed read caches nothing, so the boundary meets the failure
/// again and the latch is what keeps it to one record (#359).
#[tokio::test]
async fn a_broken_layer_at_open_is_recorded_once_across_its_first_boundary() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &broken_layer("short")).await;
    let session = new_session(&core, &ws);
    let live = core
        .sessions
        .lock()
        .get(&session)
        .cloned()
        .expect("the session the open registered");
    carry_model(&live, "model");
    invalidate_config_cache(&core);
    core.rederive_turn(&live);
    let records = config_records(Path::new(&ws.cwd), &session);
    assert_eq!(
        records.len(),
        1,
        "the open states the failure; the first boundary under it adds nothing: {records:?}"
    );
}
