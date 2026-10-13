//! Workspace git status and one-file diffs (workbench P4): thin shell-outs
//! to the git CLI. The core keeps no git library dependency (ADR-0002); the
//! files tab badges and the peek sheet need exactly what
//! `git status --porcelain=v2` and `git diff` produce.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::Path;
use std::process::Command as Process;

use tau_protocol::{FileEntry, FileGit, FileGitState, GitDiff, GitStatus, ProtocolError};

/// Parse `git status --porcelain=v2` output into the GUI's badge model.
/// Entry lines read `1 <XY> <sub> <m> <sm> <mm> <X> <Y> <path>`; untracked
/// entries read `? <path>`; a rename's live name sits on its paired `2` line.
pub fn parse_status(output: &str) -> GitStatus {
    let lines: Vec<&str> = output.lines().collect();
    let mut status = GitStatus {
        branch: String::new(),
        modified: Vec::new(),
        untracked: Vec::new(),
        deleted: Vec::new(),
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if let Some(head) = line.strip_prefix("# branch.head ") {
            status.branch = head.to_string();
        } else if let Some(path) = line.strip_prefix("? ") {
            status.untracked.push(unquote(path));
        } else if let Some(rest) = line.strip_prefix("1 ") {
            // A rename's `1` line names the old file; the paired `2` line
            // names the live one, which is what the badge should show.
            let path = if rest.starts_with('R') {
                lines
                    .get(i)
                    .and_then(|l| l.strip_prefix("2 "))
                    .and_then(path_of)
                    .or_else(|| path_of(rest))
            } else {
                path_of(rest)
            };
            let xy = rest.get(..2).unwrap_or("");
            match path {
                Some(p) if xy.contains('D') => status.deleted.push(p),
                Some(p) => status.modified.push(p),
                None => {}
            }
        }
    }
    status
}

/// The last whitespace field of a v2 entry body is the path (C-quoted when
/// it holds special characters; v0 unquotes the trivial case only).
fn path_of(body: &str) -> Option<String> {
    body.split_whitespace().last().map(unquote)
}

