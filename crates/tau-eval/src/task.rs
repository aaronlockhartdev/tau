//! The task format (ticket #57, first slice): one directory per task with
//! `task.toml` (metadata), `instruction.md` (the prompt), `setup.sh` (optional
//! workspace seed), `check.sh` (the score), and `oracle/solve.sh` (the
//! known-good solution the rig uses to confirm the task is solvable).

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::EvalError;

/// The task's tier: `smoke` rides the PR-gated nextest job, `full` is
/// on-demand only (ticket #57).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Smoke,
    Full,
}

impl Tier {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Smoke => "smoke",
            Tier::Full => "full",
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One task's `task.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFile {
    pub id: String,
    pub tier: Tier,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
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
}

impl Task {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.file.id
    }

    #[must_use]
    pub fn tier(&self) -> Tier {
        self.file.tier
    }

    #[must_use]
    pub fn timeout_secs(&self) -> u64 {
        self.file.timeout_secs
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

/// Load every task under `dir` (one subdirectory each, sorted by id).
/// A subdirectory without a `task.toml` is not a task and is skipped.
pub fn load(dir: &Path) -> Result<Vec<Task>, EvalError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(EvalError::io)? {
        let entry = entry.map_err(EvalError::io)?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("task.toml");
        if !manifest.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&manifest).map_err(EvalError::io)?;
        let file: TaskFile = toml::from_str(&raw)
            .map_err(|e| EvalError::Task(format!("{}: {e}", manifest.display())))?;
        let instruction = std::fs::read_to_string(path.join("instruction.md"))
            .map_err(|e| EvalError::Task(format!("{}: instruction.md: {e}", path.display())))?;
        out.push(Task {
            file,
            dir: path,
            instruction,
        });
    }
    out.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(out)
}
