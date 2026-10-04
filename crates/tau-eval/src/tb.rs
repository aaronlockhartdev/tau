//! Terminal-Bench 2.0 at runtime (ticket #65): the dataset is fetched, not
//! vendored — a shallow clone of the upstream repo into `target/terminal-bench`
//! (skipped when present), with the curation — which tasks we benchmark and
//! which we exclude and why — in `eval/tb-selection.toml` in this repo.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::EvalError;
use crate::task;

/// `eval/tb-selection.toml`: the curation layer over the upstream dataset.
#[derive(Debug, Deserialize)]
pub struct Selection {
    pub repo: String,
    pub tasks: Vec<String>,
    #[serde(default)]
    pub quarantine: Vec<Quarantine>,
}

/// An excluded task with the reason on record.
#[derive(Debug, Deserialize)]
pub struct Quarantine {
    pub name: String,
    pub reason: String,
}

/// Shallow-clone the dataset into `root`; a no-op when already present.
pub async fn ensure_checkout(root: &Path, repo: &str) -> Result<PathBuf, EvalError> {
    if root.join(".git").exists() {
        return Ok(root.to_path_buf());
    }
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent).map_err(EvalError::io)?;
    }
    eprintln!(
        "tau-eval: first use — cloning {repo} into {} (a few hundred MB)",
        root.display()
    );
    let out = tokio::process::Command::new("git")
        .args(["clone", "--depth", "1", repo, root.to_str().unwrap_or("")])
        .output()
        .await
        .map_err(|e| EvalError::Live(format!("git clone {repo}: {e}")))?;
    if !out.status.success() {
        return Err(EvalError::Live(format!(
            "git clone {repo} exited {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(root.to_path_buf())
}

/// The upstream task manifest, read leniently: only the fields the rig needs.
#[derive(Deserialize)]
struct TbManifest {
    #[serde(default)]
    agent: TbAgent,
    #[serde(default)]
    environment: TbEnvironment,
}

#[derive(Deserialize, Default)]
struct TbAgent {
    #[serde(default)]
    timeout_sec: f64,
}

#[derive(Deserialize, Default)]
struct TbEnvironment {
    #[serde(default)]
    docker_image: Option<String>,
}

/// Load the selected tasks from a dataset checkout, in selection order.
pub fn load(checkout: &Path, sel: &Selection) -> Result<Vec<task::Task>, EvalError> {
    let mut out = Vec::new();
    for name in &sel.tasks {
        let dir = checkout.join(name);
        let manifest = dir.join("task.toml");
        let raw = std::fs::read_to_string(&manifest).map_err(|e| {
            EvalError::Task(format!(
                "{}: {e} (is the dataset checkout complete?)",
                manifest.display()
            ))
        })?;
        let m: TbManifest = toml::from_str(&raw)
            .map_err(|e| EvalError::Task(format!("{}: {e}", manifest.display())))?;
        let image = m
            .environment
            .docker_image
            .ok_or_else(|| EvalError::Task(format!("{name}: no [environment].docker_image")))?;
        // The upstream default when a task omits the agent budget.
        let timeout = if m.agent.timeout_sec > 0.0 {
            // Upstream budgets are whole seconds (e.g. 900.0); the guard
            // above makes the truncating cast safe.
            #[allow(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let secs = m.agent.timeout_sec as u64;
            secs
        } else {
            900
        };
        out.push(task::from_tb(&dir, name, timeout, Some(image))?);
    }
    Ok(out)
}
