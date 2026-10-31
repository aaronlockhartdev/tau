use super::*;
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
    reload_system_layer(&core);
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
    reload_system_layer(&core);
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
    reload_system_layer(&core);
    core.rederive_turn(&live);
    assert_eq!(config_records(cwd, &session).len(), 1);

    // A parse failure is a different story, and its record names the file.
    write_config_fixture(
        &project.path().join(".tau"),
        "[providers.dev\nbase_url = \"x\"",
    );
    reload_system_layer(&core);
    core.rederive_turn(&live);
    let records = config_records(cwd, &session);
    assert_eq!(records.len(), 2, "a new failure is a new record");
    assert!(
        records[1].1.contains("providers.dev"),
        "the parse failure names where it stopped: {}",
        records[1].1
    );
}

/// The GUI's live edit, seen through the watcher: it reads the file, finds it
/// broken, and the merged entry it leaves behind stops claiming to be current —
/// so the next boundary states the failure while the session keeps running on the
/// config it last read. A warm cache alone used to leave that boundary silent
/// (#359).
#[tokio::test]
async fn a_watchers_reload_of_a_broken_layer_is_recorded_at_the_next_boundary() {
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
    let layer = project.path().join(".tau");

    core.rederive_turn(&live);
    assert_eq!(
        retention(&live),
        Some(CacheRetention::Short),
        "the opened layer is live at the boundary"
    );

    write_config_fixture(&layer, &broken_layer("long"));
    core.reload_workspace_config(&ws);
    core.rederive_turn(&live);
    let records = config_records(cwd, &session);
    assert_eq!(
        records.len(),
        1,
        "the watcher met the failure, so the session says it: {records:?}"
    );
    assert!(records[0].1.contains("dev/x"), "{}", records[0].1);
    assert_eq!(
        retention(&live),
        Some(CacheRetention::Short),
        "the unreadable edit is inert: the session runs on the config it last read"
    );

    core.rederive_turn(&live);
    assert_eq!(
        config_records(cwd, &session).len(),
        1,
        "one record per broken layer, not one per turn"
    );

    write_config_fixture(&layer, &project_layer("long"));
    core.reload_workspace_config(&ws);
    core.rederive_turn(&live);
    assert_eq!(
        retention(&live),
        Some(CacheRetention::Long),
        "the repaired layer is live at the next boundary"
    );
    assert_eq!(
        config_records(cwd, &session).len(),
        1,
        "the reload that reads clears the mark: the repair mints nothing"
    );
}

/// The same recurrence one layer over: the project layer's own reload is what
/// clears the mark, so a failure that returns after the repair is a new story and
/// its boundary mints it. The merge is warmed by a read first — a cold boundary
/// reads the file itself and needs no mark to see the repair — and a reload that
/// read but left the mark behind would keep the session under the old pair, so the
/// returning failure would arrive in silence (#359).
#[tokio::test]
async fn a_failure_that_returns_after_a_watchers_repair_is_recorded_again() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    core.workspace_config_checked(&ws);
    let live = manual_session(
        &core,
        &ws,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "model");
    let session = live.meta.lock().id.clone();
    let cwd = Path::new(&ws.cwd);
    let layer = project.path().join(".tau");
    for round in 0..2 {
        write_config_fixture(&layer, &broken_layer("long"));
        core.reload_workspace_config(&ws);
        core.rederive_turn(&live);
        write_config_fixture(&layer, &project_layer("long"));
        core.reload_workspace_config(&ws);
        core.rederive_turn(&live);
        assert_eq!(
            config_records(cwd, &session).len(),
            round + 1,
            "round {round}: the watcher met the broken layer, the watcher met the repair"
        );
    }
}
