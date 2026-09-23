//! The transport-free composition (ADR-0002, ADR-0006): the owned state —
//! workspaces, live sessions — plus the `dispatch` surface, the post-turn
//! reconciliation, the event pump, and the file watchers. Everything a
//! non-Tauri interface (a future `tau serve`, a CLI) needs; the Tauri
//! shell (the app crate) carries only the transport — the command, the
//! managed state wrapper, the menu, and the pump's sink.
//!
//! The core is the single owner of state (ADR-0006): workspaces and live
//! sessions live here; the GUI rebuilds from snapshots + the event stream.
//! Stream deltas reach clients through the provider seam — a forwarding
//! provider that wraps the loop's provider (the loop itself is untouched) —
//! and the 25 ms coalescer in the event pump.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::agent::{AgentSession, Lane, SessionParams, TurnConfig};
use crate::config::{self, Config};
use crate::context;
use crate::provider::{
    self, ProviderTurn, ResponseRequest, TurnEvent, TurnProvider, TurnProviderRef, TurnSink,
    Usage as CoreUsage,
};
use crate::session::{Entry, SessionStore};
use crate::subagent::{
    BoxedDrive, ChildDriver, ChildProviderFactory, SpawnNotice, StateNotice, StoppedBy,
    SubagentBridge, Supervisor, SupervisorParams, WakeKind, WakeNotice,
};
use crate::tools;
use serde_json::{Value, json};
use tau_protocol::coalesce::Coalescer;
use tau_protocol::snapshot::{
    EntryMeta, EntryStatus, LiveState, OmSnapshot, QueuedItem, SessionMeta, Snapshot, TurnState,
    ViewEntry, Workspace,
};
use tau_protocol::{
    AgentType, Command, CommandOutput, ContextMode, Event, FileEntry, FileText, MessageLane,
    OmStatusKind, ProtocolError, ProviderInfo, SkillInfo, SubagentEventKind, SubagentInfo,
    SystemEventKind, Usage,
};
use tokio::sync::mpsc;

mod dispatch;
mod dispatch_commands;
mod dispatch_sessions;
mod forwarding;
pub mod pump;
mod sessions;
mod snapshot;
mod state;
mod turn;
mod watch;
mod watchers;

pub(crate) use forwarding::*;
pub(crate) use snapshot::*;
pub(crate) use state::*;
pub use state::{Core, CoreBuilder};
pub(crate) use turn::*;
pub(crate) use watch::{Batch, Watcher};
pub(crate) use watchers::*;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_archive;
#[cfg(test)]
mod tests_archive_edges;
#[cfg(test)]
mod tests_dispatch;
#[cfg(test)]
mod tests_model;
#[cfg(test)]
mod tests_skills;
#[cfg(test)]
mod tests_stream;
#[cfg(test)]
mod tests_turns;
#[cfg(test)]
mod tests_watcher;
