//! tau-app: the Tauri shell (ADR-0002, ADR-0006). The transport-free
//! composition — state, dispatch, the event pump, the watchers — lives in
//! `tau_core::harness`; this crate carries the one Tauri surface: the
//! command, the managed-state wrapper, and the binary's menu.

use std::sync::Arc;

use tau_core::harness::Core;

/// Wraps the core's `Arc` so it can be managed as tauri state. rustc 1.98.1
/// computes `TypeId::of::<Arc<Core>>()` differently per instantiating crate,
/// so the type_id of the boxed value never matches the map key tauri's
/// `try_state` computes — `state()` panics at launch ("called before
/// manage()"). A non-generic struct's TypeId is stable, and everything
/// below just unwraps `.0`.
pub struct CoreState(pub Arc<Core>);

/// The one real Tauri seam (spec §8, ADR-0006 transport #1). A submodule,
/// not the crate root: a `pub` root-level command is `#[macro_export]`'d,
/// which would define its hidden macros at the root a second time.
pub mod command {
    use tau_protocol::{Command, CommandOutput, ProtocolError};
    use tauri::State;

    use super::CoreState;

    /// Any protocol command, dispatched through the core.
    #[tauri::command]
    pub async fn tau_command(
        state: State<'_, CoreState>,
        command: Command,
    ) -> Result<CommandOutput, ProtocolError> {
        // Dispatch is synchronous under the hood. Run it on a blocking
        // thread so a command wedged on a lock ties up a throwaway thread,
        // not a runtime worker — the drive tasks, the event pump, and the
        // provider timeouts must keep polling while any command blocks (a
        // deadlocked worker pool wedges the whole app, including every
        // in-flight turn).
        let core = state.0.clone();
        tokio::task::spawn_blocking(move || core.dispatch(command))
            .await
            .map_err(|e| ProtocolError::Other {
                message: e.to_string(),
            })?
    }
}
