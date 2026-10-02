//! The task quality gates (ticket #57, Harbor's oracle/nop gates): a task is
//! only admitted if its `oracle/solve.sh` makes `check.sh` pass AND a no-op
//! (setup only, no agent) makes it fail. Run over a temp dir with no agent
//! and no mock — pure script orchestration.

use crate::EvalError;
use crate::runner::{run_script, run_script_status};
use crate::task::Task;

/// Seed a fresh temp workspace; with `oracle` also run the known-good
/// solution. The workspace is returned so the caller scores it.
async fn seeded(task: &Task, oracle: bool) -> Result<tempfile::TempDir, EvalError> {
    let ws = tempfile::tempdir().map_err(EvalError::io)?;
    if let Some(setup) = task.setup() {
        run_script(&setup, ws.path()).await?;
    }
    if oracle {
        run_script(&task.oracle(), ws.path()).await?;
    }
    Ok(ws)
}

/// The oracle gate: `setup.sh` + `oracle/solve.sh` must make `check.sh` pass.
pub async fn oracle_passes(task: &Task) -> Result<bool, EvalError> {
    let ws = seeded(task, true).await?;
    let check = run_script_status(&task.check(), ws.path()).await?;
    Ok(check.code == 0)
}

/// The nop gate: `setup.sh` alone (no agent) must make `check.sh` fail.
pub async fn nop_fails(task: &Task) -> Result<bool, EvalError> {
    let ws = seeded(task, false).await?;
    let check = run_script_status(&task.check(), ws.path()).await?;
    Ok(check.code != 0)
}
