use super::*;

use crate::harness::testkit::*;

#[tokio::test]
async fn a_skill_added_to_a_project_root_updates_the_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, tmp.path()).await;
    let collected = collect_events(&core);
    write_skill_fixture(
        tmp.path(),
        ".tau/skills/added",
        "---\nname: added\ndescription: added mid-session\n---\nBody.\n",
    );
    let skills = wait_for_skill_list_changed(&collected, &workspace.id, |s| {
        s.iter().any(|x| x.name == "added")
    })
    .await;
    assert_eq!(
        skills.len(),
        1,
        "the full state is the new skill alone: {skills:?}"
    );
    assert_eq!(skills[0].name, "added");
}

/// Editing a skill's frontmatter (a name change) replaces the registry
/// entry: the final state carries the new name and the old one is gone.
#[tokio::test]

async fn an_edited_skill_name_replaces_the_registry_entry() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/s",
        "---\nname: old\ndescription: the old name\n---\nBody.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, tmp.path()).await;
    let collected = collect_events(&core);
    std::fs::write(
        tmp.path().join(".agents/skills/s/SKILL.md"),
        "---\nname: new\ndescription: the new name\n---\nBody.\n",
    )
    .unwrap();
    let skills = wait_for_skill_list_changed(&collected, &workspace.id, |s| {
        s.iter().any(|x| x.name == "new") && !s.iter().any(|x| x.name == "old")
    })
    .await;
    assert_eq!(
        skills.len(),
        1,
        "the final state has the renamed skill only: {skills:?}"
    );
    assert_eq!(skills[0].description, "the new name");
}

/// Deleting a skill's SKILL.md empties the workspace's registry: the
/// final state is the empty list (a lost batch self-heals on the next
/// `skill_list`).
#[tokio::test]

async fn a_deleted_skill_leaves_an_empty_registry() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/gone",
        "---\nname: gone\ndescription: to be deleted\n---\nBody.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, tmp.path()).await;
    let collected = collect_events(&core);
    std::fs::remove_file(tmp.path().join(".agents/skills/gone/SKILL.md")).unwrap();
    let skills = wait_for_skill_list_changed(&collected, &workspace.id, |s| s.is_empty()).await;
    assert!(skills.is_empty());
}

/// A home-root change fans out to every open workspace (design #30):
/// the two home roots are watched once globally, and each workspace
/// gets its own `SkillListChanged` carrying the shared skill.
#[tokio::test]

async fn a_home_root_change_fans_out_to_all_open_workspaces() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sys = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers())
        .with_system_dir(sys.path().to_path_buf())
        .with_home(home.path().to_path_buf())
        .build();
    let w1 = open_ws(&core, tmp1.path()).await;
    let w2 = open_ws(&core, tmp2.path()).await;
    let collected = collect_events(&core);
    write_skill_fixture(
        home.path(),
        ".agents/skills/shared",
        "---\nname: shared\ndescription: from the home .agents\n---\nBody.\n",
    );
    let expect = |s: &[SkillInfo]| s.iter().any(|x| x.name == "shared");
    let s1 = wait_for_skill_list_changed(&collected, &w1.id, expect).await;
    let s2 = wait_for_skill_list_changed(&collected, &w2.id, expect).await;
    for skills in [&s1, &s2] {
        assert_eq!(
            skills.len(),
            1,
            "each registry is the shared skill alone: {skills:?}"
        );
        assert!(
            skills[0]
                .location
                .starts_with(home.path().to_str().unwrap()),
            "the home file's location: {}",
            skills[0].location
        );
    }
}

/// The `None` seam (the `custom` test shape) makes the home watcher a
/// no-op — there is no global watcher — while the project roots of an
/// open workspace are still watched.
#[tokio::test]

