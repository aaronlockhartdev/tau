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

#[tokio::test]
async fn a_broken_system_layer_is_stated_by_the_boot() {
    let system = tempfile::tempdir().unwrap();
    write_config_fixture(system.path(), "[providers.dev\n");
    let core = CoreBuilder::custom(providers())
        .with_system_dir(system.path().to_path_buf())
        .build()
        .unwrap();

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
            .is_some_and(|c| !c.providers.contains_key("dev")),
        "the boot keeps its defaults: the file's providers did not arrive"
    );
}

#[tokio::test]
async fn a_readable_system_layer_states_nothing_at_boot() {
    let system = tempfile::tempdir().unwrap();
    write_config_fixture(system.path(), &project_layer("short"));
    let core = CoreBuilder::custom(providers())
        .with_system_dir(system.path().to_path_buf())
        .build()
        .unwrap();

    assert!(
        rejection(&boot_events(&core)).is_none(),
        "a layer the boot read has nothing to say"
    );
    assert!(
        core.configs
            .lock()
            .get("")
            .is_some_and(|c| c.providers.contains_key("dev")),
        "the boot rode the file it read"
    );
}
