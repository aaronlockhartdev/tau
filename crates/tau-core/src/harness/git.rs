//! Workspace git status and one-file content (workbench P4), now on `gix`
//! (the pure-Rust git library) instead of `git` CLI shell-outs (ADR-0011).
//! The file-tree badges, the changes tab, and the file-content view need the
//! status split, per-file numstat, an authoritative per-path ignore check
//! (greying `target/` and empty ignored dirs like `.tau/skills` without
//! enumerating them), and a file's content at HEAD for the diff base.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use tau_protocol::{FileEntry, FileGit, FileGitState, GitStatus, ProtocolError};

/// Discover the git repository at (or enclosing) `cwd` — a workspace may be a
/// subdirectory of the repo. A non-git path is a protocol catch-all (matches
/// `git_status_in_a_non_git_workspace_is_an_error`).
fn open_repo(cwd: &Path) -> Result<gix::Repository, ProtocolError> {
    gix::discover(cwd).map_err(|e| ProtocolError::Other {
        message: format!("git open: {e}"),
    })
}


/// The status split from an open repository: the branch name plus the
/// worktree-vs-index changes, classified into modified / deleted / untracked.
fn status_of(repo: &gix::Repository) -> Result<GitStatus, ProtocolError> {
    let branch = repo
        .head_name()
        .map_err(|e| ProtocolError::Other {
            message: format!("git head: {e}"),
        })?
        .map(|name| name.shorten().to_string())
        .unwrap_or_default();

    let platform = repo
        .status(gix::progress::Discard)
        .map_err(|e| ProtocolError::Other {
            message: format!("git status: {e}"),
        })?;
    let changes = platform
        .into_iter(std::iter::empty::<gix::bstr::BString>())
        .map_err(|e| ProtocolError::Other {
            message: format!("git status: {e}"),
        })?;

    let mut status = GitStatus {
        branch,
        modified: Vec::new(),
        untracked: Vec::new(),
        deleted: Vec::new(),
    };
    for change in changes {
        let change = change.map_err(|e| ProtocolError::Other {
            message: format!("git status: {e}"),
        })?;
        // Only worktree-vs-index changes feed the badge model; staged
        // (tree-vs-index) changes are out of scope.
        let gix::status::Item::IndexWorktree(change) = change else {
            continue
        };
        match change {
            gix::status::index_worktree::Item::Modification {
                rela_path,
                status: entry_status,
                ..
            } => {
                let path = String::from_utf8_lossy(&rela_path).into_owned();
                if let gix::status::plumbing::index_as_worktree::EntryStatus::Change(c) = entry_status {
                    match c {
                        gix::status::plumbing::index_as_worktree::Change::Modification { .. }
                        | gix::status::plumbing::index_as_worktree::Change::Type { .. } => {
                            status.modified.push(path);
                        }
                        gix::status::plumbing::index_as_worktree::Change::Removed => {
                            status.deleted.push(path);
                        }
                        gix::status::plumbing::index_as_worktree::Change::SubmoduleModification(_) => {}
                    }
                }
            }
            gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
                status
                    .untracked
                    .push(String::from_utf8_lossy(&entry.rela_path).into_owned());
            }
            gix::status::index_worktree::Item::Rewrite { .. } => {}
        }
    }
    Ok(status)
}



/// A file's content at HEAD (the file-content view's diff base). The path is
/// resolved toplevel-relative — the HEAD tree is rooted at the repo root, not
/// the workspace the dispatch layer hands over. A path absent from HEAD is an
/// error (the caller treats it as "no base", i.e. an untracked file).
pub fn git_show(cwd: &Path, rel: &str) -> Result<String, ProtocolError> {
    let repo = open_repo(cwd)?;
    // The command's path is cwd-relative; the HEAD tree is rooted at the
    // toplevel. Express the cwd→toplevel offset in canonical form (so a
    // /var → /private/var symlink can't defeat the strip) and prepend it.
    let workdir = repo.workdir().ok_or(ProtocolError::Other {
        message: "git show: not a worktree".into(),
    })?;
    let top = std::fs::canonicalize(workdir).map_err(|e| ProtocolError::Other {
        message: format!("git toplevel: {e}"),
    })?;
    let cwdc = std::fs::canonicalize(cwd).map_err(|e| ProtocolError::Other {
        message: format!("git cwd: {e}"),
    })?;
    let prefix = cwdc.strip_prefix(&top).map(Path::to_path_buf).unwrap_or_default();
    let spec = if prefix.as_os_str().is_empty() {
        rel.to_string()
    } else {
        format!("{}/{}", prefix.display(), rel)
    };
    let tree = repo
        .head_commit()
        .map_err(|e| ProtocolError::Other {
            message: format!("git show: {e}"),
        })?
        .tree()
        .map_err(|e| ProtocolError::Other {
            message: format!("git show: {e}"),
        })?;
    let text = head_blob(&repo, &tree, &spec).ok_or(ProtocolError::Other {
        message: format!("git show: {rel} is not in HEAD"),
    })?;
    // file_read strips \r; normalize the base the same way, or a CRLF file
    // diffs whole-file (a raw \r on every line reads as a change to each).
    Ok(text.replace("\r\n", "\n"))
}

