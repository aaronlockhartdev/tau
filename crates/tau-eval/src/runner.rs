//! The runner: `check.sh` / setup / oracle script execution, session
//! trajectory reading and per-trial artifact capture (session JSONL + tokens
//! + wall time + `results.json`), and the resumable checkpoint.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

use serde::{Deserialize, Serialize};
use tau_core::session::SessionStore;

use crate::EvalError;
use crate::task::Task;

/// A trial's disposition: `pass` (check exit 0), `fail` (check non-zero),
/// or `error` (the agent never completed — a harness/transient failure,
/// excluded from pass-rate denominators).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Error,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Error => "error",
        }
    }
}

/// Per-trial token accounting, summed from the session's assistant entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

/// One trial's outcome and accounting, the unit of the run summary.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub task: String,
    pub rep: u32,
    pub status: Status,
    pub score: bool,
    pub wall_ms: u64,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub turns: u32,
    pub tool_calls: Vec<String>,
    pub note: String,
}

pub(crate) fn error_outcome(task: &Task, rep: u32, e: &EvalError) -> Outcome {
    Outcome {
        task: task.id().to_owned(),
        rep,
        status: Status::Error,
        score: false,
        wall_ms: 0,
        tokens: Tokens::default(),
        cost_usd: 0.0,
        turns: 0,
        tool_calls: Vec::new(),
        note: e.to_string(),
    }
}

pub(crate) struct ScriptOut {
    pub(crate) code: i32,
    pub(crate) output: String,
}

/// Run `sh <script>` with `cwd`, capturing the exit code and combined output
/// (a non-zero exit is a value here, not an error — `check.sh` and the gates
/// read it). A child past `timeout` is killed (code `-2`; a signal death is
/// `-1`), so a hanging script can never stall a suite. `pub(crate)` so the
/// gates share the one script runner.
pub(crate) async fn run_script_status(
    script: &Path,
    cwd: &Path,
    timeout: Duration,
) -> Result<ScriptOut, EvalError> {
    let mut child = tokio::process::Command::new("sh")
        .arg(script)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(EvalError::io)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| EvalError::Live("stdout pipe missing".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| EvalError::Live("stderr pipe missing".into()))?;
    // The reader outlives the timeout, so on Elapsed we can still collect
    // what the script printed before it was killed.
    let reader = tokio::spawn(async move {
        let mut so = Vec::new();
        let mut se = Vec::new();
        let (rso, rse) = tokio::join!(stdout.read_to_end(&mut so), stderr.read_to_end(&mut se),);
        (so, se, rso, rse)
    });
    let res = tokio::time::timeout(timeout, child.wait()).await;
    let status = if let Ok(res) = res {
        res.map_err(EvalError::io)?
    } else {
        let _ = child.kill().await;
        let (so, se) = drain_reader(reader).await?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&so),
            String::from_utf8_lossy(&se)
        );
        let partial = tail(&combined, 64_000);
        return Ok(ScriptOut {
            code: -2,
            output: format!(
                "script timed out after {}s\n--- partial output (tail) ---\n{partial}",
                timeout.as_secs()
            ),
        });
    };
    let (so, se) = drain_reader(reader).await?;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&so),
        String::from_utf8_lossy(&se)
    );
    Ok(ScriptOut {
        code: status.code().unwrap_or(-1),
        output,
    })
}

/// The in-flight script output reader: captured (stdout, stderr) bytes
/// plus each pipe's read result.
type ScriptReader = tokio::task::JoinHandle<(
    Vec<u8>,
    Vec<u8>,
    Result<usize, std::io::Error>,
    Result<usize, std::io::Error>,
)>;

/// Unwrap the reader task's result: a join failure or a pipe read failure
/// is an `EvalError`; success is the captured (stdout, stderr) bytes.
async fn drain_reader(reader: ScriptReader) -> Result<(Vec<u8>, Vec<u8>), EvalError> {
    let (so, se, rso, rse) = reader
        .await
        .map_err(|e| EvalError::Live(format!("script reader task: {e}")))?;
    rso.map_err(EvalError::io)?;
    rse.map_err(EvalError::io)?;
    Ok((so, se))
}

