//! The watcher's half of the live config reload (spec §12, #337): a
//! `config.toml` edit reaches the running core's config through the watcher —
//! the swap lands only on a clean read, a rejection leaves the last good config
//! live, and `ProviderChanged` marks the model menu's rows stale. The other
//! half, what a reload does to a live session at its turn boundary, is
//! `tests_config_boundary`.

use super::*;

use crate::harness::testkit::*;

fn config_with(provider: &str, model: &str) -> String {
    format!(
        "[providers.{provider}]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n[providers.{provider}.models.\"{model}\"]\ncontext_window = 32768\n"
    )
}

fn config_path(layer: &Path) -> PathBuf {
    layer.join(crate::config::CONFIG_FILE)
}

/// The rows the cache entry for `workspace` serves: the cache is the core's
/// live config, so this is the "did the rejection overwrite it?" read.
fn cached_rows(core: &Core, workspace: &Workspace) -> Vec<ProviderInfo> {
    let cached = core
        .configs
        .lock()
        .get(&workspace.id)
        .cloned()
        .unwrap_or_default();
    dispatch::provider_rows(&cached)
}

/// The project layer's edit, end to end: a provider written into
/// `{workspace}/.tau/config.toml` shows up in `provider_list` and the edge
/// reaches the events channel.
#[tokio::test]
async fn a_project_config_edit_reaches_the_provider_list() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    write_config_fixture(&project, &config_with("first", "m1"));
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    assert_eq!(
        provider_list(&core, &workspace.id)
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["dev", "first"],
        "the file's provider is served before the edit"
    );
    let collected = collect_events(&core);
    std::fs::write(config_path(&project), config_with("local", "llama3")).unwrap();
    wait_for_provider_changed(&collected, &workspace.id).await;
    let rows = provider_list(&core, &workspace.id);
    assert!(
        rows.iter().any(|p| p.name == "local"),
        "the added provider is served: {rows:?}"
    );
    assert!(
        rows.iter().any(|p| p.name == "dev"),
        "the explicit provider is still served: {rows:?}"
    );
    assert_eq!(
        rows.iter()
            .find(|p| p.name == "local")
            .map(|p| p.models.clone()),
        Some(vec!["llama3".to_owned()]),
        "the row carries the file's model"
    );
}

/// A syntactically invalid edit is never applied: the last good config stays
/// live, the cache entry is not overwritten, and the failure reaches the user
/// naming the layer's file.
#[tokio::test]
async fn an_invalid_project_config_keeps_the_last_good_config_live() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    let file = config_path(&project);
    write_config_fixture(&project, &config_with("local", "llama3"));
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let good = core.workspace_config(&workspace);
    assert!(good.providers.contains_key("local"));
    let collected = collect_events(&core);
    std::fs::write(&file, "[providers.local]\nbase_url = ").unwrap();
    let (path, reason) = wait_for_config_rejected(&collected, &workspace.id).await;
    assert_eq!(
        path.as_deref().map(Path::new),
        Some(file.as_path()),
        "the event names the file: {path:?}"
    );
    assert!(
        reason.contains("parsing"),
        "the reason is the load error: {reason}"
    );
    assert_eq!(
        cached_rows(&core, &workspace),
        dispatch::provider_rows(&good),
        "the cache entry is the last good config: a rejection writes nothing"
    );
    assert_eq!(
        provider_list(&core, &workspace.id)
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["dev", "local"],
        "the model menu's rows are the last good config's"
    );
    assert_eq!(
        provider_changed_count(&collected, &workspace.id),
        0,
        "a rejection is not a provider change"
    );
}

/// The rejection is not sticky-forever: a later batch re-reads the file, so a
/// fixed `config.toml` takes effect (the cache-invalidation half).
#[tokio::test]
async fn a_fixed_config_takes_effect_after_a_rejection() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    let file = config_path(&project);
    write_config_fixture(&project, &config_with("first", "m1"));
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    assert!(
        provider_list(&core, &workspace.id)
            .iter()
            .any(|p| p.name == "first"),
        "the good layer is live before the rejection"
    );
    let collected = collect_events(&core);
    std::fs::write(&file, "not = = toml\n").unwrap();
    let (_, reason) = wait_for_config_rejected(&collected, &workspace.id).await;
    assert!(
        provider_list(&core, &workspace.id)
            .iter()
            .any(|p| p.name == "first"),
        "the last good config stays live through the rejection"
    );
    std::fs::write(&file, config_with("fixed", "m2")).unwrap();
    wait_for_provider_changed(&collected, &workspace.id).await;
    let rows = provider_list(&core, &workspace.id);
    assert!(
        rows.iter().any(|p| p.name == "fixed"),
        "the fixed file's provider is served: {rows:?}"
    );
    assert!(
        !rows.iter().any(|p| p.name == "first"),
        "the file replaced its provider (the rejection said: {reason})"
    );
}