/// The numstat map's value: `(added, deleted)`; a `None` count is
/// unmeasurable (binary).
pub type NumstatCounts = (Option<u32>, Option<u32>);

/// A workspace's combined git state (file-tree plan, phase 2), now built by a
/// single in-process `gix` pass instead of five `git` spawns. `ignore` carries
/// the loaded gitignore patterns so `annotate` can grey any path — empty dirs
/// included — without enumerating the worktree.
#[derive(Clone)]
pub struct GitSnapshot {
    pub status: GitStatus,
    /// Changed path → (added, deleted); untracked files are measured here, at
    /// snapshot time, so neither the tree nor the changes tab does per-row
    /// file reads.
    pub numstat: HashMap<String, NumstatCounts>,
    /// Loaded ignore patterns (info/exclude + the root .gitignore). A path is
    /// greyed when it or an ancestor dir matches — see `is_ignored`.
    pub ignore: gix::ignore::Search,
}

pub fn git_snapshot(cwd: &Path) -> Result<GitSnapshot, ProtocolError> {
    let repo = open_repo(cwd)?;
    let status = status_of(&repo)?;

    // numstat: line-diff each modified file against its HEAD blob (a bounded
    // set — the ignored files never appear in `status`), and count lines for
    // untracked files. Never touches the 467k-file `target/`.
    let head_tree = repo
        .head_commit()
        .ok()
        .and_then(|commit| commit.tree().ok());
    let mut numstat = HashMap::new();
    for path in &status.modified {
        numstat.insert(
            path.clone(),
            modified_numstat(&repo, head_tree.as_ref(), cwd, path),
        );
    }
    for path in &status.untracked {
        numstat.insert(path.clone(), (count_lines(&cwd.join(path)), Some(0)));
    }

    // The ignore patterns: info/exclude + the worktree's root .gitignore.
    // Nested .gitignore files are not consulted (v0 scope; the status walk
    // above already uses gix's full ignore machinery for pruning).
    let mut buf = Vec::new();
    let ignore = gix::ignore::Search::from_git_dir(
        repo.git_dir(),
        repo.workdir().map(|dir| dir.join(".gitignore")),
        &mut buf,
        gix::ignore::search::Ignore::default(),
    )
    .map_err(|e| ProtocolError::Other {
        message: format!("git ignore: {e}"),
    })?;

    Ok(GitSnapshot {
        status,
        numstat,
        ignore,
    })
}

/// A modified file's `(added, deleted)`: a line-diff of its HEAD blob against
/// the worktree copy. A binary worktree copy is unmeasurable; a file with no
/// HEAD base reads as all-added.
fn modified_numstat(
    repo: &gix::Repository,
    head_tree: Option<&gix::Tree>,
    cwd: &Path,
    path: &str,
) -> NumstatCounts {
    let Ok(new) = std::fs::read_to_string(cwd.join(path)) else {
        return (None, None); // binary worktree copy
    };
    match head_tree.and_then(|tree| head_blob(repo, tree, path)) {
        Some(old) => {
            let (added, deleted) = line_diff_counts(&old, &new);
            (Some(added), Some(deleted))
        }
        None => (
            Some(u32::try_from(new.lines().count()).unwrap_or(u32::MAX)),
            Some(0),
        ),
    }
}

/// `(added, deleted)` line counts of a line-based diff, via `similar`. A
/// replace is emitted by `similar` as a paired delete + insert, so both sides
/// are counted.
fn line_diff_counts(old: &str, new: &str) -> (u32, u32) {
    let diff = similar::TextDiff::from_lines(old, new);
    let (mut added, mut deleted) = (0u32, 0u32);
    for op in diff.iter_all_changes() {
        let lines = u32::try_from(op.value().lines().count()).unwrap_or(u32::MAX);
        match op.tag() {
            similar::ChangeTag::Insert => added = added.saturating_add(lines),
            similar::ChangeTag::Delete => deleted = deleted.saturating_add(lines),
            similar::ChangeTag::Equal => {}
        }
    }
    (added, deleted)
}

/// Read a file's bytes at HEAD by walking the tree path segment by segment
/// (`gix` has no `find_file`; a `Tree` only exposes single-name `find_entry`).
fn head_blob(repo: &gix::Repository, tree: &gix::Tree, rel: &str) -> Option<String> {
    let mut parts = rel.split('/');
    let name = parts.next()?;
    let entry = tree.find_entry(name)?;
    match entry.kind() {
        gix::object::tree::EntryKind::Tree => {
            let rest = parts.collect::<Vec<_>>().join("/");
            let next = repo.find_tree(entry.id()).ok()?;
            head_blob(repo, &next, &rest)
        }
        gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
            let blob = repo.find_blob(entry.id()).ok()?;
            Some(String::from_utf8_lossy(&blob.data).into_owned())
        }
        _ => None,
    }
}

/// An untracked file's line count (the numstat stand-in): a pure read,
/// capped so a huge untracked file stays cheap.
pub fn count_lines(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = std::io::BufReader::new(file);
    let mut line = String::new();
    let mut n = 0u32;
    while n < 100_000 {
        if buf.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        line.clear();
        n += 1;
    }
    Some(n)
}

