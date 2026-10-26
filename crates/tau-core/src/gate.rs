//! The completion gate (design "The mechanical completion gate is the
//! oracle", spec §5.3): a global, mechanical, user-configured check the core
//! runs at every qualifying turn-end. It is the oracle — the model's
//! self-report is held to the user's explicit, visible verification criteria
//! (ADR-0007 Supplement: a deliberate transparency strengthening). The check
//! is authored by the delegator (the user for a root session), never the
//! model; it runs with the user's privileges and bounces the *model* (a
//! repair turn), never the user.

use crate::config::Gates;
use std::path::Path;
use std::time::Duration;

/// The cap on the check output carried into the `gate_attempt` record and the
/// message: a failing build/test suite can emit megabytes, and the record
/// (and the model's context) only needs the head of it.
const MAX_OUTPUT: usize = 4000;

/// A `QUESTION:` line in the final assistant text (spec §5.3): the model is
/// asking the user, so the turn is `awaiting_input` and the gate is skipped.
#[must_use]
pub fn is_question(text: &str) -> bool {
    text.lines().any(|line| line.starts_with("QUESTION:"))
}

/// One check run's outcome. The gate is total: it never aborts a turn, so a
/// timeout, a spawn failure, and a non-zero exit all land here as a `false`
/// `passed` with a diagnostic `output` — "did not pass" is the honest answer
/// for every of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateResult {
    /// The check exited 0.
    pub passed: bool,
    /// The check's combined output, truncated to `MAX_OUTPUT`.
    pub output: String,
}

/// Run the check command with its deadline, in the session's cwd. Mirrors the
/// `bash` tool's execution (a `bash -c`, `kill_on_drop` so a timed-out child
/// is killed, not left behind).
pub async fn run(gates: &Gates, cwd: &Path) -> GateResult {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(&gates.command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output =
        match tokio::time::timeout(Duration::from_secs(gates.timeout_secs), cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return GateResult {
                    passed: false,
                    output: format!("the check did not run: {e}"),
                };
            }
            Err(_) => {
                return GateResult {
                    passed: false,
                    output: format!("the check timed out after {}s", gates.timeout_secs),
                };
            }
        };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    GateResult {
        passed: output.status.success(),
        output: truncate(&text),
    }
}

/// Truncate the check output to `MAX_OUTPUT` chars, marking the cut (#219:
/// on a line boundary — the last visible line is complete, and the marker
/// rides the end of it).
fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_OUTPUT {
        return s.to_owned();
    }
    let mut out = s.to_owned();
    let mut count = 0;
    for (i, _) in out.char_indices() {
        count += 1;
        if count > MAX_OUTPUT {
            let prefix = out.get(..i).expect("char_indices yields a boundary");
            let cut = prefix.rfind('\n').map_or(0, |j| j + 1);
            out.truncate(cut);
            let _ = out.pop();
            out.push('…');
            break;
        }
    }
    out
}

/// The steering-lane message that bounces the repair turn: the model sees the
/// failure and is told to make the check pass.
#[must_use]
pub fn repair_message(
    gates: &Gates,
    result: &GateResult,
    attempt: u32,
    max_retries: u32,
) -> String {
    format!(
        "Your work failed the completion gate — the check `{}` did not pass (repair {attempt} of \
{max_retries}).\n\n{}\n\nFix your work so the check passes, then end your turn again. If you \
need the user's input, end your turn with a line starting `QUESTION:`.",
        gates.command, result.output
    )
}

/// The system-prompt section stating what "done" is verified against (the
/// design: "your work is verified by: …"). Present only when a gate is
/// configured.
#[must_use]
pub fn section(gates: &Gates) -> String {
    format!(
        "## Completion gate\n\nYour work is verified by this check, which the core runs at the \
end of every turn in which you use a tool (a turn that ends with a `QUESTION:` line is skipped): \
\n\n    {}\n\nIf it fails, your turn is bounced with the failure output and you must repair your \
work until it passes.",
        gates.command
    )
}

/// Append the completion-gate section to a system prompt (spec §5.3): a
/// no-op when no gate is configured.
pub fn with_section(prompt: &mut String, gates: &Option<crate::config::Gates>) {
    if let Some(g) = gates {
        prompt.push_str("\n\n");
        prompt.push_str(&section(g));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_line_marks_the_text() {
        assert!(is_question("QUESTION: which DB should I use?"));
        assert!(is_question("done\nQUESTION: ship it?"));
        // A line that merely contains the marker is not one that starts with it.
        assert!(!is_question("the text mentions QUESTION: mid-line"));
        assert!(!is_question("all done"));
    }

    #[tokio::test]
    async fn a_passing_check_passes() {
        let gates = Gates {
            command: "true".into(),
            ..Gates::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run(&gates, dir.path()).await;
        assert!(result.passed, "true must pass: {:?}", result.output);
    }

    #[tokio::test]
    async fn a_failing_check_fails_with_its_output() {
        let gates = Gates {
            command: "echo boom; exit 3".into(),
            ..Gates::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run(&gates, dir.path()).await;
        assert!(!result.passed);
        assert!(result.output.contains("boom"), "got: {:?}", result.output);
    }

    #[tokio::test]
    async fn a_missing_command_is_a_failure_not_a_panic() {
        let gates = Gates {
            command: "definitely-not-a-real-command-xyz".into(),
            ..Gates::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run(&gates, dir.path()).await;
        assert!(!result.passed);
        assert!(!result.output.is_empty());
    }

    #[tokio::test]
    async fn a_timed_out_check_fails() {
        let gates = Gates {
            command: "sleep 5".into(),
            timeout_secs: 1,
            ..Gates::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run(&gates, dir.path()).await;
        assert!(!result.passed);
        assert!(
            result.output.contains("timed out"),
            "got: {:?}",
            result.output
        );
    }

    #[test]
    fn the_output_is_truncated() {
        let big = "x".repeat(MAX_OUTPUT + 100);
        let out = truncate(&big);
        assert!(out.chars().count() <= MAX_OUTPUT + 20);
        assert!(out.ends_with('…'));
        // Short output is untouched.
        assert_eq!(truncate("short"), "short");
    }

    #[test]
    fn the_output_is_truncated_at_a_line_boundary() {
        // #219: the cut lands on a line boundary — the last visible line is
        // complete and the marker follows on its own line.
        let big = (0..600)
            .map(|i| format!("line {i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(big.chars().count() > MAX_OUTPUT);
        let out = truncate(&big);
        assert!(out.ends_with('…'));
        let body = out.strip_suffix('…').expect("the marker");
        assert!(!body.ends_with('\n'));
        assert!(body.ends_with("line 443"), "last visible line: {out:?}");
        assert!(out.chars().count() <= MAX_OUTPUT + 20);
    }

    #[test]
    fn the_section_names_the_command() {
        let gates = Gates {
            command: "cargo nextest run".into(),
            ..Gates::default()
        };
        let section = section(&gates);
        assert!(section.contains("cargo nextest run"));
        assert!(section.contains("Completion gate"));
    }

    #[test]
    fn the_repair_message_carries_the_output_and_budget() {
        let gates = Gates {
            command: "cargo test".into(),
            ..Gates::default()
        };
        let result = GateResult {
            passed: false,
            output: "test failed".into(),
        };
        let msg = repair_message(&gates, &result, 1, 2);
        assert!(msg.contains("cargo test"));
        assert!(msg.contains("test failed"));
        assert!(msg.contains("repair 1 of 2"));
        assert!(msg.contains("QUESTION:"));
    }
}
