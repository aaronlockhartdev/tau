//! The runner: an in-process mock, a temp-dir workspace per trial, the
//! in-process `tau-core` drive, `check.sh` scoring, and per-trial artifact
//! capture (session JSONL + tokens + wall time + `results.json`), with a
//! bounded tokio pool and a resumable checkpoint (ticket #57, §2.2).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tau_core::agent::{AgentSession, Lane, TurnConfig};
use tau_core::config::{Provider, Requests, ToolBatchPolicy};
use tau_core::harness::SessionRole;
use tau_core::provider;
use tau_core::session::SessionStore;
use tau_core::tools;
use tau_mock_llm::{scenario::ScenarioSet, server, server::MODEL_ID};
use tokio::sync::{Mutex, Semaphore};

use crate::EvalError;
use crate::task::Task;

/// The system prompt every trial's session is launched with. It deliberately
/// names no scenario `match` pattern, so the mock routes each request by the
/// task's instruction (the user message), not the prompt.
const SYSTEM_PROMPT: &str = "You have the tools read, write, edit, and bash. \
Use them as instructed; edit takes the 3-char anchors from read output.";

/// Cap every generation (the acceptance driver's cap); the mock's script is
/// short, so this only bounds a runaway.
const OUTPUT_CAP: u64 = 300;

/// A mock LLM serving `scenarios_dir` on an ephemeral loopback port.
pub struct MockHandle {
    pub base_url: String,
}

/// Start the deterministic mock in-process (no subprocess, no fixed port).
pub async fn start_mock(scenarios_dir: &Path) -> Result<MockHandle, EvalError> {
    let set = Arc::new(ScenarioSet::load_dir(scenarios_dir).map_err(EvalError::Mock)?);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(EvalError::io)?;
    let addr = listener
        .local_addr()
        .expect("a bound listener has a local addr");
    tokio::spawn(server::serve(listener, set));
    Ok(MockHandle {
        base_url: format!("http://{addr}/v1"),
    })
}

/// A trial's disposition: `pass` (check exit 0), `fail` (check non-zero),
/// or `error` (the agent never completed — a harness/transient failure,
/// excluded from pass-rate denominators).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

/// One trial's outcome and accounting, the unit of the run summary.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub task: String,
    pub tier: String,
    pub status: Status,
    pub score: bool,
    pub wall_ms: u64,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub turns: u32,
    pub tool_calls: Vec<String>,
    pub note: String,
}

