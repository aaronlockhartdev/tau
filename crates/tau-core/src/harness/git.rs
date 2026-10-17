//! Workspace git status and one-file content (workbench P4), now on `gix`
//! (the pure-Rust git library) instead of `git` CLI shell-outs (ADR-0011).
//! The file-tree badges, the changes tab, and the file-content view need the
//! status split, per-file numstat, an authoritative per-path ignore check
//! (greying `target/` and empty ignored dirs like `.tau/skills` without
//! enumerating them), and a file's content at HEAD for the diff base.

use std::collections::{HashMap, HashSet};
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
            continue;
        };
        match change {
            gix::status::index_worktree::Item::Modification {
                rela_path,
                status: entry_status,
                ..
            } => {
                let path = String::from_utf8_lossy(&rela_path).into_owned();
                if let gix::status::plumbing::index_as_worktree::EntryStatus::Change(c) =
                    entry_status
                {
                    match c {
                        gix::status::plumbing::index_as_worktree::Change::Modification {
                            ..
                        }
                        | gix::status::plumbing::index_as_worktree::Change::Type { .. } => {
                            status.modified.push(path);
                        }
                        gix::status::plumbing::index_as_worktree::Change::Removed => {
                            status.deleted.push(path);
                        }
                        gix::status::plumbing::index_as_worktree::Change::SubmoduleModification(
                            _,
                        ) => {}
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

/// The workspace's position within the repo, workdir-relative and in canonical
/// form (so a /var → /private/var symlink can't defeat the strip): empty for a
/// toplevel workspace, e.g. `sub` for one opened at <repo>/sub. Shared by
/// `git_show` (which prepends it to a cwd-relative path) and `git_snapshot`
/// (which stores it so `annotate` can translate cwd-relative tree paths into the
/// toplevel-relative terms the rest of the snapshot uses).
fn cwd_offset(repo: &gix::Repository, cwd: &Path) -> Result<String, ProtocolError> {
    let workdir = repo.workdir().ok_or(ProtocolError::Other {
        message: "git offset: not a worktree".into(),
    })?;
    let top = std::fs::canonicalize(workdir).map_err(|e| ProtocolError::Other {
        message: format!("git toplevel: {e}"),
    })?;
    let cwdc = std::fs::canonicalize(cwd).map_err(|e| ProtocolError::Other {
        message: format!("git cwd: {e}"),
    })?;
    Ok(cwdc
        .strip_prefix(&top)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default())
}

/// Express a toplevel-relative status in the workspace's cwd-relative terms for
/// the wire: strip the workspace's `offset` from each path and drop changes
/// outside the workspace (a subdir workspace reports only its own subtree). An
/// empty offset (toplevel workspace) is a no-op.
pub fn rebase_status(status: GitStatus, offset: &str) -> GitStatus {
    let prefix = format!("{offset}/");
    let strip = |p: &String| {
        if offset.is_empty() {
            Some(p.clone())
        } else {
            p.strip_prefix(&prefix).map(ToString::to_string)
        }
    };
    GitStatus {
        branch: status.branch,
        modified: status.modified.iter().filter_map(&strip).collect(),
        untracked: status.untracked.iter().filter_map(&strip).collect(),
        deleted: status.deleted.iter().filter_map(&strip).collect(),
    }
}

/// A file's content at HEAD (the file-content view's diff base). The path is
/// resolved toplevel-relative — the HEAD tree is rooted at the repo root, not
/// the workspace the dispatch layer hands over. A path absent from HEAD is an
/// error (the caller treats it as "no base", i.e. an untracked file).
pub fn git_show(cwd: &Path, rel: &str) -> Result<String, ProtocolError> {
    let repo = open_repo(cwd)?;
    // The command's path is cwd-relative; the HEAD tree is rooted at the
    // toplevel. Prepend the cwd→toplevel offset (empty for a toplevel workspace).
    let offset = cwd_offset(&repo, cwd)?;
    let spec = if offset.is_empty() {
        rel.to_string()
    } else {
        format!("{offset}/{rel}")
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
    /// Loaded ignore patterns: info/exclude, the root .gitignore, and every
    /// nested .gitignore (each scoped to its own dir). A path is greyed when it
    /// or an ancestor dir matches — see `is_ignored`.
    pub ignore: gix::ignore::Search,
    /// Dir prefixes holding a tracked file (from the index). A folder with no
    /// tracked and no shown-untracked content is fully-ignored and greyed; this
    /// set keeps real folders (e.g. `crates/`) out of the greying.
    pub tracked_dirs: HashSet<String>,
    /// Dir prefixes holding a shown (non-ignored) untracked file.
    pub shown_dirs: HashSet<String>,
    /// The workspace's workdir-relative position (empty for a toplevel
    /// workspace). `annotate` prepends it to each cwd-relative tree path so the
    /// toplevel-relative lookups below line up for a subdir workspace.
    pub offset: String,
}

pub fn git_snapshot(cwd: &Path) -> Result<GitSnapshot, ProtocolError> {
    let repo = open_repo(cwd)?;
    let status = status_of(&repo)?;
    // The snapshot stays in gix's toplevel-relative terms; the workspace's
    // position within the repo rides along for `annotate` to translate
    // cwd-relative tree paths. File reads below are toplevel-relative too.
    let workdir = repo.workdir().ok_or(ProtocolError::Other {
        message: "git snapshot: not a worktree".into(),
    })?;
    let offset = cwd_offset(&repo, cwd)?;

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
            modified_numstat(&repo, head_tree.as_ref(), workdir, path),
        );
    }
    for path in &status.untracked {
        numstat.insert(path.clone(), (count_lines(&workdir.join(path)), Some(0)));
    }

    // The ignore patterns: info/exclude, the root .gitignore, and every nested
    // .gitignore reachable through non-ignored dirs (build_ignore_search folds
    // them in, each scoped to its own directory).
    let ignore = build_ignore_search(&repo)?;

    // Folder-greying sets: a dir is greyed when it holds neither a tracked
    // nor a shown-untracked file - everything under it is ignored. The index
    // and the status untracked set are both bounded (ignored dirs like
    // target/ appear in neither), so this stays cheap.
    let mut tracked_dirs = HashSet::new();
    if let Some(tree) = &head_tree {
        collect_tree_paths(&repo, tree, "", &mut tracked_dirs);
    }
    let mut shown_dirs = HashSet::new();
    for p in &status.untracked {
        add_dir_prefixes(p, &mut shown_dirs);
    }

    Ok(GitSnapshot {
        status,
        numstat,
        ignore,
        tracked_dirs,
        shown_dirs,
        offset,
    })
}

/// The worktree's ignore patterns: info/exclude, the root .gitignore, and every
/// nested .gitignore reachable through non-ignored directories. The recursive
/// walk prunes dirs the (growing) search already ignores, so it never descends
/// into e.g. `target/` — the 467k-file case the gix migration (ADR-0011) was
/// meant to avoid. Each nested file is scoped to its own directory (the search
/// `base`), so git's "closest .gitignore wins" rule holds via the search's
/// reverse-iteration match order.
/// Bases are workdir-rooted (toplevel-relative); `annotate` translates each
/// cwd-relative tree path into those terms before checking, so a subdir
/// workspace matches correctly and a sibling subtree's pattern never leaks in.
fn build_ignore_search(repo: &gix::Repository) -> Result<gix::ignore::Search, ProtocolError> {
    let mut buf = Vec::new();
    let mut search = gix::ignore::Search::from_git_dir(
        repo.git_dir(),
        repo.workdir().map(|dir| dir.join(".gitignore")),
        &mut buf,
        gix::ignore::search::Ignore::default(),
    )
    .map_err(|e| ProtocolError::Other {
        message: format!("git ignore: {e}"),
    })?;
    if let Some(workdir) = repo.workdir() {
        collect_nested_gitignores(workdir, "", workdir, &mut search);
    }
    Ok(search)
}

/// Recursively fold `.gitignore` files under `dir` into `search`, pruning dirs
/// the search already ignores (their subtree is greyed wholesale, so no nested
/// pattern in it can matter). `workdir` is the repo root the pattern bases are
/// scoped against; `rel` is `dir`'s workdir-relative path, for the prune check.
fn collect_nested_gitignores(
    dir: &Path,
    rel: &str,
    workdir: &Path,
    search: &mut gix::ignore::Search,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        // The repo's own .git dir is never worktree content.
        if name == ".git" {
            continue;
        }
        let rel_sub = if rel.is_empty() {
            name.to_string()
        } else {
            format!("{rel}/{name}")
        };
        if is_ignored_by_search(search, &rel_sub) {
            continue;
        }
        let subdir = dir.join(&file_name);
        let gitignore = subdir.join(".gitignore");
        if let Ok(bytes) = std::fs::read(&gitignore) {
            search.add_patterns_buffer(
                &bytes,
                gitignore,
                Some(workdir),
                gix::ignore::search::Ignore::default(),
            );
        }
        collect_nested_gitignores(&subdir, &rel_sub, workdir, search);
    }
}

/// A workdir-relative dir is ignored when the search matches it as a directory
/// with a non-negative pattern. The prune test for the nested-gitignore walk:
/// an ancestor match would have pruned higher up, so only this level is checked.
fn is_ignored_by_search(search: &gix::ignore::Search, rel_dir: &str) -> bool {
    search
        .pattern_matching_relative_path(
            gix::bstr::BStr::new(rel_dir.as_bytes()),
            Some(true),
            gix::glob::pattern::Case::default(),
        )
        .is_some_and(|m| !m.pattern.is_negative())
}

/// A modified file's `(added, deleted)`: a line-diff of its HEAD blob against
/// the worktree copy. A binary worktree copy is unmeasurable; a file with no
/// HEAD base reads as all-added.
fn modified_numstat(
    repo: &gix::Repository,
    head_tree: Option<&gix::Tree>,
    base: &Path,
    path: &str,
) -> NumstatCounts {
    let Ok(new) = std::fs::read_to_string(base.join(path)) else {
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

/// Recursively collect the ancestor-directory prefixes of every blob in `tree`
/// into `into` — the set of dirs that transitively hold a tracked file. A blob
/// `a/b/c` contributes `a`, `a/b`, `a/b/c`; git stores no empty trees, so every
/// recorded dir has tracked content beneath it.
fn collect_tree_paths(
    repo: &gix::Repository,
    tree: &gix::Tree,
    prefix: &str,
    into: &mut HashSet<String>,
) {
    let Ok(decoded) = tree.decode() else { return };
    for entry in &decoded.entries {
        let name = String::from_utf8_lossy(entry.filename).into_owned();
        let full = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.mode.is_tree() {
            if let Ok(sub) = repo.find_tree(entry.oid) {
                collect_tree_paths(repo, &sub, &full, into);
            }
        } else {
            add_dir_prefixes(&full, into);
        }
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
        // The snapshot is toplevel-relative; a subdir workspace's tree paths are
        // cwd-relative, so translate before any lookup (empty offset = no-op).
        let rel = workdir_rel(&snap.offset, &f.path);
        f.ignored = if f.dir {
            is_ignored_dir(&rel, snap)
        } else {
            is_ignored(&rel, false, snap)
        };
        if f.dir {
            continue;
        }
        f.git = git_state_of(&rel, snap);
    }
}

/// A cwd-relative tree path in the snapshot's toplevel-relative terms: prepend
/// the workspace's `offset` (a no-op for a toplevel workspace).
fn workdir_rel(offset: &str, cwd_rel: &str) -> String {
    if offset.is_empty() {
        cwd_rel.to_string()
    } else {
        format!("{offset}/{cwd_rel}")
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

/// Insert every proper ancestor-directory prefix of a file `path` into `into`
/// (`a/b/c` yields `a` and `a/b`; a top-level file yields none).
fn add_dir_prefixes(path: &str, into: &mut HashSet<String>) {
    let segments: Vec<&str> = path.split('/').collect();
    let mut prefix = String::new();
    for seg in &segments[..segments.len().saturating_sub(1)] {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(seg);
        into.insert(prefix.clone());
    }
}

/// A dir is greyed when it has no tracked and no shown-untracked content, i.e.
/// everything under it is ignored (so `.pi` and `target/` grey, `crates/` does not).
fn is_ignored_dir(path: &str, snap: &GitSnapshot) -> bool {
    !snap.tracked_dirs.contains(path) && !snap.shown_dirs.contains(path)
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
    let (added, deleted) = snap.numstat.get(path).copied().unwrap_or((None, None));
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
            tracked_dirs: HashSet::new(),
            shown_dirs: HashSet::new(),
            offset: String::new(),
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
    fn a_dir_whose_only_content_is_ignored_is_greyed() {
        // `.pi` is not itself ignored, but it holds no tracked and no
        // shown-untracked file - its sole child `.pi/skills` is ignored - so it
        // is greyed like its child, while `crates` (tracked content) is not.
        let snap = GitSnapshot {
            status: GitStatus {
                branch: "main".into(),
                modified: vec![],
                untracked: vec![],
                deleted: vec![],
            },
            numstat: HashMap::new(),
            ignore: ignore_search(&[".pi/skills/"]),
            tracked_dirs: {
                let mut s = HashSet::new();
                s.insert("crates".to_owned());
                s
            },
            shown_dirs: HashSet::new(),
            offset: String::new(),
        };
        let mut files = vec![
            entry("pi", ".pi", true),
            entry("skills", ".pi/skills", true),
            entry("crates", "crates", true),
        ];
        annotate(&mut files, Some(&snap));
        assert!(
            files[0].ignored,
            ".pi is greyed: its only content is ignored"
        );
        assert!(files[1].ignored, ".pi/skills is greyed: directly ignored");
        assert!(
            !files[2].ignored,
            "crates is not greyed: it has tracked content"
        );
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
            tracked_dirs: HashSet::new(),
            shown_dirs: HashSet::new(),
            offset: String::new(),
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