/// The tail of a string, char-boundary safe: the diagnostic value of a
/// timed-out script's output is where it stopped.
fn tail(s: &str, max_chars: usize) -> &str {
    let n = s.chars().count();
    if n <= max_chars {
        return s;
    }
    let skip = n - max_chars;
    let i = s.char_indices().nth(skip).map_or(s.len(), |(i, _)| i);
    s.get(i..).unwrap_or(s)
}

/// Run a script that must succeed (setup / oracle); a non-zero exit is an
/// error carrying the script's own output.
pub(crate) async fn run_script(
    script: &Path,
    cwd: &Path,
    timeout: Duration,
) -> Result<(), EvalError> {
    let out = run_script_status(script, cwd, timeout).await?;
    if out.code != 0 {
        return Err(EvalError::Script {
            script: script.display().to_string(),
            code: out.code,
            output: out.output,
        });
    }
    Ok(())
}

/// Read the trial's session back: the turn count, the dispatched tool
/// sequence, and the summed usage.
pub(crate) fn read_session(cwd: &Path, id: &str) -> Result<(u32, Vec<String>, Tokens), EvalError> {
    let store = SessionStore::for_workspace(cwd, id);
    let entries = store
        .entries_range(0, usize::MAX)
        .map_err(EvalError::Session)?;
    let mut turns = 0u32;
    let mut tool_calls = Vec::new();
    let mut tokens = Tokens::default();
    for e in &entries {
        match e.kind.as_str() {
            tau_core::agent::KIND_ASSISTANT => {
                turns += 1;
                if let Some(u) = e.payload.get("usage") {
                    tokens.input += u
                        .get("input_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    tokens.output += u
                        .get("output_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    tokens.total += u
                        .get("total_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                }
            }
            tau_core::agent::KIND_TOOL => {
                if let Some(name) = e.payload.get("name").and_then(serde_json::Value::as_str) {
                    tool_calls.push(name.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok((turns, tool_calls, tokens))
}

/// Copy the trial's session file (the trajectory) into the artifact dir.
pub(crate) fn copy_session(cwd: &Path, id: &str, dest: &Path) -> std::io::Result<()> {
    let src = cwd
        .join(".tau")
        .join("sessions")
        .join(format!("{id}.jsonl"));
    std::fs::copy(&src, dest).map(|_| ())
}

/// The checkpoint/artifact key for one trial: bare id at rep 0,
/// `id#rep` beyond.
pub(crate) fn trial_key(id: &str, rep: u32) -> String {
    if rep == 0 {
        id.to_owned()
    } else {
        format!("{id}#{rep}")
    }
}

/// The trial's artifact dir: `artifacts/<id>` at rep 0, `artifacts/<id>/rep-<n>`
/// beyond.
pub(crate) fn trial_dir(artifacts: &Path, id: &str, rep: u32) -> PathBuf {
    if rep == 0 {
        artifacts.join(id)
    } else {
        artifacts.join(id).join(format!("rep-{rep}"))
    }
}

/// The run-level resumable checkpoint: the completed task ids and their
/// status, so an interrupted batch re-run skips what already finished.
#[derive(Default, Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub completed: BTreeMap<String, String>,
}

impl Checkpoint {
    pub fn load(path: &Path) -> Result<Self, EvalError> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(EvalError::io)?;
        serde_json::from_str(&raw).map_err(|e| EvalError::Task(format!("checkpoint: {e}")))
    }

    /// Record one finished trial and persist (a crash-safe resume point).
    pub(crate) fn record(&mut self, outcome: &Outcome, path: &Path) {
        self.completed.insert(
            trial_key(&outcome.task, outcome.rep),
            outcome.status.as_str().to_owned(),
        );
        let json = serde_json::to_vec(self).expect("a checkpoint is always serializable");
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn script_exit_code_is_captured() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("s.sh");
        std::fs::write(&script, "exit 3\n").unwrap();
        let out = run_script_status(&script, dir.path(), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(out.code, 3);
    }

    #[tokio::test]
    async fn script_timeout_reports_sentinel_code() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("s.sh");
        std::fs::write(&script, "echo MARKER-PRINTED; sleep 10\n").unwrap();
        let out = run_script_status(&script, dir.path(), Duration::from_millis(200))
            .await
            .unwrap();
        assert_eq!(out.code, -2);
        // the bytes the script printed before the kill are retained
        assert!(out.output.contains("MARKER-PRINTED"));
        assert!(out.output.contains("partial output"));
    }
}
