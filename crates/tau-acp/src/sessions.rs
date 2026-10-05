//! The ACP session registry: one state block per `session/new`, holding the
//! mapper, the per-session turn lock, and the pending prompt response.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;

use crate::mapper::Mapper;

/// Why an in-flight `session/prompt` gets its response.
#[derive(Debug, Clone)]
pub enum Outcome {
    EndTurn,
    Cancelled,
    Error(String),
}

#[allow(clippy::struct_excessive_bools)] // one bit per turn-lifecycle state; merging is worse
pub struct SessionState {
    /// The workspace the session lives in (from `session/new`).
    pub workspace: String,
    /// The session's working directory; the model selector loads the
    /// project layer for it (M4).
    pub cwd: String,
    pub mapper: Mapper,
    /// A `session/prompt` is in flight (the per-session turn lock: a second
    /// prompt is rejected, not queued).
    pub in_flight: bool,
    /// The pending prompt response; the pump takes it out on settle.
    pub pending: Option<oneshot::Sender<Outcome>>,
    /// Settle rule (verified by the protocol tests, `tests/acp_protocol.rs`):
    /// after the first `StreamEnd`, an empty `Queue` settles the turn; a
    /// session-scoped `System` error settles it immediately. Both flags
    /// reset when a new prompt registers.
    pub saw_stream_end: bool,
    pub saw_interrupted: bool,
    /// A `session/cancel` landed for this turn: the settle then answers the
    /// prompt `cancelled` even if the abort surfaced as a `System` error
    /// (the spec MUSTs the response, N3).
    pub cancel_requested: bool,
    /// The session model's declared context window, when any config layer
    /// declares one: a `StreamEnd` with usage then rides a `usage_update`
    /// (the schema's `size` must not be guessed; N5). Resolved at
    /// `session/new`, refreshed by `set_config_option`.
    pub context_window: Option<u64>,
    /// The run's task text (the client prompt that started it): the
    /// headless continuation prompts re-state it (research doc §12.5).
    pub task: Option<String>,
    /// The last upserted assistant entry of the in-flight turn
    /// `(text, call count)`: the settle reads it for the completion marker.
    pub last_assistant: Option<(String, usize)>,
    /// The headless episode-loop state (ticket #73); a fresh run on each
    /// client prompt.
    pub headless: crate::headless::EpisodeState,
}

pub type Registry = Mutex<HashMap<String, SessionState>>;

/// Take the pending response out (first settle wins) and resolve it.
#[allow(clippy::implicit_hasher)] // the registry map is a std HashMap; no hasher seam in v0
pub fn settle(reg: &mut HashMap<String, SessionState>, session: &str, outcome: Outcome) -> bool {
    let Some(state) = reg.get_mut(session) else {
        return false;
    };
    let Some(tx) = state.pending.take() else {
        return false;
    };
    let _ = tx.send(outcome);
    true
}
