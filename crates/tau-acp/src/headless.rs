//! Headless (unattended) run control for ACP sessions: the episode loop,
//! the completion marker, and the continuation prompts.
//!
//! An ACP prompt is one client message in, one response out; headless runs
//! are "one prompt, N internal episodes". A turn that settles without a
//! completion declaration is not terminal — it is answered with a
//! task-anchored continuation prompt (research: `docs/research/headless-mode.md`
//! §12; ticket #73).

use crate::sessions::Outcome;

/// The base prompt frame for headless sessions (research doc §12.3): no
/// user present, no greetings or questions, verify by execution, completion
/// declared with the `task_complete` marker.
pub const FRAME: &str = "You are Tau, an autonomous coding agent running headless. \
There is no user at the other end of this session: you will not receive answers \
to questions, and no human will review your work before it is graded.

The task is your only contract. Keep working until the task's outcome is \
achieved, or until a concrete blocker makes it impossible (then state the \
blocker and stop).

RULES:
- Never open a turn with a greeting, self-introduction, or small talk. Never \
end a turn by asking the user a question or requesting confirmation. If you \
are uncertain, choose the most reasonable interpretation, state the assumption \
in your final summary, and proceed.
- Do not end a turn with plain text while work remains. A turn with no tool \
calls and no completion declaration is treated as incomplete and the run will \
continue.
- Verify by execution, never by assumption. Before declaring the task \
complete, gather concrete evidence from your own tool output that every \
requirement is met: run the program or tests, confirm each required output \
file exists at the exact requested path, and read the output back to confirm \
its contents.
- If you delegate work to a subagent, do not declare completion until the \
delegated work has landed and you have verified its results yourself.
- When (and only when) all requirements are verified, end your final response \
with a fenced JSON block:
```json
{\"task_complete\": true}
```";

/// The two-step completion confirmation (Terminus-2 shape, research doc
/// §12.4): the first declaration is met with this; the second ends the run.
const CONFIRMATION: &str = "Are you sure you want to mark the task as complete? \
This will trigger your solution to be graded and you won't be able to make any \
further corrections. If so, declare task_complete again in a fenced JSON block.";

/// Episode budget and degenerate-stop caps (research doc §12.6). Env
/// overrides keep the tests short without a config surface.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub max_episodes: u32,
    pub max_consecutive_empty: u32,
    pub max_identical_nudges: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_episodes: 100,
            max_consecutive_empty: 3,
            max_identical_nudges: 3,
        }
    }
}

impl Params {
    /// The env overrides (`TAU_ACP_*`); production runs use the defaults.
    #[must_use]
    pub fn from_env() -> Self {
        fn env_u32(var: &str, default: u32) -> u32 {
            std::env::var(var)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        Self {
            max_episodes: env_u32("TAU_ACP_MAX_EPISODES", 100),
            max_consecutive_empty: env_u32("TAU_ACP_MAX_EMPTY", 3),
            max_identical_nudges: env_u32("TAU_ACP_MAX_IDENTICAL", 3),
        }
    }
}

/// Per-session episode state; reset to a fresh run on each client prompt.
#[derive(Debug, Clone, Default)]
pub struct EpisodeState {
    /// The current episode number (the first client prompt is episode 1).
    pub episode: u32,
    pub consecutive_empty: u32,
    pub consecutive_identical: u32,
    pub last_nudge: Option<String>,
    /// A completion was declared and the confirmation round sent; the next
    /// declaration ends the run.
    pub confirmation_sent: bool,
}

/// What the settle does with a headless turn that ended.
#[derive(Debug)]
pub enum Decision {
    /// The run continues: send this text as the next user message (episode
    /// N+1).
    Continue(String),
    /// The run ends: answer the `session/prompt` with this outcome.
    Done(Outcome),
}

/// The completion marker in a tool-free final response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// A fenced JSON block declares `{"task_complete": true}`.
    Complete,
    /// The text mentions the marker but no block parses — a nudge explains
    /// the shape.
    Malformed,
    Absent,
}