/// Annotate a dir listing with the snapshot's ignore + git state
/// (file-tree plan, phase 2). A node is greyed when it (or an ancestor dir)
/// is .gitignored; a non-ignored file also carries its git state.
pub fn annotate(files: &mut [FileEntry], snap: Option<&GitSnapshot>) {
    let Some(snap) = snap else { return };
    for f in files.iter_mut() {
        f.ignored = is_ignored(&f.path, f.dir, snap);
        if f.dir {
            continue;
        }
        f.git = git_state_of(&f.path, snap);
    }
}

/// A path is ignored when it, or any ancestor directory, matches a loaded
/// ignore pattern — the rule that greys an empty ignored dir (`.tau/skills`)
/// just like its ignored parent (`.tau/`).
fn is_ignored(path: &str, is_dir: bool, snap: &GitSnapshot) -> bool {
    let mut current = path;
    let mut at_leaf = true;
    loop {
        let dir = if at_leaf { is_dir } else { true };
        let matched = snap
            .ignore
            .pattern_matching_relative_path(
                gix::bstr::BStr::new(current.as_bytes()),
                Some(dir),
                gix::glob::pattern::Case::default(),
            )
            .is_some_and(|m| !m.pattern.is_negative());
        if matched {
            return true;
        }
        let Some((parent, _)) = current.rsplit_once('/') else {
            return false;
        };
        current = parent;
        at_leaf = false;
    }
}

fn git_state_of(path: &str, snap: &GitSnapshot) -> Option<FileGit> {
    if snap.status.deleted.iter().any(|p| p == path) {
        return Some(FileGit {
            state: FileGitState::Deleted,
            added: None,
            deleted: None,
        });
    }
    if snap.status.untracked.iter().any(|p| p == path) {
        return Some(FileGit {
            state: FileGitState::Untracked,
            added: None,
            deleted: None,
        });
    }
    let (added, deleted) = snap
        .numstat
        .get(path)
        .copied()
        .unwrap_or((None, None));
    snap.status
        .modified
        .iter()
        .any(|p| p == path)
        .then_some(FileGit {
            state: FileGitState::Modified,
            added,
            deleted,
        })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn ignore_search(patterns: &[&str]) -> gix::ignore::Search {
        gix::ignore::Search::from_overrides(
            patterns.to_vec(),
            gix::ignore::search::Ignore::default(),
        )
    }

    fn entry(name: &str, path: &str, dir: bool) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: path.into(),
            dir,
            size: 0,
            ignored: false,
            git: None,
        }
    }

    #[test]
    fn annotate_marks_ignored_and_git_state() {
        let snap = GitSnapshot {
            status: GitStatus {
                branch: "main".into(),
                modified: vec!["src/a.rs".into()],
                untracked: vec!["new.md".into()],
                deleted: vec![],
            },
            numstat: {
                let mut m = HashMap::new();
                m.insert("src/a.rs".to_owned(), (Some(3), Some(1)));
                m
            },
            ignore: ignore_search(&["target/"]),
        };
        let mut files = vec![
            entry("a.rs", "src/a.rs", false),
            entry("new.md", "new.md", false),
            entry("x", "target/x", false),
        ];
        annotate(&mut files, Some(&snap));
        assert_eq!(
            files[0].git,
            Some(FileGit {
                state: FileGitState::Modified,
                added: Some(3),
                deleted: Some(1)
            })
        );
        assert_eq!(
            files[1].git,
            Some(FileGit {
                state: FileGitState::Untracked,
                added: None,
                deleted: None
            })
        );
        assert!(files[2].ignored && files[2].git.is_none());
    }

    #[test]
    fn a_path_under_an_ignored_dir_is_ignored() {
        let snap = GitSnapshot {
            status: GitStatus {
                branch: "main".into(),
                modified: vec![],
                untracked: vec![],
                deleted: vec![],
            },
            numstat: HashMap::new(),
            ignore: ignore_search(&[".tau/", "node_modules/"]),
        };
        // `.tau/skills` is empty and never in the pattern list, but its
        // ancestor `.tau` is ignored, so the whole subtree is greyed.
        assert!(is_ignored(".tau", true, &snap));
        assert!(is_ignored(".tau/skills", true, &snap));
        assert!(is_ignored("node_modules/pkg.js", false, &snap));
        assert!(!is_ignored("crates", true, &snap));
        assert!(!is_ignored("crates/tau-core", true, &snap));
    }

    #[test]
    fn annotate_is_a_noop_without_a_snapshot() {
        let mut files = vec![entry("a.rs", "a.rs", false)];
        annotate(&mut files, None);
        assert!(!files[0].ignored && files[0].git.is_none());
    }

    #[test]
    fn line_diff_counts_added_and_deleted_lines() {
        // b→x is a replace (1 delete + 1 insert); the trailing y is an insert.
        let (added, deleted) = line_diff_counts("a\nb\nc\n", "a\nx\nc\ny\n");
        assert_eq!((added, deleted), (2, 1));
    }
}
