//! The one real Tauri surface (G4): the command's managed-state wiring
//! (the `CoreState` TypeId, spec §8), the `spawn_blocking` hop, and the
//! `JoinError → ProtocolError` mapping. `tauri::test` is marked unstable,
//! so the suite is one test.

use std::collections::BTreeMap;

use tau_app::{CoreState, command::tau_command};
use tau_core::harness::CoreBuilder;
use tau_protocol::{Command, CommandOutput};
use tauri::Manager;
use tauri::test::mock_app;

#[tokio::test]
async fn the_command_reaches_the_core_through_managed_state() {
    let app = mock_app();
    app.manage(CoreState(CoreBuilder::custom(BTreeMap::new()).build()));
    let out = tau_command(app.state(), Command::WorkspaceList)
        .await
        .expect("the command must not fail");
    let CommandOutput::Workspaces { workspaces } = out else {
        panic!("expected the workspace list output");
    };
    assert!(workspaces.is_empty());
}
