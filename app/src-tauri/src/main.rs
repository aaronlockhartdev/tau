//! Transport #1 (ADR-0006): Tauri carries the protocol's tagged-union
//! messages. One command carries any `Command`; the event pump emits
//! coalesced batches on the `tau://event` channel.

use std::sync::Arc;

use tau_app::{
    CoreState,
    core::{Core, CoreBuilder, pump},
};
use tau_protocol::{Command, CommandOutput, ProtocolError};
use tauri::{
    Emitter, Manager, State,
    menu::{AboutMetadata, MenuBuilder, MenuItem, PredefinedMenuItem, Submenu},
};

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

/// The standard menu (the shape of `Menu::default`) with `Open Folder…`
/// added to File — the product's way to open a project (ticket #29 B1).
/// The menu only emits: the picker itself runs the dialog plugin's
/// frontend path, so its scope handling stays in one place.
fn build_menu(app: &tauri::App) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let pkg_info = app.package_info();
    let config = app.config();
    let about_meta = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let open = MenuItem::with_id(
        app,
        "open-folder",
        "Open Folder…",
        true,
        Some("CmdOrCtrl+O"),
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &open,
            #[cfg(not(target_os = "linux"))]
            &PredefinedMenuItem::separator(app)?,
            #[cfg(not(target_os = "linux"))]
            &PredefinedMenuItem::close_window(app, None)?,
            #[cfg(not(target_os = "linux"))]
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window = Submenu::with_id_and_items(
        app,
        tauri::menu::WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let help = Submenu::with_id_and_items(
        app,
        tauri::menu::HELP_SUBMENU_ID,
        "Help",
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::about(app, None, Some(about_meta.clone()))?,
        ],
    )?;
    let menu = MenuBuilder::new(app)
        .items(&[
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_meta))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &file,
            &edit,
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window,
            &help,
        ])
        .build()?;
    Ok(menu)
}

fn main() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(CoreState(CoreBuilder::default_system().build()))
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "open-folder" {
                let _ = app.emit("open_folder_requested", ());
            }
        })
        .setup(|app| {
            app.handle().set_menu(build_menu(app)?)?;
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
