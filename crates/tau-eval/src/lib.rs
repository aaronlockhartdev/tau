//! tau-eval: the in-repo evaluation rig (tickets #57/#65). Runs the user's
//! real harness on a real model over the Terminal-Bench 2.0 task set, every
//! trial inside the task's own container image, scored by a `check.sh` exit
//! on the final state.

#![cfg_attr(test, allow(clippy::unwrap_used), allow(clippy::panic))]

use std::fmt;

pub mod docker;
pub mod gates;
pub mod live;
pub mod report;
pub mod runner;
pub mod task;
pub mod tb;

/// A rig failure: task loading, session storage, the agent loop,
/// or a task script that failed.
#[derive(Debug)]
pub enum EvalError {
    Io(std::io::Error),
    /// A task package is malformed (bad `task.toml`, missing `instruction.md`).
    Task(String),
    /// The live leg could not start (no user config, core build, dispatch).
    Live(String),
    Session(tau_core::session::Error),
    Agent(tau_core::agent::AgentError),
    /// `AgentSession::launch` failed (the protocol error, stringified).
    Launch(String),
    /// A task script (setup / check / oracle) exited non-zero.
    Script {
        script: String,
        code: i32,
        output: String,
    },
}

impl EvalError {
    fn io(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Task(m) => write!(f, "task: {m}"),
            Self::Live(m) => write!(f, "live: {m}"),
            Self::Session(e) => write!(f, "session: {e}"),
            Self::Agent(e) => write!(f, "agent: {e}"),
            Self::Launch(m) => write!(f, "launch: {m}"),
            Self::Script { script, code, .. } => write!(f, "script {script} exited {code}"),
        }
    }
}

impl std::error::Error for EvalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Session(e) => Some(e),
            Self::Agent(e) => Some(e),
            _ => None,
        }
    }
}
