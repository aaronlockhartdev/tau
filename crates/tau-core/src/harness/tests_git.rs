//! The git status/diff commands against real (temporary) repositories.
//! Covers the coverage triangle: a git repo, a non-git workspace, a missing path.

use super::*;
use crate::harness::testkit::*;
use tau_protocol::{FileGit, FileGitState};
/// Run git with the user's global/system config isolated, so host gpg-sign
/// and branch-name preferences cannot leak into the fixture repos.
fn git_ok(cwd: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &std::path::Path) {
    git_ok(dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.join("b.txt"), "two\n").unwrap();
    git_ok(dir, &["add", "a.txt", "b.txt"]);
    git_ok(dir, &["commit", "-qm", "init"]);
}

#[tokio::test]
async fn git_status_reports_branch_modifications_and_untracked() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("a.txt"), "changed\n").unwrap();
    std::fs::remove_file(tmp.path().join("b.txt")).unwrap();
    std::fs::write(tmp.path().join("c.txt"), "three\n").unwrap();

    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let status = match core
        .dispatch(Command::GitStatus {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::GitStatus { status } => status,
        other => panic!("expected git status: {other:?}"),
    };
    assert!(!status.branch.is_empty());
    assert_eq!(status.modified, vec!["a.txt"]);
    assert_eq!(status.deleted, vec!["b.txt"]);
    assert_eq!(status.untracked, vec!["c.txt"]);
}

#[tokio::test]
async fn git_status_in_a_non_git_workspace_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let err = core
        .dispatch(Command::GitStatus {
            workspace: workspace.id.clone(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "expected a transportable error, got {err:?}"
    );
}

#[tokio::test]
async fn git_show_returns_the_base_content_of_a_tracked_file() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // Diverge the working copy so the HEAD base differs from the file on disk.
    std::fs::write(tmp.path().join("a.txt"), "changed\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let text = match core
        .dispatch(Command::GitShow {
            workspace: workspace.id.clone(),
            path: "a.txt".into(),
        })
        .unwrap()
    {
        CommandOutput::GitShow { text } => text,
        other => panic!("expected git show: {other:?}"),
    };
    // HEAD holds the committed "one\n", not the working-copy "changed\n".
    assert_eq!(text, "one\n");
}

#[tokio::test]
async fn git_show_of_an_untracked_path_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("untracked.txt"), "new\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let err = core
        .dispatch(Command::GitShow {
            workspace: workspace.id.clone(),
            path: "untracked.txt".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "expected a transportable error, got {err:?}"
    );
}

#[tokio::test]
async fn git_show_from_a_subdir_workspace_resolves_the_toplevel_offset() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // A subdirectory becomes the workspace root; its file is toplevel-relative
    // as sub/nested.txt, so `git show` needs the `sub/` prefix the core adds.
    let sub = tmp.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("nested.txt"), "nested\n").unwrap();
    git_ok(tmp.path(), &["add", "sub/nested.txt"]);
    git_ok(tmp.path(), &["commit", "-qm", "sub"]);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, &sub).await;
    let text = match core
        .dispatch(Command::GitShow {
            workspace: workspace.id.clone(),
            path: "nested.txt".into(),
        })
        .unwrap()
    {
        CommandOutput::GitShow { text } => text,
        other => panic!("expected git show: {other:?}"),
    };
    assert_eq!(text, "nested\n");
}

#[tokio::test]
async fn file_listing_is_annotated_with_git_state_and_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // Ignore one untracked path; leave one untracked path shown.
    std::fs::write(tmp.path().join(".gitignore"), "hidden.txt\n").unwrap();
    std::fs::write(tmp.path().join("hidden.txt"), "h\n").unwrap();
    std::fs::write(tmp.path().join("shown.txt"), "s\n").unwrap();
    std::fs::write(tmp.path().join("a.txt"), "changed\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let files = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: ".".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    let by_path: std::collections::HashMap<_, _> =
        files.iter().map(|f| (f.path.as_str(), f)).collect();
    // Modified: state + numstat counts, not ignored.
    let a = by_path["a.txt"];
    assert_eq!(
        a.git,
        Some(FileGit {
            state: FileGitState::Modified,
            added: Some(1),
            deleted: Some(1)
        })
    );
    assert!(!a.ignored);
    // Untracked: state present, not ignored.
    let s = by_path["shown.txt"];
    assert_eq!(
        s.git.as_ref().map(|g| g.state),
        Some(FileGitState::Untracked)
    );
    assert!(!s.ignored);
    // Ignored: shown but greyed by the flag, no git state.
    let h = by_path["hidden.txt"];
    assert!(h.ignored);
    assert!(h.git.is_none());
}

#[tokio::test]
async fn a_file_ignored_by_a_nested_gitignore_is_greyed() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // A nested .gitignore (not the root one) ignores sub/nested.txt; the root
    // .gitignore knows nothing of it. Only folding in the nested file greys it.
    let sub = tmp.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join(".gitignore"), "nested.txt\n").unwrap();
    std::fs::write(sub.join("nested.txt"), "n\n").unwrap();
    std::fs::write(sub.join("visible.txt"), "v\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let files = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: "sub".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    let by_path: std::collections::HashMap<_, _> =
        files.iter().map(|f| (f.path.as_str(), f)).collect();
    // nested.txt is ignored by sub/.gitignore (a nested file the root-only
    // search never saw); its non-ignored sibling is not greyed.
    assert!(
        by_path["sub/nested.txt"].ignored,
        "nested-gitignore file is greyed"
    );
    assert!(
        !by_path["sub/visible.txt"].ignored,
        "non-ignored sibling is not greyed"
    );
}

