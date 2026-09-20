//! Transport #1 (ADR-0006): Tauri carries the protocol's tagged-union
//! messages. One command carries any `Command`; the event pump emits
//! coalesced batches on the `tau://event` channel.

use std::sync::Arc;

use tau_app::{
    CoreState,
    core::{Core, CoreBuilder, pump},
};
use tau_protocol::{Command, CommandOutput, ProtocolError};
use tauri::{Emitter, Manager, State};

#[cfg(debug_assertions)]
mod dev_bridge;

#[tauri::command]
async fn tau_command(
    state: State<'_, CoreState>,
    command: Command,
) -> Result<CommandOutput, ProtocolError> {
    // Dispatch is synchronous under the hood. Run it on a blocking thread
    // so a command wedged on a lock ties up a throwaway thread, not a
    // runtime worker — the drive tasks, the event pump, and the provider
    // timeouts must keep polling while any command blocks (a deadlocked
    // worker pool wedges the whole app, including every in-flight turn).
    let core = state.0.clone();
    tokio::task::spawn_blocking(move || core.dispatch(command))
        .await
        .map_err(|e| ProtocolError::Other {
            message: e.to_string(),
        })?
}

fn main() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(CoreState(CoreBuilder::default_system().build()))
        .setup(|app| {
            #[cfg(debug_assertions)]
            if let Err(e) = dev_bridge::start_bridge(app.handle()).map(|_| ()) {
                eprintln!("Warning: Failed to start dev bridge: {e}");
            }
            let core: Arc<Core> = app.state::<CoreState>().0.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                pump(core, |batch| {
                    let _ = handle.emit("tau://event", batch);
                })
                .await;
            });
            Ok(())
        });

    // The agent-tools dev bridge adds one command; tauri takes a single
    // invoke_handler, so the two build branches merge them.
    #[cfg(debug_assertions)]
    {
        builder = builder.invoke_handler(tauri::generate_handler![
            tau_command,
            dev_bridge::__dev_bridge_result
        ]);
    }
    #[cfg(not(debug_assertions))]
    {
        builder = builder.invoke_handler(tauri::generate_handler![tau_command]);
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
