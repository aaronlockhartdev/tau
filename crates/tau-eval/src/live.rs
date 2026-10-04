//! The live leg (ticket #65, phase 1): the user's real harness on a real
//! model. Each trial goes through the app's own construction path — a
//! production-shape `Core` built over a scratch copy of the user's config
//! dir, so the trial's workspace/session state stays out of the user's live
//! app state while config, provider, prompts, OM, and the supervisor are
//! exactly what the app would build. Turn completion is the app's own
//! signal: the post-turn `Queue` projection of the drained queue, after the
//! first `StreamEnd` (the protocol has no dedicated turn-done event).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tau_core::config::{self, Cost};
use tau_core::harness::{Core, CoreBuilder};
use tau_protocol::{Command, CommandOutput, Event, MessageLane, SystemEventKind};
use tokio::sync::{Mutex, Semaphore, mpsc};

use crate::EvalError;

/// The TB-native check: upstream `test.sh` always exits 0 and writes its
/// verdict to /logs/verifier/reward.txt, so the script fails on a test.sh
/// crash and otherwise on a reward file that is not "1" (tasks without a
/// reward file pass on test.sh's own exit status).
const TB_CHECK: &str = r#"#!/bin/sh
bash /tests/test.sh || exit 1
[ ! -f /logs/verifier/reward.txt ] || [ "$(cat /logs/verifier/reward.txt)" = "1" ]
"#;

use crate::runner::{self, Outcome, Status, run_script_status, trial_dir};
use crate::task::Task;
/// The user's system config dir (the production shape `CoreBuilder::default_system` uses).
fn user_system_dir() -> Result<PathBuf, EvalError> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        EvalError::Live("HOME is not set; cannot locate the user's tau config".into())
    })?;
    Ok(PathBuf::from(home).join(".config").join("tau"))
}

/// Copy the config surface (a few small files) into the scratch dir.
fn copy_dir(src: &Path, dst: &Path) -> Result<(), EvalError> {
    std::fs::create_dir_all(dst).map_err(EvalError::io)?;
    for entry in std::fs::read_dir(src).map_err(EvalError::io)? {
        let entry = entry.map_err(EvalError::io)?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(EvalError::io)?;
        }
    }
    Ok(())
}

/// The live leg: a scratch core, the event pump fanning out to per-session
/// subscribers, and the model the benchmark runs (the config's own default
/// resolution — the rig picks no model of its own).
pub struct Live {
    core: Arc<Core>,
    /// The scratch config dir; held for the process's life.
    scratch: tempfile::TempDir,
    model: String,
    /// Per-1M-token cost, per configured model (absent = unpriced).
    costs: BTreeMap<String, Cost>,
    subs: Arc<Mutex<BTreeMap<String, mpsc::UnboundedSender<Event>>>>,
}

/// Start the live leg: scratch-copy the user's config and build the
/// production-shape core over it.
pub fn start() -> Result<Live, EvalError> {
    let system_dir = user_system_dir()?;
    if !system_dir.is_dir() {
        return Err(EvalError::Live(format!(
            "no user config at {} — the live leg runs the user's real model configuration",
            system_dir.display()
        )));
    }
    let scratch = tempfile::tempdir().map_err(EvalError::io)?;
    copy_dir(&system_dir, scratch.path())?;

    let core = CoreBuilder::default_system()
        .with_system_dir(scratch.path().to_path_buf())
        .build();

    // The merged config, the same load the core uses: the cost table and
    // the default model both come from it.
    let cfg = config::load(scratch.path(), scratch.path())
        .map_err(|e| EvalError::Live(format!("config: {e}")))?;
    let (model, costs) = model_and_costs(&cfg);

    let subs = Arc::new(Mutex::new(BTreeMap::new()));
    {
        let subs = subs.clone();
        tokio::spawn(pump(core.events(), subs));
    }

    Ok(Live {
        core,
        scratch,
        model,
        costs,
        subs,
    })
}

/// The model a trial runs: the first provider's `generation.default_model`
/// when declared there, else the provider's first model — the same
/// resolution `root_session` applies.
fn model_and_costs(cfg: &config::Config) -> (String, BTreeMap<String, Cost>) {
    let mut costs = BTreeMap::new();
    for p in cfg.providers.values() {
        for (id, def) in &p.models {
            if let Some(c) = def.cost {
                costs.insert(id.clone(), c);
            }
        }
    }
    let model = cfg.providers.values().next().map_or_else(String::new, |p| {
        let d = &cfg.generation.default_model;
        if !d.is_empty() && p.models.contains_key(d) {
            d.clone()
        } else {
            p.models.keys().next().cloned().unwrap_or_default()
        }
    });
    (model, costs)
}

