use super::*;
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
/// again, and the latch is what keeps it to one record (#359).
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
    reload_system_layer(&core);
    core.rederive_turn(&live);
    let records = config_records(Path::new(&ws.cwd), &session);
    assert_eq!(
        records.len(),
        1,
        "the open states the failure; the first boundary under it adds nothing: {records:?}"
    );
}

/// The record, not the latch, is what survives a close: a session closed and
/// reopened over one unfixed failure states it once — the projection says the file
/// already names it, and primes the fresh latch so the first boundary under the
/// reopen stays quiet too. The user's path: close the tab, reopen, send (#359).
#[tokio::test]
async fn a_broken_layer_is_stated_once_across_a_reopen() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &broken_layer("short")).await;
    let session = new_session(&core, &ws);
    reload_system_layer(&core);
    for _ in 0..2 {
        core.sessions.lock().remove(&session);
        core.session_reopen(&ws, &session).unwrap();
        let live = core
            .sessions
            .lock()
            .get(&session)
            .cloned()
            .expect("the session the reopen registered");
        core.rederive_turn(&live);
    }
    let records = config_records(Path::new(&ws.cwd), &session);
    assert_eq!(
        records.len(),
        1,
        "a file left broken is one story, not one per open: {records:?}"
    );
}

/// A read that succeeds clears the latch: a failure that returns after a repair
/// is a new story, and its boundary mints it (#359).
#[tokio::test]
async fn a_failure_that_returns_after_a_repair_is_recorded_again() {
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
    for round in 0..2 {
        write_config_fixture(&layer, &broken_layer("long"));
        reload_system_layer(&core);
        core.rederive_turn(&live);
        write_config_fixture(&layer, &project_layer("long"));
        reload_system_layer(&core);
        core.rederive_turn(&live);
        assert_eq!(
            config_records(cwd, &session).len(),
            round + 1,
            "round {round}: the broken layer is stated, the repair mints nothing"
        );
    }
}

/// A closed workspace keeps no merge and no mark of one: a layer broken while the
/// workspace is open, then repaired while it is closed, is live at the reopen, and
/// the failure is not restated from a cache that stopped reading the file the
/// moment the workspace closed (#359).
#[tokio::test]
async fn a_layer_repaired_while_the_workspace_is_closed_is_live_at_the_reopen() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    let session = new_session(&core, &ws);
    core.reload_workspace_config(&ws);
    write_config_fixture(&project.path().join(".tau"), &broken_layer("short"));
    core.reload_workspace_config(&ws);
    core.close_workspace(&ws.id).unwrap();
    assert!(
        core.configs.lock().get(&ws.id).is_none(),
        "a closed workspace keeps no merge"
    );
    assert!(
        core.config_stale.lock().get(&ws.id).is_none(),
        "and no mark of one"
    );
    write_config_fixture(&project.path().join(".tau"), &project_layer("long"));
    let ws = open_ws(&core, project.path()).await;
    core.session_reopen(&ws, &session).unwrap();
    let live = core
        .sessions
        .lock()
        .get(&session)
        .cloned()
        .expect("the session the reopen registered");
    carry_model(&live, "model");
    core.rederive_turn(&live);
    assert_eq!(
        retention(&live),
        Some(CacheRetention::Long),
        "the repaired layer is live, not the merge the closed workspace left"
    );
    assert_eq!(
        config_records(Path::new(&ws.cwd), &session).len(),
        0,
        "the failure the file outgrew is not restated from a cache"
    );
}

/// The mark describes the file, not history: the reload that reads replaces both
/// the merge and its mark, so a workspace never runs under a failure it has
/// outlived — the system layer's reload drops the marks of the merges it
/// invalidates, the same breath it drops the merges (#359).
#[tokio::test]
async fn a_reload_that_reads_leaves_no_mark_on_the_merge_it_replaces() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    core.reload_workspace_config(&ws);
    write_config_fixture(&project.path().join(".tau"), &broken_layer("short"));
    core.reload_workspace_config(&ws);
    assert!(
        core.config_stale.lock().contains_key(&ws.id),
        "the failed reload marks the merge it left in place"
    );
    write_config_fixture(&project.path().join(".tau"), &project_layer("long"));
    reload_system_layer(&core);
    assert!(
        core.config_stale.lock().get(&ws.id).is_none(),
        "the mark of the dropped merge goes with it"
    );
    assert_eq!(
        core.configs.lock().get(&ws.id).map(|c| c.cache.retention),
        Some(CacheRetention::Long),
        "the merge that refills reads the repaired layer, not the marked one"
    );
}

/// The pair, not the path: a reopen that meets a different failure in the same
/// file is a second story, and the open states it. A compare that stopped at the
/// path would call the new failure told, and the latch it primes would keep the
/// boundary quiet under it too (#359).
#[tokio::test]
async fn a_reopen_that_meets_a_new_failure_records_that_failure_too() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &broken_layer("short")).await;
    let session = new_session(&core, &ws);
    reload_system_layer(&core);
    core.sessions.lock().remove(&session);
    core.session_reopen(&ws, &session).unwrap();
    assert_eq!(
        config_records(Path::new(&ws.cwd), &session).len(),
        1,
        "the same failure across a reopen is one story"
    );
    // The file is broken in a new way: a parse failure, at the same path.
    write_config_fixture(
        &project.path().join(".tau"),
        "[providers.dev\nbase_url = \"x\"",
    );
    reload_system_layer(&core);
    core.sessions.lock().remove(&session);
    core.session_reopen(&ws, &session).unwrap();
    let records = config_records(Path::new(&ws.cwd), &session);
    assert_eq!(
        records.len(),
        2,
        "a new failure under the same path is a new record: {records:?}"
    );
    assert!(
        records[1].1.contains("providers.dev"),
        "the new record repeats the new failure: {}",
        records[1].1
    );
}

/// A mark describes one merged entry, so a read that produces the entry leaves no
/// mark behind: a failed reload marks the workspace, the file is repaired before
/// the reload's debounce elapses, and the next read — cold, because a failed read
/// writes no merge — must answer with the current config, not a stated failure
/// (#359).
#[tokio::test]
async fn a_cold_read_of_a_repaired_layer_leaves_no_mark_behind_it() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &project_layer("short")).await;
    write_config_fixture(&project.path().join(".tau"), &broken_layer("short"));
    core.reload_workspace_config(&ws);
    assert!(
        core.config_stale.lock().contains_key(&ws.id),
        "the failed reload marks the workspace"
    );
    assert!(
        core.configs.lock().get(&ws.id).is_none(),
        "a failed read writes nothing: the workspace has no merge to ride"
    );
    // The file is repaired before the watcher's debounce elapses, so the next read
    // is cold: there is no merge, only the mark the failed reload left.
    write_config_fixture(&project.path().join(".tau"), &project_layer("long"));
    let (config, rejection) = core.workspace_config_checked(&ws);
    assert_eq!(config.cache.retention, CacheRetention::Long);
    assert!(
        rejection.is_none(),
        "a config read off disk answers with no failure: {rejection:?}"
    );
    assert!(
        core.config_stale.lock().get(&ws.id).is_none(),
        "the mark of the merge this read replaces goes with it"
    );
    let live = manual_session(
        &core,
        &ws,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "model");
    core.rederive_turn(&live);
    assert!(
        config_records(Path::new(&ws.cwd), &live.meta.lock().id.clone()).is_empty(),
        "the boundary is told nothing about a layer that reads"
    );
}
