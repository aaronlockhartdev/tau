//! Transport #1 (ADR-0006): Tauri carries the protocol's tagged-union
//! messages. One command carries any `Command`; the event pump emits
//! coalesced batches on the `tau://event` channel.

use std::sync::Arc;

use tau_app::core::{Core, CoreBuilder, pump};
use tau_protocol::{Command, CommandOutput, ProtocolError};
use tauri::{Emitter, Manager, State};

#[tauri::command]
async fn tau_command(
    state: State<'_, Arc<Core>>,
    command: Command,
) -> Result<CommandOutput, ProtocolError> {
    state.dispatch(command).await
}

fn main() {
    tauri::Builder::default()
        .manage(Arc::new(CoreBuilder::default_system().build()))
        .setup(|app| {
            let core: Arc<Core> = (*app.state::<Arc<Core>>()).clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                pump(core, |batch| {
                    let _ = handle.emit("tau://event", batch);
                })
                .await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![tau_command])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