/// The one event pump: fan each event out to that session's subscribers.
async fn pump(
    mut rx: mpsc::Receiver<Event>,
    subs: Arc<Mutex<BTreeMap<String, mpsc::UnboundedSender<Event>>>>,
) {
    while let Some(ev) = rx.recv().await {
        let Some(sid) = session_of(&ev) else {
            continue;
        };
        if let Some(s) = subs.lock().await.get(sid).cloned() {
            let _ = s.send(ev.clone());
        }
    }
}

/// The session an event targets; `None` for session-less workspace events.
fn session_of(ev: &Event) -> Option<&str> {
    match ev {
        Event::StreamStart { session, .. }
        | Event::StreamEnd { session, .. }
        | Event::EntryUpsert { session, .. }
        | Event::Queue { session, .. }
        | Event::SessionEvent { session, .. }
        | Event::OmStatus { session, .. }
        | Event::SubagentEvent { session, .. }
        | Event::TaskChanged { session, .. } => Some(session),
        Event::System {
            session: Some(s), ..
        } => Some(s),
        Event::System { session: None, .. }
        | Event::SkillListChanged { .. }
        | Event::FileTreeChanged { .. } => None,
    }
}

impl Live {
    /// The model the trials run: the config's own default resolution.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
    /// The scratch config dir the trials run under; the container path
    /// copies it into each trial's container.
    #[must_use]
    pub fn scratch_config_dir(&self) -> &Path {
        self.scratch.path()
    }

    /// Subscribe to one session's events (before the send: the turn's events
    /// start flowing at once).
    async fn subscribe(&self, session: &str) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subs.lock().await.insert(session.to_owned(), tx);
        rx
    }

    /// Run one trial through the app's own dispatch path over `cwd` (the
    /// container path's `/app`).
    pub async fn run_trial(
        &self,
        task: &Task,
        rep: u32,
        artifacts_dir: &Path,
        cwd: &Path,
    ) -> Result<Outcome, EvalError> {
        let start = Instant::now();

        let open = self
            .core
            .dispatch(Command::WorkspaceOpen {
                cwd: cwd.display().to_string(),
            })
            .map_err(|e| EvalError::Live(format!("{e:?}")))?;
        let workspace = match open {
            CommandOutput::Workspace { workspace } => workspace.id,
            other => return Err(EvalError::Live(format!("workspace open: {other:?}"))),
        };

        let new = self
            .core
            .dispatch(Command::SessionNew {
                workspace: workspace.clone(),
                title: None,
            })
            .map_err(|e| EvalError::Live(format!("{e:?}")))?;
        let session = match new {
            CommandOutput::Session { session } => session.id,
            other => return Err(EvalError::Live(format!("session new: {other:?}"))),
        };

        let mut events = self.subscribe(&session).await;

        self.core
            .dispatch(Command::MessageSend {
                session: session.clone(),
                text: task.instruction.trim_end().to_owned(),
                lane: MessageLane::FollowUp,
            })
            .map_err(|e| EvalError::Live(format!("{e:?}")))?;

        let timed_out = settle(
            &mut events,
            &session,
            cwd,
            Duration::from_secs(task.timeout_secs()),
        )
        .await;
        if timed_out {
            // The bounded runaway: cut the stream so the turn ends.
            let _ = self.core.dispatch(Command::MessageStop {
                session: session.clone(),
            });
        }

        let (turns, tool_calls, tokens) = runner::read_session(cwd, &session)?;

        let check = if task.tb_native {
            // The TB protocol: tests at /tests, verdict at /logs/verifier.
            let script = artifacts_dir.join("check-tb.sh");
            std::fs::write(&script, TB_CHECK).map_err(EvalError::io)?;
            run_script_status(&script, cwd, Duration::from_secs(task.timeout_secs())).await?
        } else {
            run_script_status(&task.check(), cwd, Duration::from_secs(task.timeout_secs())).await?
        };
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
            Status::Error => "agent turn timed out".to_owned(),
        };

        let outcome = Outcome {
            task: task.id().to_owned(),
            rep,
            status,
            score: passed,
            wall_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            tokens,
            cost_usd: self.cost_of(&tokens),
            turns,
            tool_calls,
            note,
        };

        let trial = trial_dir(artifacts_dir, task.id(), rep);
        std::fs::create_dir_all(&trial).map_err(EvalError::io)?;
        runner::copy_session(cwd, &session, &trial.join("session.jsonl")).map_err(EvalError::io)?;
        crate::report::write_results(&trial, &outcome).map_err(EvalError::io)?;
        Ok(outcome)
    }

    /// The trial's cost from the config's per-1M prices; 0.0 when the model
    /// is unpriced (tokens still carry the efficiency signal).
    fn cost_of(&self, tokens: &runner::Tokens) -> f64 {
        let Some(c) = self.costs.get(&self.model) else {
            return 0.0;
        };
        // Counts fit a u32 by many orders of magnitude; saturate the absurd
        // case (a runaway trial) rather than cast.
        let in_m = f64::from(u32::try_from(tokens.input).unwrap_or(u32::MAX)) / 1e6;
        let out_m = f64::from(u32::try_from(tokens.output).unwrap_or(u32::MAX)) / 1e6;
        in_m * c.input + out_m * c.output
    }
}