fn unquote(path: &str) -> String {
    path.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .map_or(path.to_string(), str::to_string)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, ProtocolError> {
    let out = Process::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| ProtocolError::Other {
            message: format!("git: {e}"),
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(ProtocolError::Other {
            message: format!("git {}: {}", args.join(" "), stderr.trim()),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn git_status(cwd: &Path) -> Result<GitStatus, ProtocolError> {
    let out = run_git(cwd, &["status", "--porcelain=v2"])?;
    let mut status = parse_status(&out);
    if status.branch.is_empty() {
        // This git build omits the v2 header lines; fall back to rev-parse.
        status.branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();
    }
    Ok(status)
}

/// Unified diff of one file vs HEAD (staged + unstaged). The returned path
/// is workspace-relative when the file lives under the workspace root.
pub fn git_diff(cwd: &Path, file: &Path) -> Result<GitDiff, ProtocolError> {
    let out = run_git(cwd, &["diff", "HEAD", "--", &file.display().to_string()])?;
    let path = file
        .strip_prefix(cwd)
        .ok()
        .and_then(|p| p.to_str())
        .map_or(file.display().to_string(), str::to_string);
    Ok(GitDiff { path, diff: out })
}

/// A workspace's combined git state (file-tree plan, phase 2): which files
/// changed (status), by how much (numstat), and which paths are .gitignored
/// (the ignore set). Three spawns total, cached per workspace and
/// invalidated on `file_tree_changed`; every dir listing in the workspace
/// annotates from it.
/// The numstat map's value: `(added, deleted)`; a `None` count is
/// unmeasurable (binary).
pub type NumstatCounts = (Option<u32>, Option<u32>);

#[derive(Clone)]
pub struct GitSnapshot {
    pub status: GitStatus,
    /// Changed path → (added, deleted); `None` count = unmeasurable
    /// (binary). Untracked files are measured here, at snapshot time, so
    /// neither the tree nor the changes tab does per-row file reads.
    pub numstat: HashMap<String, NumstatCounts>,
    pub ignored: HashSet<String>,
}

pub fn git_snapshot(cwd: &Path) -> Result<GitSnapshot, ProtocolError> {
    let status = git_status(cwd)?;
    let mut numstat = parse_numstat(&run_git(cwd, &["diff", "--numstat", "HEAD"])?)
        .into_iter()
        .collect::<HashMap<_, _>>();
    // `git diff` never measures untracked files: count their lines now,
    // once per snapshot, not per tree row.
    for path in &status.untracked {
        numstat.entry(path.clone()).or_insert_with(|| {
            (count_lines(&cwd.join(path)), Some(0))
        });
    }
    let ignored = ignored_set(cwd)?;
    Ok(GitSnapshot {
        status,
        numstat,
        ignored,
    })
}

/// Parse `git diff --numstat` output: `added<TAB>deleted<TAB>path` per
/// line; a binary file reads `-` for a count and carries none.
pub fn parse_numstat(output: &str) -> Vec<(String, NumstatCounts)> {
    let count = |s: &str| s.trim().parse::<u32>().ok();
    output
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            match (f.next(), f.next(), f.next()) {
                (Some(a), Some(d), Some(p)) if !p.trim().is_empty() => {
                    Some((unquote(p), (count(a), count(d))))
                }
                _ => None,
            }
        })
        .collect()
}

/// The .gitignored set (file-tree plan): what `ls-files --others` (every
/// untracked path) has that `--exclude-standard` (untracked minus ignored)
/// drops. A set difference — no per-file stat.
pub fn ignored_set(cwd: &Path) -> Result<HashSet<String>, ProtocolError> {
    let all = run_git(cwd, &["ls-files", "--others"])?;
    let shown = run_git(cwd, &["ls-files", "--others", "--exclude-standard"])?;
    let shown: HashSet<String> = shown.lines().map(str::to_owned).collect();
    Ok(all.lines()
        .map(str::to_owned)
        .filter(|p| !shown.contains(p))
        .collect())
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
/// (file-tree plan, phase 2). Directories carry no git state: a dir's
/// change status is its children's, derived by the GUI.
pub fn annotate(files: &mut [FileEntry], snap: Option<&GitSnapshot>) {
    let Some(snap) = snap else { return };
    for f in files.iter_mut().filter(|f| !f.dir) {
        f.ignored = snap.ignored.contains(&f.path);
        f.git = git_state_of(&f.path, snap);
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

    #[test]
    fn parses_branch_modifications_untracked_and_deletions() {
        let out = "# branch.head main\n\
                   1 .M   N...  100644 100644 100644 aaaa bbbb src/a.rs\n\
                   1 M.   N...  100644 100644 100644 aaaa bbbb src/b.rs\n\
                   ? docs/new.md\n\
                   1 .D   -----  -      aaaa cccc -      src/gone.rs\n";
        let s = parse_status(out);
        assert_eq!(s.branch, "main");
        assert_eq!(s.modified, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(s.untracked, vec!["docs/new.md"]);
        assert_eq!(s.deleted, vec!["src/gone.rs"]);
    }

    #[test]
    fn rename_badges_the_live_name_from_the_pair_line() {
        let out = "# branch.head dev\n\
                   1 R.   N...  100644 100644 100644 aaaa bbbb old/name.rs\n\
                   2 R.   N...  100644 100644 100644 aaaa bbbb new/name.rs\n";
        let s = parse_status(out);
        assert_eq!(s.modified, vec!["new/name.rs"]);
        assert!(s.deleted.is_empty());
    }

    #[test]
    fn empty_status_is_a_clean_state() {
        let s = parse_status("# branch.head main\n");
        assert_eq!(s.branch, "main");
        assert!(s.modified.is_empty() && s.untracked.is_empty() && s.deleted.is_empty());
    }

    #[test]
    fn parses_numstat_counts_and_binary_markers() {
        let out = "3\t1\tsrc/a.rs\n\t\t\n-\t-\tassets/logo.png\n0\t5\tdocs/gone.md\n";
        let v = parse_numstat(out);
        assert_eq!(v, vec![
            ("src/a.rs".into(), (Some(3), Some(1))),
            ("assets/logo.png".into(), (None, None)),
            ("docs/gone.md".into(), (Some(0), Some(5))),
        ]);
    }

    #[test]
    fn malformed_numstat_lines_are_skipped() {
        let v = parse_numstat("no-tabs-here\n1\t2\n");
        assert!(v.is_empty());
    }

    #[test]
    fn unquoted_numstat_paths_are_kept() {
        let v = parse_numstat("1\t1\t\"src/odd name.rs\"\n");
        assert_eq!(v, vec![("src/odd name.rs".into(), (Some(1), Some(1)))]);
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
            ignored: ["target/x".into()].into_iter().collect(),
        };
        let mut files = vec![
            FileEntry {
                name: "a.rs".into(),
                path: "src/a.rs".into(),
                dir: false,
                size: 1,
                ignored: false,
                git: None,
            },
            FileEntry {
                name: "new.md".into(),
                path: "new.md".into(),
                dir: false,
                size: 1,
                ignored: false,
                git: None,
            },
            FileEntry {
                name: "x".into(),
                path: "target/x".into(),
                dir: false,
                size: 1,
                ignored: false,
                git: None,
            },
        ];
        annotate(&mut files, Some(&snap));
        assert_eq!(files[0].git, Some(FileGit { state: FileGitState::Modified, added: Some(3), deleted: Some(1) }));
        assert_eq!(files[1].git, Some(FileGit { state: FileGitState::Untracked, added: None, deleted: None }));
        assert!(files[2].ignored && files[2].git.is_none());
    }

    #[test]
    fn annotate_is_a_noop_without_a_snapshot() {
        let mut files = vec![FileEntry {
            name: "a.rs".into(),
            path: "a.rs".into(),
            dir: false,
            size: 1,
            ignored: false,
            git: None,
        }];
        annotate(&mut files, None);
        assert!(!files[0].ignored && files[0].git.is_none());
    }
}
