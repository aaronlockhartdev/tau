//! Workspace git status and one-file diffs (workbench P4): thin shell-outs
//! to the git CLI. The core keeps no git library dependency (ADR-0002); the
//! files tab badges and the peek sheet need exactly what
//! `git status --porcelain=v2` and `git diff` produce.

use std::path::Path;
use std::process::Command as Process;

use tau_protocol::{GitDiff, GitStatus, ProtocolError};

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
}
