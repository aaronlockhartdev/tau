//! Per-trial `results.json` and the readable run summary (ticket #57).

use std::fmt::Write;
use std::path::Path;

use crate::runner::{Outcome, Status};

/// Write one trial's record next to its trajectory.
pub fn write_results(trial_dir: &Path, outcome: &Outcome) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(outcome).expect("an outcome is always serializable");
    std::fs::write(trial_dir.join("results.json"), json)
}

/// The human-readable run summary: one line per trial plus a totals line.
#[must_use]
pub fn render_summary(outcomes: &[Outcome]) -> String {
    let pass = outcomes.iter().filter(|o| o.status == Status::Pass).count();
    let fail = outcomes.iter().filter(|o| o.status == Status::Fail).count();
    let err = outcomes
        .iter()
        .filter(|o| o.status == Status::Error)
        .count();

    let mut s = String::new();
    for o in outcomes {
        let calls = if o.tool_calls.is_empty() {
            String::from("-")
        } else {
            o.tool_calls.join(",")
        };
        let _ = writeln!(
            s,
            "  {:<6} {:<16} {:>6}ms  in={:<5} out={:<5} calls={}",
            o.status.as_str().to_uppercase(),
            o.task,
            o.wall_ms,
            o.tokens.input,
            o.tokens.output,
            calls,
        );
        if !o.note.is_empty() {
            let _ = writeln!(s, "         note: {}", first_line(&o.note));
        }
    }
    let _ = writeln!(
        s,
        "summary: {pass} passed, {fail} failed, {err} errored ({} trials)",
        outcomes.len()
    );
    s
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}
