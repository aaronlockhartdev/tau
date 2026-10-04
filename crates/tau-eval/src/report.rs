//! Per-trial `results.json` and the readable run summary (ticket #57).

use std::fmt::Write;
use std::path::Path;

use crate::runner::{Outcome, Status};

/// Write one trial's record next to its trajectory.
pub fn write_results(trial_dir: &Path, outcome: &Outcome) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(outcome).expect("an outcome is always serializable");
    std::fs::write(trial_dir.join("results.json"), json)
}

/// The live leg's summary: one line per task aggregating its repetitions
/// (pass rate, summed tokens/cost/wall), plus the per-rep notes and the
/// budget-skipped count.
#[must_use]
pub fn render_summary_live(outcomes: &[Outcome], skipped: usize) -> String {
    let mut by_task: std::collections::BTreeMap<&str, Vec<&Outcome>> =
        std::collections::BTreeMap::new();
    for o in outcomes {
        by_task.entry(&o.task).or_default().push(o);
    }

    let mut s = String::new();
    let mut total_pass = 0usize;
    let mut total_trials = 0usize;
    for (task, os) in &by_task {
        let passed = os.iter().filter(|o| o.status == Status::Pass).count();
        total_pass += passed;
        total_trials += os.len();
        let tin: u64 = os.iter().map(|o| o.tokens.input).sum();
        let tout: u64 = os.iter().map(|o| o.tokens.output).sum();
        let cost: f64 = os.iter().map(|o| o.cost_usd).sum();
        let wall: u64 = os.iter().map(|o| o.wall_ms).sum();
        let _ = writeln!(
            s,
            "  {:<36} {passed}/{:<3} in={tin:<7} out={tout:<6} ${cost:.4}  {}ms",
            task,
            os.len(),
            wall,
        );
        for o in os.iter().filter(|o| !o.note.is_empty()) {
            let _ = writeln!(s, "      rep {} note: {}", o.rep, first_line(&o.note));
        }
    }
    let _ = writeln!(
        s,
        "summary: {total_pass}/{total_trials} trials passed across {} tasks",
        by_task.len()
    );
    if skipped > 0 {
        let _ = writeln!(s, "budget: {skipped} trial(s) not run (budget exhausted)");
    }
    s
}
fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}