async fn none_seams_make_the_home_watcher_a_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    assert!(
        core.home_watcher.lock().unwrap().is_none(),
        "the None seam has no home roots to watch"
    );
    let workspace = open_ws(&core, tmp.path()).await;
    let collected = collect_events(&core);
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/proj",
        "---\nname: proj\ndescription: from the project .agents\n---\nBody.\n",
    );
    let skills = wait_for_skill_list_changed(&collected, &workspace.id, |s| {
        s.iter().any(|x| x.name == "proj")
    })
    .await;
    assert_eq!(skills.len(), 1);
}

/// The listing's shape (ticket #32): dirs first, then name; paths
/// workspace-relative; the design's exclusions never appear.
#[test]

fn file_list_lists_a_dir_with_exclusions_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    std::fs::create_dir_all(cwd.join("src/core")).unwrap();
    std::fs::write(cwd.join("README.md"), "hi").unwrap();
    std::fs::write(cwd.join("src/core/main.rs"), "fn main() {}").unwrap();
    for ex in [
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        "out",
        ".venv",
        "__pycache__",
    ] {
        std::fs::create_dir_all(cwd.join(ex)).unwrap();
        std::fs::write(cwd.join(ex).join("inside"), "x").unwrap();
    }
    let files = list_dir(cwd, cwd);
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    // Dirs first (src), then the file; no excluded names.
    assert_eq!(names, vec!["src", "README.md"]);
    let src = &files[0];
    assert!(src.dir);
    assert_eq!(src.path, "src");
    assert_eq!(files[1].path, "README.md");
    assert_eq!(files[1].size, 2);
    // A nested listing carries full relative paths.
    let nested = list_dir(cwd, &cwd.join("src"));
    assert_eq!(
        nested.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["src/core"]
    );
}

/// The acceptance bar measured on this repo (ticket #32): listing the
/// repo root yields zero `target/` entries — 3,777 of its 4,471 dirs
/// sit under it, so the exclusion is what keeps the listing usable.
#[test]

fn the_repo_root_listing_carries_no_target_entries() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let files = list_dir(&repo, &repo);
    assert!(
        !files.iter().any(|f| f.name == "target"),
        "target leaked into the listing: {:?}",
        files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
    );
    // The repo root is not empty.
    assert!(!files.is_empty());
}

/// A batch's stale-dir mapping (ticket #32): a file change names its
/// parent, a dir change names itself, excluded paths drop, an empty
/// batch is the rescan root, an unattributable path marks the whole
/// tree stale.
#[test]

fn tree_changed_dirs_maps_paths_to_stale_dirs() {
    let cwd = "/w";
    let empty: Batch = vec![];
    assert_eq!(tree_changed_dirs(cwd, &empty), vec![".".to_string()]);
    let batch: Batch = vec![
        PathBuf::from("/w/src/a.rs"),
        PathBuf::from("/w/src/core"),
        PathBuf::from("/w"),
        PathBuf::from("/w/target/release"),
        PathBuf::from("/elsewhere/x"),
    ];
    assert_eq!(
        tree_changed_dirs(cwd, &batch),
        vec![
            ".".to_string(),
            "src".to_string(),
            "src/a.rs".to_string(),
            "src/core".to_string()
        ]
    );
}

/// A write under a watched dir produces `FileTreeChanged` and the
/// refetch sees it — the pane's live path end-to-end (ticket #32),
/// asserting the final tree state, never the event sequence.
#[tokio::test]

async fn a_write_under_a_watched_dir_invalidates_and_refetches() {
    let tmp = tempfile::tempdir().unwrap();
    // Canonical: the OS may report the watched root through a resolved
    // symlink (macOS /var -> /private/var), and the stale-dir mapping
    // attributes paths by string prefix.
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let collected = collect_events(&core);
    let sub = cwd.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("new.rs"), "fn main() {}").unwrap();
    let changed =
        wait_for_file_tree_changed(&collected, &workspace.id, |c| c.iter().any(|d| d == "src"))
            .await;
    // The refetch (what the store does on invalidation) lists the new file.
    let files = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: "src".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    assert!(files.iter().any(|f| f.name == "new.rs"));
    assert!(
        changed.iter().any(|d| d == "src"),
        "the stale dir is src: {changed:?}"
    );
}
