use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The session entry kind for sub-agent lifecycle records (spawn, state
/// transitions, notifies) — the child's session file is the record.
pub const KIND_SUBAGENT: &str = "subagent";

/// A parked child's declared wait target (spec §5.2: a secondary hint —
/// the observed rest is the driver; the declaration rides the `idle`
/// state as the GUI's badge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitingOn {
    Parent,
    User,
    Subagent,
}

/// The turn origin (spec §5.2 wake rules): a per-drive property, not a
/// sticky mode. A rest after *autonomous* (spawn-origin) work wakes the
/// delegating parent; a rest that replies to a *direct message* (user in
/// the tab, a parent `subagent_message`, a resume) is silent to the
/// parent — the sender is engaged, and the state event is the GUI badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOrigin {
    /// The spawn's brief, or its continuation — autonomous work.
    Spawn,
    /// A direct message (user chat, a parent `subagent_message`, a resume).
    Message,
}
impl WaitingOn {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::User => "user",
            Self::Subagent => "subagent",
        }
    }
}

/// Who stopped the child (the only asymmetry between stops — ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoppedBy {
    User,
    Parent,
}

impl StoppedBy {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Parent => "parent",
        }
    }
}

/// The 6-state lifecycle (spec §5.2, 2026-10-15 plan-file revision):
/// machine-observed — the states are read from observed signals, not
/// declared. All non-running states are deliberately resumable; nothing
/// auto-resumes anything (ADR-0001 supplement).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    Running,
    Idle {
        waiting_on: WaitingOn,
    },
    /// A `QUESTION:` marker in the child's final output (spec §5.2): the
    /// child is asking the user and rests for an answer.
    Awaiting,
    Done {
        output: Option<Value>,
    },
    Failed {
        reason: String,
    },
    Stopped {
        by: StoppedBy,
    },
}

impl ChildState {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle { .. } => "idle",
            Self::Awaiting => "awaiting",
            Self::Done { .. } => "done",
            Self::Failed { .. } => "failed",
            Self::Stopped { .. } => "stopped",
        }
    }
}

/// A spawn's context mode (spec §5.1; the protocol carries its own mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    Fresh,
    Compacted,
    Fork,
}
