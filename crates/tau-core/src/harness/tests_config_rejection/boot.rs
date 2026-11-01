//! The system layer as the app finds it at boot: the one read allowed to ride a
//! bad config, and the one break with no session open to record it (#363).
//! A parse failure names its file, so this is the break whose whole story fits
//! on one line: the boot's own event.

use super::*;
use crate::harness::state::CoreBuilder;
use tau_protocol::{Event, SystemEventKind};

/// The events a boot emitted, drained from the app's own pipe. The channel
/// buffers, so a receiver taken after the build still sees what the boot said.
fn boot_events(core: &Core) -> Vec<Event> {
    let mut events = core.events();
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

fn rejection(events: &[Event]) -> Option<(Option<String>, String)> {
    events.iter().find_map(|e| match e {
        Event::System {
            kind: SystemEventKind::ConfigRejected { path, reason },
            ..
        } => Some((path.clone(), reason.clone())),
        _ => None,
    })
}

/// A boot over a system layer carrying `body`, sealed from the developer's
/// machine: the boot watches the home's skill roots, so a test that leaves the
/// home seam unset watches a real `$HOME`.
fn boot(body: &str) -> Arc<Core> {
    let home = tempfile::tempdir().expect("temp home");
    let system = tempfile::tempdir().expect("temp system dir");
    write_config_fixture(system.path(), body);
    let core = CoreBuilder::custom(providers())
        .with_home(home.path().to_path_buf())
        .with_system_dir(system.path().to_path_buf())
        .build()
        .expect("the boot completes: the shape is the app's, not the config's");
    // The temp dirs outlive the Core they configured: the watchers hold paths
    // in them, and a watcher on a deleted dir is the parked-debouncer shape
    // (#367), not a fact about the boot.
    std::mem::forget(home);
    std::mem::forget(system);
    core
}

#[tokio::test]
async fn a_broken_system_layer_is_stated_by_the_boot() {
    // A provider named only in the file: the builder injects its own map, so
    // the file's name is the only one that can show the boot rode the file.
    let core = boot("[providers.fileonly\nbase_url = \"http://127.0.0.1:9/v1\"\n");
    let stated = rejection(&boot_events(&core));
    assert!(
        stated.as_ref().is_some_and(|(path, reason)| {
            path.as_deref().is_some_and(|p| p.ends_with("config.toml")) && reason.contains("parse")
        }),
        "the boot says what it refused to read: {stated:?}"
    );
    assert!(
        core.configs
            .lock()
            .get("")
            .is_some_and(|c| !c.providers.contains_key("fileonly")),
        "the boot keeps its defaults: the file's provider did not arrive"
    );
}

#[tokio::test]
async fn a_readable_system_layer_states_nothing_at_boot() {
    let core = boot(
        "[providers.fileonly]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n[providers.fileonly.models.model]\ncontext_window = 32768\nmax_tokens = 8192\n",
    );
    assert!(
        rejection(&boot_events(&core)).is_none(),
        "a readable layer gives the boot nothing to say"
    );
    assert!(
        core.configs
            .lock()
            .get("")
            .is_some_and(|c| c.providers.contains_key("fileonly")),
        "the readable layer's provider arrived: the boot rode the file"
    );
}
