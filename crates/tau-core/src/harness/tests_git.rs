//! The git status/diff commands against real (temporary) repositories.
//! Covers the DoD triangle: a git repo, a non-git workspace, a missing path.

use super::*;
use crate::harness::testkit::*;

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

    let core = CoreBuilder::custom(providers()).build();
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
    let core = CoreBuilder::custom(providers()).build();
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
async fn git_diff_of_a_missing_path_is_empty_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, tmp.path()).await;
    let diff = match core
        .dispatch(Command::GitDiff {
            workspace: workspace.id.clone(),
            path: "nope.txt".into(),
        })
        .unwrap()
    {
        CommandOutput::GitDiff { diff } => diff,
        other => panic!("expected git diff: {other:?}"),
    };
    assert_eq!(diff.path, "nope.txt");
    assert!(diff.diff.is_empty());
}

#[tokio::test]
async fn git_diff_of_a_modified_file_is_unified() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("a.txt"), "changed\n").unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, tmp.path()).await;
    let diff = match core
        .dispatch(Command::GitDiff {
            workspace: workspace.id.clone(),
            path: "a.txt".into(),
        })
        .unwrap()
    {
        CommandOutput::GitDiff { diff } => diff,
        other => panic!("expected git diff: {other:?}"),
    };
    assert_eq!(diff.path, "a.txt");
    assert!(diff.diff.contains("+changed"), "diff: {}", diff.diff);
}