/// Detect the completion marker (lenient): any text between two code
/// fences that parses to a `task_complete: true` object completes the run;
/// a mention of the marker without a parseable block is malformed, not
/// absent (fences may start mid-line, as in "All verified. <fence>json").
#[must_use]
pub fn detect_marker(text: &str) -> Marker {
    let mentioned = text.contains("task_complete");
    let mut fences = Vec::new();
    let mut from = 0;
    while let Some(rel) = text.get(from..).and_then(|rest| rest.find("```")) {
        let pos = from + rel;
        fences.push(pos);
        from = pos + 3;
    }
    for (a, &open) in fences.iter().enumerate() {
        for &close in &fences[a + 1..] {
            // Skip the opening fence's info string (the word after the fence).
            let open_end = text
                .get(open..)
                .and_then(|rest| rest.find('\n').map(|i| open + i + 1))
                .unwrap_or(close);
            let inner = text.get(open_end..close).unwrap_or("");
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(inner)
                && value
                    .get("task_complete")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            {
                return Marker::Complete;
            }
        }
    }
    if mentioned {
        Marker::Malformed
    } else {
        Marker::Absent
    }
}

/// The settle decision for a headless session (research doc §12.2/12.4/12.5).
/// `final_text` is the settled turn's last assistant text; `task` is the
/// run's anchor (every continuation re-states it — the greeting-degeneration
/// countermeasure).
#[must_use]
pub fn decide(state: &mut EpisodeState, params: &Params, task: &str, final_text: &str) -> Decision {
    match detect_marker(final_text) {
        Marker::Complete => {
            if state.confirmation_sent {
                // The two-step confirmation is satisfied.
                Decision::Done(Outcome::EndTurn)
            } else {
                state.confirmation_sent = true;
                state.consecutive_empty = 0;
                state.consecutive_identical = 0;
                continue_episode(state, params, CONFIRMATION.to_owned())
            }
        }
        Marker::Malformed => {
            let nudge = "Your previous response mentioned task_complete, but the \
JSON block did not parse. To finish, end your final response with a fenced \
JSON block:
```json
{\"task_complete\": true}
```"
            .to_owned();
            continue_episode(state, params, nudge)
        }
        Marker::Absent => {
            // A declaration that did not survive the confirmation round is
            // an early stop again: the model may re-declare later.
            state.confirmation_sent = false;
            let trimmed = final_text.trim();
            let nudge = if trimmed.is_empty() {
                format!("Your previous response was empty. Continue the task: {task}.")
            } else if trimmed.ends_with('?') {
                format!(
                    "There is no user to answer questions in this session. Resolve \
your question yourself with your tools (search, read, run), pick the most \
reasonable option, and continue the task: {task}."
                )
            } else {
                format!(
                    "The task is not complete. Task: {task}. Continue from the \
current state; do not re-introduce yourself and do not ask questions."
                )
            };
            continue_episode(state, params, nudge)
        }
    }
}

/// Budget check, then the episode advances and the nudge is queued
/// (research doc §12.6: the caps settle the run instead of nudging forever).
fn continue_episode(state: &mut EpisodeState, params: &Params, nudge: String) -> Decision {
    if nudge.is_empty() || is_empty_nudge(&nudge) {
        state.consecutive_empty += 1;
        state.consecutive_identical = 0;
        if state.consecutive_empty >= params.max_consecutive_empty {
            return Decision::Done(Outcome::EndTurn);
        }
    } else if state.last_nudge.as_deref() == Some(nudge.as_str()) {
        state.consecutive_identical += 1;
        state.consecutive_empty = 0;
        if state.consecutive_identical > params.max_identical_nudges {
            return Decision::Done(Outcome::EndTurn);
        }
    } else {
        state.consecutive_identical = 1;
        state.consecutive_empty = 0;
    }
    if state.episode >= params.max_episodes {
        return Decision::Done(Outcome::EndTurn);
    }
    state.episode += 1;
    state.last_nudge = Some(nudge.clone());
    Decision::Continue(nudge)
}