/// The system layer is shared by every workspace, so one edit reaches them all
/// with its own edge per workspace (the rows the menu serves are per
/// workspace).
#[tokio::test]
async fn a_system_config_edit_reaches_every_open_workspace() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    let sys = tempfile::tempdir().unwrap();
    write_config_fixture(sys.path(), &config_with("sysfirst", "m1"));
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().to_path_buf())
        .build()
        .unwrap();
    let w1 = open_ws(&core, tmp1.path()).await;
    let w2 = open_ws(&core, tmp2.path()).await;
    let collected = collect_events(&core);
    std::fs::write(config_path(sys.path()), config_with("sysnew", "m9")).unwrap();
    wait_for_provider_changed(&collected, &w1.id).await;
    for ws in [&w1, &w2] {
        let rows = provider_list(&core, &ws.id);
        assert!(
            rows.iter().any(|p| p.name == "sysnew"),
            "{} serves the system layer's provider: {rows:?}",
            ws.id
        );
    }
    assert!(
        provider_changed_count(&collected, &w2.id) >= 1,
        "the second workspace got its own edge"
    );
}

/// A rejected system-layer edit keeps the boot-time config live (the cache
/// holds the last good merge, not a default), and the failure is routed with an
/// empty workspace key — the system layer belongs to no workspace.
#[tokio::test]
async fn an_invalid_system_config_keeps_the_boot_config_live() {
    let tmp = tempfile::tempdir().unwrap();
    let sys = tempfile::tempdir().unwrap();
    write_config_fixture(sys.path(), &config_with("sysgood", "m1"));
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().to_path_buf())
        .build()
        .unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let good = provider_list(&core, &workspace.id);
    assert!(good.iter().any(|p| p.name == "sysgood"));
    let collected = collect_events(&core);
    std::fs::write(
        config_path(sys.path()),
        "[providers.sysbad]\nbase_url = \"http://127.0.0.1:9/v1\"\nbogus = 1\n",
    )
    .unwrap();
    let (path, _) = wait_for_config_rejected(&collected, "").await;
    assert_eq!(
        path.as_deref().map(Path::new),
        Some(config_path(sys.path()).as_path()),
        "the event names the system layer's file: {path:?}"
    );
    assert_eq!(
        provider_list(&core, &workspace.id),
        good,
        "the last good config is what the core serves"
    );
    assert_eq!(
        provider_changed_count(&collected, &workspace.id),
        0,
        "a rejection is not a provider change"
    );
}

/// The edge, negatively: a reload whose rows are the ones already live emits no
/// `ProviderChanged` — a save that changes nothing must not churn the menu.
#[tokio::test]
async fn a_reload_that_changes_nothing_emits_no_provider_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    let raw = config_with("local", "llama3");
    write_config_fixture(&project, &raw);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    provider_list(&core, &workspace.id);
    let collected = collect_events(&core);
    std::fs::write(config_path(&project), raw).unwrap();
    // A quiet window past the debounce (500 ms): the reload ran — the file
    // changed — and it found the rows it already served.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        provider_changed_count(&collected, &workspace.id),
        0,
        "the identical rows emitted nothing"
    );
    assert!(
        provider_list(&core, &workspace.id)
            .iter()
            .any(|p| p.name == "local"),
        "the config is still the file's"
    );
}
/// The rejection's other half: a validation failure is the merged config's
/// property, not one file's, so the event names no layer (`path: null`) and the
/// last good config stays live. The parse case, which names its file, is
/// `an_invalid_system_config_keeps_the_boot_config_live`.
#[tokio::test]
async fn a_validation_failure_rejects_without_naming_a_layer() {
    let tmp = tempfile::tempdir().unwrap();
    let sys = tempfile::tempdir().unwrap();
    write_config_fixture(sys.path(), &config_with("sysgood", "m1"));
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().to_path_buf())
        .build()
        .unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let good = provider_list(&core, &workspace.id);
    let collected = collect_events(&core);
    // Parses cleanly; the threshold is over the u32 token limit, a fact only the
    // merged config can state.
    std::fs::write(
        config_path(sys.path()),
        format!(
            "{}\n[om]\nobserve_threshold = 5000000000\n",
            config_with("sysbad", "m2")
        ),
    )
    .unwrap();
    let (path, reason) = wait_for_config_rejected(&collected, "").await;
    assert_eq!(
        path, None,
        "a merged-config failure names no file the user can edit: {reason}"
    );
    assert!(
        reason.contains("observe_threshold"),
        "the reason is the bound that failed: {reason}"
    );
    assert_eq!(
        provider_list(&core, &workspace.id),
        good,
        "the rejected edit applies nothing"
    );
}

/// The watcher's batch filter: the watched layer dirs carry the sessions,
/// skills, and plans too, so only a batch naming `config.toml` — or the
/// rescan's empty batch — means config.
#[test]
fn a_batch_means_config_only_when_it_names_the_config_file() {
    use crate::config::CONFIG_FILE;
    let empty: Batch = vec![];
    assert!(batch_carries_config_file(&empty));
    let named: Batch = vec![PathBuf::from(format!("/w/.tau/{CONFIG_FILE}"))];
    assert!(batch_carries_config_file(&named));
    let other: Batch = vec![
        PathBuf::from("/w/.tau/skills"),
        PathBuf::from("/w/.tau/plans/session.md"),
        PathBuf::from("/w/.tau/sessions.log"),
    ];
    assert!(!batch_carries_config_file(&other));
}