fn error_outcome(task: &Task, e: &EvalError) -> Outcome {
    Outcome {
        task: task.id().to_owned(),
        tier: task.tier().as_str().to_owned(),
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

/// A no-pool client: a pooled keep-alive connection would keep the runtime
/// alive past the last call and hang process exit (the #23 teardown lesson).
fn client() -> Result<reqwest::Client, EvalError> {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .map_err(EvalError::Http)
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
    let child_out = tokio::process::Command::new("sh")
        .arg(script)
        .current_dir(cwd)
        .output();
    let out = match tokio::time::timeout(timeout, child_out).await {
        Ok(res) => res.map_err(EvalError::io)?,
        // Elapsed: dropping the in-flight future kills the child.
        Err(_) => {
            return Ok(ScriptOut {
                code: -2,
                output: format!("script timed out after {}s", timeout.as_secs()),
            });
        }
    };
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(ScriptOut {
        code: out.status.code().unwrap_or(-1),
        output,
    })
}

/// Run a script that must succeed (setup / oracle); a non-zero exit is an
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

/// One trial: seed a temp-dir workspace, drive the core against the mock,
/// score with `check.sh`, and capture the per-trial artifacts.
pub async fn run_trial(
    task: &Task,
    base_url: &str,
    artifacts_dir: &Path,
) -> Result<Outcome, EvalError> {
    let start = Instant::now();
    let ws = tempfile::tempdir().map_err(EvalError::io)?;
    let cwd = ws.path();

    if let Some(setup) = task.setup() {
        run_script(&setup, cwd, Duration::from_secs(task.timeout_secs())).await?;
    }

    let id = SessionStore::new_session_id();
    let mut store = SessionStore::for_workspace(cwd, &id);
    store.create().map_err(EvalError::Session)?;

    let client = client()?;
    let provider = Provider::with_model(base_url.to_owned(), MODEL_ID);
    let agent = AgentSession::launch(
        store,
        SessionRole::Bare {
            provider: provider::production(&client, &provider, &Requests::default()),
            system_prompt: SYSTEM_PROMPT.to_owned(),
            model: MODEL_ID.to_owned(),
            tools: tools::tool_specs(),
            cwd: cwd.to_path_buf(),
            turn: TurnConfig {
                max_output_tokens: Some(OUTPUT_CAP),
                ..TurnConfig::default()
            },
            tool_batch_on_force: ToolBatchPolicy::default(),
            om: None,
            om_model: String::new(),
        },
    )
    .map_err(|e| EvalError::Launch(format!("{e:?}")))?;

    agent.send(task.instruction.trim_end(), Lane::Steering);
    // Distinguish a timeout from a genuine `AgentError`: both end the drive,
    // but only the first earns the "timed out" diagnosis.
    let (timed_out, drive_note) =
        match tokio::time::timeout(Duration::from_secs(task.timeout_secs()), agent.process()).await
        {
            Ok(Ok(())) => (false, None),
            Ok(Err(e)) => (false, Some(e.to_string())),
            Err(_) => (true, None),
        };
    drop(agent);

    // The session file is the trajectory; reading it back verifies every
    // line's CRC (the session-integrity invariant).
    let (turns, tool_calls, tokens) = read_session(cwd, &id)?;

    let check =
        run_script_status(&task.check(), cwd, Duration::from_secs(task.timeout_secs())).await?;
    let passed = check.code == 0;
    let status = if timed_out {
        Status::Error
    } else if passed {
        Status::Pass
    } else {
        Status::Fail
    };
    let note = match status {
        Status::Pass => String::new(),
        Status::Fail => check.output,
        Status::Error => drive_note.unwrap_or_else(|| "agent turn timed out".to_owned()),
    };

    // Per-trial artifacts: the trajectory plus the record.
    let trial_dir = artifacts_dir.join(task.id());
    std::fs::create_dir_all(&trial_dir).map_err(EvalError::io)?;
    copy_session(cwd, &id, &trial_dir.join("session.jsonl")).map_err(EvalError::io)?;

    let outcome = Outcome {
        task: task.id().to_owned(),
        tier: task.tier().as_str().to_owned(),
        status,
        score: passed,
        wall_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        tokens,
        cost_usd: 0.0, // the mock carries no pricing; the live leg fills this
        turns,
        tool_calls,
        note,
    };
    crate::report::write_results(&trial_dir, &outcome).map_err(EvalError::io)?;
    Ok(outcome)
}

/// Read the trial's session back: the turn count, the dispatched tool
/// sequence, and the summed usage.
fn read_session(cwd: &Path, id: &str) -> Result<(u32, Vec<String>, Tokens), EvalError> {
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
fn copy_session(cwd: &Path, id: &str, dest: &Path) -> std::io::Result<()> {
    let src = cwd
        .join(".tau")
        .join("sessions")
        .join(format!("{id}.jsonl"));
    std::fs::copy(&src, dest).map(|_| ())
}

/// Run `tasks` with bounded concurrency, writing a per-trial `results.json`
/// and a run-level resumable checkpoint. Tasks already in the checkpoint
/// (from an interrupted prior run) are skipped.
pub async fn run_suite(
    tasks: &[Task],
    base_url: &str,
    artifacts_dir: &Path,
    concurrency: usize,
) -> Result<Vec<Outcome>, EvalError> {
    std::fs::create_dir_all(artifacts_dir).map_err(EvalError::io)?;
    let checkpoint_path = artifacts_dir.join("checkpoint.json");
    let ckpt = Arc::new(Mutex::new(Checkpoint::load(&checkpoint_path)?));

    let done: HashSet<String> = ckpt.lock().await.completed.keys().cloned().collect();
    let due: Vec<&Task> = tasks.iter().filter(|t| !done.contains(t.id())).collect();

    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut handles = Vec::new();
    for task in due {
        let sem = sem.clone();
        let ckpt = ckpt.clone();
        let base_url = base_url.to_owned();
        let artifacts_dir = artifacts_dir.to_path_buf();
        let checkpoint_path = checkpoint_path.clone();
        let task = task.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem
                .acquire_owned()
                .await
                .expect("the semaphore is never closed");
            let outcome = match run_trial(&task, &base_url, &artifacts_dir).await {
                Ok(o) => o,
                Err(e) => error_outcome(&task, &e),
            };
            ckpt.lock().await.record(&outcome, &checkpoint_path);
            outcome
        }));
    }

    let mut outcomes = Vec::new();
    for handle in handles {
        // run_trial returns a Result; a join error means a panic, which the
        // driver does not do.
        outcomes.push(handle.await.expect("a trial task does not panic"));
    }
    outcomes.sort_by(|a, b| a.task.cmp(&b.task));
    Ok(outcomes)
}

/// The run-level resumable checkpoint: the completed task ids and their
/// status, so an interrupted batch re-run skips what already finished.
#[derive(Default, Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub completed: BTreeMap<String, String>,
}

impl Checkpoint {
    fn load(path: &Path) -> Result<Self, EvalError> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(EvalError::io)?;
        serde_json::from_str(&raw).map_err(|e| EvalError::Task(format!("checkpoint: {e}")))
    }

    /// Record one finished trial and persist (a crash-safe resume point).
    fn record(&mut self, outcome: &Outcome, path: &Path) {
        self.completed
            .insert(outcome.task.clone(), outcome.status.as_str().to_owned());
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
        std::fs::write(&script, "sleep 10\n").unwrap();
        let out = run_script_status(&script, dir.path(), Duration::from_millis(200))
            .await
            .unwrap();
        assert_eq!(out.code, -2);
    }
}
