//! `tau-eval <tier>` — run an eval tier against the in-process deterministic
//! mock (ticket #57). Defaults to `smoke`; the justfile's `eval` target wraps
//! this. Paths are repo-relative (the justfile runs from the repo root).

use std::path::PathBuf;
use std::process::ExitCode;

use tau_eval::report;
use tau_eval::runner;
use tau_eval::task::{self, Tier};

/// The trial pool size (ticket #57, §2.2: small by default).
const CONCURRENCY: usize = 4;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tier_arg = args.first().map_or("smoke", |s| s.as_str());
    let Some(tier) = parse_tier(tier_arg) else {
        return fail(&format!(
            "unknown tier {tier_arg:?} (expected \"smoke\" or \"full\")"
        ));
    };
    // Absolute task/scenario roots: the runner executes the task scripts with
    // the temp-dir workspace as cwd, so a relative root would not resolve.
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => return fail(&format!("current dir: {e}")),
    };
    let tasks_dir = cwd.join("eval/tasks");
    let scenarios_dir = cwd.join("eval/scenarios");
    let artifacts_dir = PathBuf::from(format!("target/eval/{stamp}-{tier}", stamp = stamp()));

    let mock = match runner::start_mock(&scenarios_dir).await {
        Ok(m) => m,
        Err(e) => return fail(&format!("mock: {e}")),
    };

    let due: Vec<_> = match task::load(&tasks_dir) {
        Ok(all) => all.into_iter().filter(|t| t.tier() == tier).collect(),
        Err(e) => return fail(&format!("tasks: {e}")),
    };
    if due.is_empty() {
        return fail(&format!("no {tier} tasks under {}", tasks_dir.display()));
    }

    match runner::run_suite(&due, &mock.base_url, &artifacts_dir, CONCURRENCY).await {
        Ok(outcomes) => {
            let summary = report::render_summary(&outcomes);
            println!("{summary}");
            let _ = std::fs::create_dir_all(&artifacts_dir);
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
        Err(e) => fail(&format!("run: {e}")),
    }
}

fn parse_tier(s: &str) -> Option<Tier> {
    match s {
        "smoke" => Some(Tier::Smoke),
        "full" => Some(Tier::Full),
        _ => None,
    }
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
