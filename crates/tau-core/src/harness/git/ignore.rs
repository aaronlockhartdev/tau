use std::collections::HashSet;
use std::path::Path;

use tau_protocol::ProtocolError;

use super::GitSnapshot;

/// Bases are workdir-rooted (toplevel-relative); `annotate` translates each
/// cwd-relative tree path into those terms before checking, so a subdir
/// workspace matches correctly and a sibling subtree's pattern never leaks in.
pub(super) fn build_ignore_search(
    repo: &gix::Repository,
) -> Result<gix::ignore::Search, ProtocolError> {
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
/// A path is ignored when it, or any ancestor directory, matches a loaded
/// ignore pattern — the rule that greys an empty ignored dir (`.tau/skills`)
/// just like its ignored parent (`.tau/`).
pub(super) fn is_ignored(path: &str, is_dir: bool, snap: &GitSnapshot) -> bool {
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
pub(super) fn add_dir_prefixes(path: &str, into: &mut HashSet<String>) {
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
pub(super) fn is_ignored_dir(path: &str, snap: &GitSnapshot) -> bool {
    !snap.tracked_dirs.contains(path) && !snap.shown_dirs.contains(path)
}
