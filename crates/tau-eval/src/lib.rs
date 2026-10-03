//! tau-eval: the in-repo evaluation rig (ticket #57, first slice). Drives
//! `tau-core` in-process over a temp-dir workspace per trial, scores each
//! trial by a `check.sh` exit on the final state, and — on the deterministic
//! leg — runs the tasks against the mock LLM to assert harness invariants
//! (protocol, session integrity, tool dispatch) rather than model quality.

#![cfg_attr(test, allow(clippy::unwrap_used), allow(clippy::panic))]

use std::fmt;

pub mod gates;
pub mod report;
pub mod runner;
pub mod task;

/// A rig failure: task loading, the mock, session storage, the agent loop,
/// or a task script that failed.
#[derive(Debug)]
pub enum EvalError {
    Io(std::io::Error),
    /// The HTTP client failed to build.
    Http(reqwest::Error),
    /// A task package is malformed (bad `task.toml`, missing `instruction.md`).
    Task(String),
    /// The scenario set failed to load.
    Mock(String),
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
            Self::Http(e) => write!(f, "http client: {e}"),
            Self::Task(m) => write!(f, "task: {m}"),
            Self::Mock(m) => write!(f, "mock: {m}"),
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
            Self::Http(e) => Some(e),
            Self::Session(e) => Some(e),
            Self::Agent(e) => Some(e),
            _ => None,
        }
    }
}
