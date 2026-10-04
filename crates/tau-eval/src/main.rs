//! `tau-eval live [--reps N] [--budget USD] [--filter SUBSTR]` — the
//! benchmark: the user's real harness on a real model over the Terminal-Bench
//! 2.0 set, every trial in the task's own container image (ticket #65). The
//! justfile's `eval` target wraps it. Paths are repo-relative
//! (the justfile runs from the repo root).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tau_eval::report;
use tau_eval::runner;
use tau_eval::task;
/// The trial pool size (ticket #57, §2.2: small by default).
const CONCURRENCY: usize = 4;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "inner") {
        return inner(args.get(1..)).await;
    }
    if args.first().is_none_or(|a| a != "live") {
        return fail("usage: tau-eval live [--reps N] [--budget USD] [--filter SUBSTR]");
    }
    let rest = &args[1..];
    let (reps, budget, filter) = match parse_args(rest) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    // Absolute task root: the runner executes the task scripts with the
    // container's /app as cwd, so a relative root would not resolve.
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => return fail(&format!("current dir: {e}")),
    };
    let tasks_dir = cwd.join("eval/tasks");
    let artifacts_dir = PathBuf::from(format!("target/eval/{stamp}", stamp = stamp()));

    // The curation layer: which upstream tasks we benchmark (ticket #65).
    let selection_path = cwd.join("eval/tb-selection.toml");
    let selection: tau_eval::tb::Selection = match std::fs::read_to_string(&selection_path)
        .map_err(|e| format!("{}: {e}", selection_path.display()))
        .and_then(|raw| {
            toml::from_str(&raw).map_err(|e| format!("{}: {e}", selection_path.display()))
        }) {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    // The dataset itself is fetched at runtime, never vendored.
    let checkout =
        match tau_eval::tb::ensure_checkout(&cwd.join("target/terminal-bench"), &selection.repo)
            .await
        {
            Ok(c) => c,
            Err(e) => return fail(&format!("dataset: {e}")),
        };
    let mut due: Vec<_> = match tau_eval::tb::load(&checkout, &selection) {
        Ok(t) => t,
        Err(e) => return fail(&format!("tasks: {e}")),
    };
    // Hand-written tasks (if any) live under eval/tasks.
    if tasks_dir.is_dir() {
        match task::load(&tasks_dir) {
            Ok(local) => due.extend(local),
            Err(e) => return fail(&format!("tasks: {e}")),
        }
    }
    due.sort_by(|a, b| a.id().cmp(b.id()));
    let due: Vec<_> = due
        .into_iter()
        .filter(|t| filter.as_deref().is_none_or(|f| t.id().contains(f)))
        .collect();
    if due.is_empty() {
        return fail("no tasks match the filter");
    }
    let reps = reps.max(1);
    let trials: Vec<(task::Task, u32)> = due
        .into_iter()
        .flat_map(|t| {
            let t = t.clone();
            (1..=reps).map(move |r| (t.clone(), r))
        })
        .collect();

    let live = match tau_eval::live::start() {
        Ok(l) => std::sync::Arc::new(l),
        Err(e) => return fail(&format!("{e}")),
    };
    if live.model().is_empty() {
        return fail("the user's config names no model");
    }
    eprintln!(
        "tau-eval: live leg — model {} (the user's config), {} trials",
        live.model(),
        trials.len()
    );
    match tau_eval::live::run_suite(live, &trials, &artifacts_dir, CONCURRENCY, budget).await {
        Ok((outcomes, skipped)) => finish(&outcomes, &artifacts_dir, skipped),
        Err(e) => fail(&format!("run: {e}")),
    }
}

/// `tau-eval inner --task <dir> --workspace <path>`: the in-container half
/// of the docker execution path — one trial against the image's /app, the
/// result JSON on the last stdout line.
async fn inner(args: Option<&[String]>) -> ExitCode {
    let mut task_dir: Option<&str> = None;
    let mut workspace: Option<&str> = None;
    let mut tb = false;
    let mut task_id: Option<&str> = None;
    let mut timeout: u64 = 900;
    let mut it = args.unwrap_or_default().iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--task" => task_dir = it.next().map(String::as_str),
            "--workspace" => workspace = it.next().map(String::as_str),
            "--tb" => tb = true,
            "--id" => task_id = it.next().map(String::as_str),
            "--timeout" => timeout = it.next().and_then(|s| s.parse().ok()).unwrap_or(900),
            other => return fail(&format!("inner: unknown argument {other:?}")),
        }
    }
    let Some(task_dir) = task_dir else {
        return fail("inner: --task <dir> is required");
    };
    let Some(workspace) = workspace else {
        return fail("inner: --workspace <path> is required");
    };
    let task = if tb {
        // The host knows the task's id (the in-container dir is /tmp/task).
        let id = task_id.map_or(
            Path::new(task_dir)
                .file_name()
                .map_or(task_dir.to_owned(), |n| n.to_string_lossy().into_owned()),
            ToOwned::to_owned,
        );
        match task::from_tb(Path::new(task_dir), &id, timeout, None) {
            Ok(t) => t,
            Err(e) => return fail(&format!("task: {e}")),
        }
    } else {
        match task::load_one(Path::new(task_dir)) {
            Ok(t) => t,
            Err(e) => return fail(&format!("task: {e}")),
        }
    };
    let live = match tau_eval::live::start() {
        Ok(l) => l,
        Err(e) => return fail(&format!("{e}")),
    };
    let outcome = match live
        .run_trial(&task, 0, Path::new("/tmp/trial"), Path::new(workspace))
        .await
    {
        Ok(o) => o,
        Err(e) => return fail(&format!("trial: {e}")),
    };
    let json = match serde_json::to_string(&tau_eval::docker::TrialResult::from(&outcome)) {
        Ok(j) => j,
        Err(e) => return fail(&format!("result json: {e}")),
    };
    println!("{json}");
    if outcome.status == tau_eval::runner::Status::Pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
/// Print the summary, persist it, and map the outcomes to an exit code.
fn finish(outcomes: &[runner::Outcome], artifacts_dir: &PathBuf, skipped: usize) -> ExitCode {
    let summary = report::render_summary_live(outcomes, skipped);
    println!("{summary}");
    let _ = std::fs::create_dir_all(artifacts_dir);
    let _ = std::fs::write(artifacts_dir.join("summary.txt"), &summary);
    eprintln!("tau-eval: artifacts at {}", artifacts_dir.display());
    let bad = outcomes
        .iter()
        .filter(|o| o.status != runner::Status::Pass)
        .count();
    if bad > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `tau-eval live [--reps N] [--budget USD] [--filter SUBSTR]`.
fn parse_args(rest: &[String]) -> Result<(u32, Option<f64>, Option<String>), String> {
    let mut reps = 0u32;
    let mut budget = None;
    let mut filter = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--reps" => {
                i += 1;
                reps = rest.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
            "--budget" => {
                i += 1;
                budget = rest.get(i).and_then(|s| s.parse().ok());
            }
            "--filter" => {
                i += 1;
                filter = rest.get(i).map(ToOwned::to_owned);
            }
            other => {
                return Err(format!("unknown argument {other:?}"));
            }
        }
        i += 1;
    }
    Ok((reps, budget, filter))
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("tau-eval: {msg}");
    ExitCode::FAILURE
}

/// A compact, sortable run stamp (epoch seconds.nanos — the nanos keep two
/// runs started in the same second apart).
fn stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}{:09}", d.as_secs(), d.subsec_nanos()))
        .unwrap_or_default()
}
