//! The config's own failures, as records (#359): a layer the code cannot read
//! is named to the session that keeps running on its last good config. The
//! watcher's `ConfigRejected` event is the live GUI's banner; these are the
//! durable facts a session carries, minted at open and at the turn boundary.

use super::*;

use crate::config::CacheRetention;
use crate::harness::testkit::*;

mod boot;
mod boundary;
mod opening;
mod two_writers;

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