#[tokio::test]
async fn git_status_in_a_subdir_workspace_is_cwd_relative_and_scoped() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // A subdirectory becomes the workspace, with its own tracked file.
    let sub = tmp.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("keep.txt"), "keep\n").unwrap();
    git_ok(tmp.path(), &["add", "sub/keep.txt"]);
    git_ok(tmp.path(), &["commit", "-qm", "sub"]);
    // A change inside the workspace, plus one outside it (must be dropped).
    std::fs::write(sub.join("keep.txt"), "changed\n").unwrap();
    std::fs::write(sub.join("new.txt"), "new\n").unwrap();
    std::fs::write(tmp.path().join("a.txt"), "changed\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, &sub).await;
    let status = match core
        .dispatch(Command::GitStatus {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::GitStatus { status } => status,
        other => panic!("expected git status: {other:?}"),
    };
    // cwd-relative (no "sub/" prefix) and scoped to the workspace subtree.
    assert_eq!(status.modified, vec!["keep.txt"]);
    assert_eq!(status.untracked, vec!["new.txt"]);
    assert!(!status.modified.iter().any(|p| p == "a.txt"));
}

#[tokio::test]
async fn file_listing_in_a_subdir_workspace_carries_git_state() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    let sub = tmp.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("keep.txt"), "keep\n").unwrap();
    git_ok(tmp.path(), &["add", "sub/keep.txt"]);
    git_ok(tmp.path(), &["commit", "-qm", "sub"]);
    std::fs::write(sub.join("keep.txt"), "changed\n").unwrap();
    std::fs::write(sub.join("new.txt"), "new\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, &sub).await;
    let files = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: ".".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    let by_path: std::collections::HashMap<_, _> =
        files.iter().map(|f| (f.path.as_str(), f)).collect();
    // Badges resolve for a subdir workspace: the cwd-relative path matches the
    // (now cwd-relative) status entry.
    assert_eq!(
        by_path["keep.txt"].git,
        Some(FileGit {
            state: FileGitState::Modified,
            added: Some(1),
            deleted: Some(1),
        })
    );
    assert_eq!(
        by_path["new.txt"].git.as_ref().map(|g| g.state),
        Some(FileGitState::Untracked)
    );
}

#[tokio::test]
async fn a_subdir_workspace_neither_leaks_a_sibling_gitignore_nor_loses_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // A sibling toplevel dir (sub2/) with a .gitignore, and a same-named dir
    // inside the workspace (sub/sub2/): the sibling's pattern must not reach in.
    // The workspace's own .gitignore must still apply.
    let sub = tmp.path().join("sub");
    let sib = tmp.path().join("sub2");
    std::fs::create_dir_all(sub.join("sub2")).unwrap();
    std::fs::create_dir(&sib).unwrap();
    std::fs::write(sib.join(".gitignore"), "notes.md\n").unwrap();
    std::fs::write(sub.join("sub2/notes.md"), "n\n").unwrap();
    std::fs::write(sub.join(".gitignore"), "inner.log\n").unwrap();
    std::fs::write(sub.join("inner.log"), "l\n").unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, &sub).await;
    let root = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: ".".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    let by_path: std::collections::HashMap<_, _> =
        root.iter().map(|f| (f.path.as_str(), f)).collect();
    // The workspace's own .gitignore applies (inner.log is greyed).
    assert!(by_path["inner.log"].ignored, "workspace .gitignore applies");
    // The sibling sub2/.gitignore does NOT reach into sub/sub2/ (no false positive).
    let sub2 = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: "sub2".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    let notes = sub2.iter().find(|f| f.path == "sub2/notes.md").unwrap();
    assert!(
        !notes.ignored,
        "sibling .gitignore does not leak into the workspace"
    );
}

#[tokio::test]
async fn folder_greying_in_a_subdir_workspace_ignores_only_ignored_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    // A subdir workspace with a tracked dir (real content) and an ignored-only dir.
    let sub = tmp.path().join("sub");
    std::fs::create_dir_all(sub.join("real")).unwrap();
    std::fs::create_dir_all(sub.join("junk")).unwrap();
    std::fs::write(sub.join("real/keep.txt"), "keep\n").unwrap();
    std::fs::write(sub.join(".gitignore"), "junk/\n").unwrap();
    std::fs::write(sub.join("junk/x.txt"), "x\n").unwrap();
    git_ok(tmp.path(), &["add", "sub/real/keep.txt", "sub/.gitignore"]);
    git_ok(tmp.path(), &["commit", "-qm", "sub"]);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, &sub).await;
    let files = match core
        .dispatch(Command::FileList {
            workspace: workspace.id.clone(),
            path: ".".into(),
        })
        .unwrap()
    {
        CommandOutput::Files { files } => files,
        other => panic!("expected files: {other:?}"),
    };
    let by_path: std::collections::HashMap<_, _> =
        files.iter().map(|f| (f.path.as_str(), f)).collect();
    // The real dir (tracked content) is not greyed; the junk dir (only ignored
    // content) is — both resolved cwd-relative for the subdir workspace.
    assert!(
        !by_path["real"].ignored,
        "dir with tracked content is not greyed"
    );
    assert!(
        by_path["junk"].ignored,
        "dir whose only content is ignored is greyed"
    );
}