/// Wait for the turn to end; `true` when the deadline hits first (the
/// runaway the caller stops). Two stages: the app's turn-end signal (the
/// post-turn empty `Queue` after the first `StreamEnd`, or a `System`
/// error), then file quiescence so late writers (subagent children,
/// post-turn appends) finish before `check.sh` runs.
async fn settle(
    events: &mut mpsc::UnboundedReceiver<Event>,
    session: &str,
    cwd: &Path,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut saw_end = false;
    let done = loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break false;
        };
        let Some(ev) = tokio::time::timeout(remaining, events.recv())
            .await
            .ok()
            .flatten()
        else {
            break false;
        };
        match &ev {
            Event::StreamEnd { session: s, .. } if s == session => saw_end = true,
            Event::Queue {
                session: s, items, ..
            } if s == session && saw_end && items.is_empty() => {
                break true;
            }
            Event::System {
                session: Some(s),
                kind: SystemEventKind::Error { .. },
                ..
            } if s == session => break true,
            _ => {}
        }
    };
    if done {
        quiescent(cwd, session, Duration::from_secs(30)).await;
    }
    !done
}

/// Poll the session file until it stops growing (5 s) or `bound` elapses.
async fn quiescent(cwd: &Path, session: &str, bound: Duration) {
    let path = cwd
        .join(".tau")
        .join("sessions")
        .join(format!("{session}.jsonl"));
    let deadline = Instant::now() + bound;
    let mut last = file_len(&path);
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let len = file_len(&path);
        if len == last || Instant::now() >= deadline {
            return;
        }
        last = len;
    }
}

fn file_len(p: &Path) -> u64 {
    match std::fs::metadata(p) {
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// Run the live suite: bounded concurrency, a hard budget, the resumable
/// checkpoint. A scheduler — not an up-front spawn — walks the trials in
/// order, so the budget gate sees spend as it accrues; once the budget is
/// hit it stops scheduling and the rest are reported as skipped (§2.6).
pub async fn run_suite(
    live: Arc<Live>,
    tasks: &[(Task, u32)],
    artifacts_dir: &Path,
    concurrency: usize,
    budget_usd: Option<f64>,
) -> Result<(Vec<Outcome>, usize), EvalError> {
    std::fs::create_dir_all(artifacts_dir).map_err(EvalError::io)?;
    let checkpoint_path = artifacts_dir.join("checkpoint.json");
    let ckpt = Arc::new(Mutex::new(runner::Checkpoint::load(&checkpoint_path)?));
    let done: HashSet<String> = ckpt.lock().await.completed.keys().cloned().collect();
    let due: Vec<(Task, u32)> = tasks
        .iter()
        .filter(|(t, rep)| !done.contains(&runner::trial_key(t.id(), *rep)))
        .map(|(t, rep)| (t.clone(), *rep))
        .collect();

    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let spent = Arc::new(Mutex::new(0.0_f64));
    let mut skipped = 0usize;
    let mut handles = Vec::new();

    // The scheduler loop (inline — `run_suite` is the scheduler): budget
    // gate, then permit, then spawn. The permit keeps at most `concurrency`
    // trials in flight, so spend accrues as trials finish.
    for (task, rep) in due {
        if let Some(b) = budget_usd
            && *spent.lock().await >= b
        {
            skipped += 1;
            continue;
        }
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        let ckpt = Arc::clone(&ckpt);
        let live = Arc::clone(&live);
        let spent = Arc::clone(&spent);
        let checkpoint_path = checkpoint_path.clone();
        let artifacts_dir = artifacts_dir.to_path_buf();
        handles.push(tokio::spawn(async move {
            let trial = crate::docker::run_trial(&live, &task, rep, &artifacts_dir).await;
            let outcome = match trial {
                Ok(o) => o,
                Err(e) => runner::error_outcome(&task, rep, &e),
            };
            if budget_usd.is_some() {
                *spent.lock().await += outcome.cost_usd;
            }
            ckpt.lock().await.record(&outcome, &checkpoint_path);
            drop(permit);
            outcome
        }));
    }

    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.push(handle.await.expect("a trial task does not panic"));
    }
    outcomes.sort_by(|a, b| a.task.cmp(&b.task).then(a.rep.cmp(&b.rep)));
    Ok((outcomes, skipped))
}
