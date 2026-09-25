//! B7 (persistent workspace close): the registry's per-workspace `open`
//! flag — open writes it true, close persists false, boot restores open
//! workspaces only, and reopening flips the flag back.

use super::*;

use crate::harness::testkit::*;

fn index(sys: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(sys.join("workspaces.json")).unwrap()).unwrap()
}

/// The index entry for `cwd` (the index is a flat array of entries).
fn entry_for(sys: &Path, cwd: &Path) -> serde_json::Value {
    index(sys)
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|e| e["cwd"] == cwd.display().to_string())
                .cloned()
        })
        .unwrap_or_else(|| panic!("no index entry for {cwd:?}"))
}

fn list_ws(core: &Arc<Core>) -> Vec<Workspace> {
    match core.dispatch(Command::WorkspaceList).unwrap() {
        CommandOutput::Workspaces { workspaces } => workspaces,
        other => panic!("expected workspaces: {other:?}"),
    }
}

#[tokio::test]
async fn open_writes_the_open_flag_true() {
    let sys = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().into())
        .build();
    open_ws(&core, cwd.path()).await;
    let entry = entry_for(sys.path(), cwd.path());
    assert_eq!(entry["open"], true, "open marks the entry open: {entry:?}");
}

#[tokio::test]
async fn an_index_entry_without_the_flag_loads_as_open() {
    // Migration: an index written before the flag existed carries no
    // `open` field at all; the load must treat it as open, and a later
    // close must persist the flag onto it.
    let sys = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        sys.path().join("workspaces.json"),
        format!(
            "[{{\"id\":\"w-old\",\"name\":\"{}\",\"cwd\":\"{}\"}}]",
            cwd.path().file_name().unwrap().to_string_lossy(),
            cwd.path().display()
        ),
    )
    .unwrap();
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().into())
        .build();
    let list = list_ws(&core);
    assert_eq!(list.len(), 1, "the flagless entry restores: {list:?}");
    let w = list.into_iter().next().unwrap();

    core.dispatch(Command::WorkspaceClose { workspace: w.id })
        .unwrap();
    let entry = entry_for(sys.path(), cwd.path());
    assert_eq!(
        entry["open"], false,
        "the close persisted the flag: {entry:?}"
    );
}

#[tokio::test]
async fn close_persists_and_drops_the_workspace() {
    let sys = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().into())
        .build();
    let w = open_ws(&core, cwd.path()).await;
    assert_eq!(list_ws(&core).len(), 1);

    assert_eq!(
        core.dispatch(Command::WorkspaceClose {
            workspace: w.id.clone()
        })
        .unwrap(),
        CommandOutput::None
    );
    assert!(
        list_ws(&core).is_empty(),
        "the closed workspace is out of the live list"
    );
    let entry = entry_for(sys.path(), cwd.path());
    assert_eq!(entry["open"], false, "the flag persisted: {entry:?}");
}

#[tokio::test]
async fn close_refuses_an_unknown_workspace() {
    let core = CoreBuilder::custom(providers()).build();
    let err = core
        .dispatch(Command::WorkspaceClose {
            workspace: "w-none".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::NotFound { .. }),
        "an unknown workspace id is a NotFound, not a silent ok: {err:?}"
    );
}

#[tokio::test]
async fn reopen_sets_the_flag_back_to_open() {
    let sys = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().into())
        .build();
    let w = open_ws(&core, cwd.path()).await;
    core.dispatch(Command::WorkspaceClose {
        workspace: w.id.clone(),
    })
    .unwrap();
    assert_eq!(entry_for(sys.path(), cwd.path())["open"], false);

    let again = open_ws(&core, cwd.path()).await;
    assert_eq!(list_ws(&core).len(), 1, "the workspace re-opens");
    let entry = entry_for(sys.path(), cwd.path());
    assert_eq!(entry["open"], true, "reopen flips the flag back: {entry:?}");
    assert_eq!(again.id, w.id);
}

#[tokio::test]
async fn a_closed_workspace_stays_closed_across_a_restart() {
    let sys = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    {
        let core = CoreBuilder::custom(providers())
            .with_system_dir(sys.path().into())
            .build();
        let w = open_ws(&core, cwd.path()).await;
        core.dispatch(Command::WorkspaceClose { workspace: w.id })
            .unwrap();
    }
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().into())
        .build();
    assert!(
        list_ws(&core).is_empty(),
        "boot restores open workspaces only"
    );
}
