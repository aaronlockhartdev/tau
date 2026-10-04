//! The task format (ticket #57, first slice): one directory per task with
//! `task.toml` (metadata), `instruction.md` (the prompt), `setup.sh` (optional
//! workspace seed), `check.sh` (the score), and `oracle/solve.sh` (the
//! known-good solution the rig uses to confirm the task is solvable).

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::EvalError;

/// One task's `task.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFile {
    pub id: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// The container environment: the image the trial runs in.
    #[serde(default)]
    pub docker: Option<String>,
}

fn default_timeout() -> u64 {
    120
}

/// A loaded task: its metadata plus the files the runner executes.
#[derive(Debug, Clone)]
pub struct Task {
    pub file: TaskFile,
    pub dir: PathBuf,
    pub instruction: String,
    /// TB-native: the check is the Terminal-Bench protocol (tests at /tests,
    /// verdict at /logs/verifier/reward.txt) rather than a task check.sh.
    pub tb_native: bool,
}

impl Task {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.file.id
    }

    #[must_use]
    pub fn timeout_secs(&self) -> u64 {
        self.file.timeout_secs
    }

    /// The docker image of a container-only environment; `None` = the task
    /// runs straight in the workspace.
    #[must_use]
    pub fn docker(&self) -> Option<&str> {
        self.file.docker.as_deref()
    }

    /// The workspace seed script; absent for trivial tasks.
    #[must_use]
    pub fn setup(&self) -> Option<PathBuf> {
        let p = self.dir.join("setup.sh");
        p.is_file().then_some(p)
    }

    /// The verifier (the score); required.
    #[must_use]
    pub fn check(&self) -> PathBuf {
        self.dir.join("check.sh")
    }

    /// The known-good solution (the oracle gate); required for the gate.
    #[must_use]
    pub fn oracle(&self) -> PathBuf {
        self.dir.join("oracle").join("solve.sh")
    }
}

/// Load one task from `dir` (which must contain a `task.toml`).
pub fn load_one(dir: &Path) -> Result<Task, EvalError> {
    let manifest = dir.join("task.toml");
    let raw = std::fs::read_to_string(&manifest).map_err(EvalError::io)?;
    let file: TaskFile = toml::from_str(&raw)
        .map_err(|e| EvalError::Task(format!("{}: {e}", manifest.display())))?;
    let instruction = std::fs::read_to_string(dir.join("instruction.md"))
        .map_err(|e| EvalError::Task(format!("{}: instruction.md: {e}", dir.display())))?;
    Ok(Task {
        file,
        dir: dir.to_path_buf(),
        instruction,
        tb_native: false,
    })
}

/// Build a task from a Terminal-Bench-native directory: the upstream
/// `instruction.md` and the TB check protocol in place of a task `check.sh`
/// (`docker_image` is what the upstream manifest names; `None` in-container,
/// where the image is already the ground truth).
pub fn from_tb(
    dir: &Path,
    id: &str,
    timeout_secs: u64,
    docker_image: Option<String>,
) -> Result<Task, EvalError> {
    let instruction = std::fs::read_to_string(dir.join("instruction.md"))
        .map_err(|e| EvalError::Task(format!("{}: instruction.md: {e}", dir.display())))?;
    Ok(Task {
        file: TaskFile {
            id: id.to_owned(),
            timeout_secs,
            docker: docker_image,
        },
        dir: dir.to_path_buf(),
        instruction,
        tb_native: true,
    })
}
/// Load every task under `dir`: a subdirectory with a `task.toml` is a
/// task; a subdirectory without one is a *set* whose subdirectories are
/// scanned (the `tb/` import namespace). A set marked `QUARANTINE.md` is
/// excluded. Sorted by id.
pub fn load(dir: &Path) -> Result<Vec<Task>, EvalError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(EvalError::io)? {
        let path = entry.map_err(EvalError::io)?.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("task.toml").is_file() {
            out.push(load_one(&path)?);
            continue;
        }
        if path.join("QUARANTINE.md").is_file() {
            continue;
        }
        for sub in std::fs::read_dir(&path).map_err(EvalError::io)? {
            let sub = sub.map_err(EvalError::io)?.path();
            if sub.is_dir() && sub.join("task.toml").is_file() {
                out.push(load_one(&sub)?);
            }
        }
    }
    out.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_manifest_is_a_task_error() {
        let dir = tempfile::tempdir().unwrap();
        let task_dir = dir.path().join("broken");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("task.toml"), "id = [unterminated\n").unwrap();
        let err = load(dir.path()).unwrap_err();
        assert!(matches!(err, EvalError::Task(_)));
    }
}