fn is_empty_nudge(nudge: &str) -> bool {
    nudge.starts_with("Your previous response was empty")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_parses_a_fenced_block() {
        assert_eq!(
            detect_marker("All done.\n```json\n{\"task_complete\": true}\n```"),
            Marker::Complete
        );
    }

    #[test]
    fn marker_is_lenient_about_surrounding_text() {
        assert_eq!(
            detect_marker(
                "Summary of work. ```json\n{\"task_complete\": true, \"note\": \"ok\"}\n``` Thanks."
            ),
            Marker::Complete
        );
    }

    #[test]
    fn marker_is_malformed_when_the_block_does_not_parse() {
        assert_eq!(
            detect_marker("```json\n{\"task_complete\": true\n```"),
            Marker::Malformed
        );
        assert_eq!(detect_marker("task_complete was true"), Marker::Malformed);
    }

    #[test]
    fn marker_absent_on_plain_text() {
        assert_eq!(
            detect_marker("I will start by reading the files."),
            Marker::Absent
        );
        assert_eq!(detect_marker(""), Marker::Absent);
    }

    #[test]
    fn early_stop_continues_and_completion_confirms_twice() {
        let p = Params::default();
        let mut s = EpisodeState {
            episode: 1,
            ..Default::default()
        };
        // Episode 1: plain text, no marker → a continuation.
        let d = decide(&mut s, &p, "TASK", "working on it");
        assert!(matches!(d, Decision::Continue(_)), "{d:?}");
        // Episode 2: the declaration → the confirmation round.
        let d = decide(
            &mut s,
            &p,
            "TASK",
            "done ```json\n{\"task_complete\": true}\n```",
        );
        assert!(
            matches!(d, Decision::Continue(ref t) if t.contains("Are you sure")),
            "{d:?}"
        );
        // Episode 3: the second declaration → the run ends.
        let d = decide(
            &mut s,
            &p,
            "TASK",
            "yes ```json\n{\"task_complete\": true}\n```",
        );
        assert!(matches!(d, Decision::Done(Outcome::EndTurn)), "{d:?}");
    }

    #[test]
    fn empty_stops_settle_at_the_budget() {
        let p = Params {
            max_consecutive_empty: 3,
            ..Params::default()
        };
        let mut s = EpisodeState {
            episode: 1,
            ..Default::default()
        };
        assert!(matches!(decide(&mut s, &p, "T", ""), Decision::Continue(_)));
        assert!(matches!(decide(&mut s, &p, "T", ""), Decision::Continue(_)));
        assert!(matches!(decide(&mut s, &p, "T", ""), Decision::Done(_)));
    }

    #[test]
    fn identical_nudges_settle_at_the_budget() {
        let p = Params {
            max_identical_nudges: 3,
            ..Params::default()
        };
        let mut s = EpisodeState {
            episode: 1,
            ..Default::default()
        };
        // The mock clamps: the same final text every episode → the same
        // generic nudge → the cap settles the run.
        for _ in 0..3 {
            assert!(matches!(
                decide(&mut s, &p, "T", "stuck"),
                Decision::Continue(_)
            ));
        }
        assert!(matches!(
            decide(&mut s, &p, "T", "stuck"),
            Decision::Done(_)
        ));
    }

    #[test]
    fn max_episodes_caps_the_run() {
        let p = Params {
            max_episodes: 3,
            ..Params::default()
        };
        let mut s = EpisodeState {
            episode: 3,
            ..Default::default()
        };
        // Vary the text so the identical-nudge cap does not fire first.
        assert!(matches!(
            decide(&mut s, &p, "T", "stuck again"),
            Decision::Done(_)
        ));
    }

    #[test]
    fn question_stops_get_the_auto_answer() {
        let p = Params::default();
        let mut s = EpisodeState {
            episode: 1,
            ..Default::default()
        };
        let d = decide(&mut s, &p, "TASK", "Should I use MySQL or Postgres?");
        assert!(
            matches!(d, Decision::Continue(ref t) if t.contains("no user to answer questions")),
            "{d:?}"
        );
    }
}
