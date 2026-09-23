//! The snapshot projection (spec §8, ADR-0006): the ephemeral state the GUI
//! rebuilds from — session metadata + entry *metadata* (no payloads) + the
//! bounded OM observation log + live state + a durable cursor. Built on
//! demand by the core (app launch, tab open, reconnect); never a file.

use crate::MessageLane;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A workspace as identity (spec §8: no local paths as identity; `cwd` is
/// the on-host root the tools resolve relative paths against, §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub cwd: String,
}

/// Session metadata (the snapshot's (1) piece, spec §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    /// The owning workspace's id.
    pub workspace: String,
    /// The display name (the file header's title; adjective-noun for fresh
    /// top-level sessions, "Sub-agent: …" for children).
    pub title: Option<String>,
    /// The creator session (a sub-agent's parent); absent for top-level.
    pub parent: Option<String>,
    /// The header's created timestamp (epoch ms).
    pub created: u64,
    /// The active branch's leaf entry id.
    pub leaf: Option<String>,
    pub model: Option<String>,
    /// The last assistant entry's usage.
    pub usage: Option<crate::Usage>,
    /// The session is archived (ADR-0005): its file lives in the
    /// workspace's `archive/` dir; the GUI's archive folder lists it.
    #[serde(default)]
    pub archived: bool,
}

/// One entry's metadata: everything the entry tree needs, no payload
/// (spec §8: the snapshot carries no entry payloads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryMeta {
    pub id: String,
    pub parent: Option<String>,
    pub kind: String,
    pub timestamp: u64,
    /// The serialized line size in bytes (the GUI's virtualization sizes).
    pub size: u64,
    /// The first line of the entry's text, truncated (the tree's preview).
    pub preview: String,
    /// Set on compaction records (spec §3).
    pub first_kept: Option<String>,
    /// `Ok` is omitted on the wire — it is the near-universal case and the
    /// type's default; the spec's field stays present via the type.
    #[serde(default, skip_serializing_if = "is_ok_status")]
    pub status: EntryStatus,
}
/// Per-entry status (spec §8 entry metadata): in the v0 format the only
/// per-entry state is a cut partial — an assistant entry recorded as
/// `interrupted` (spec §6/§7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    #[default]
    Ok,
    Interrupted,
}

fn is_ok_status(s: &EntryStatus) -> bool {
    *s == EntryStatus::Ok
}

/// A paged-read window (spec §8: `entries {range}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRange {
    pub start: usize,
    pub count: usize,
}
/// A sidecar-blob pointer (ADR-0005 hardening 2): the payload lives in a
/// zstd-compressed file, the entry carries this. Kept protocol-owned so the
/// page-read types never import core types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub id: String,
    pub size: u64,
    pub hash: String,
}

/// One entry for a page read — unlike `EntryMeta`, the payload is included
/// (the GUI renders the visible page's content; blobs stream on demand).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewEntry {
    pub id: String,
    pub parent: Option<String>,
    pub kind: String,
    pub timestamp: u64,
    pub payload: Value,
    /// A sidecar pointer (ADR-0005) when the payload lives out-of-band.
    pub blob: Option<BlobRef>,
    pub first_kept: Option<String>,
}

/// A message sitting on a lane, as the GUI's vertical queue shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedItem {
    pub text: String,
    pub lane: MessageLane,
}

/// The session's turn state (spec §8 live state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Idle,
    Running,
}

/// The live, non-persisted part of the snapshot (spec §8 (4)): the pending
/// lane, the turn state, and the sub-agent set (empty until #23 wires it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveState {
    pub queue: Vec<QueuedItem>,
    pub turn: TurnState,
    /// The session's children (structured state — the GUI's sub-agent
    /// panel); empty for child sessions (the depth cap is structural).
    pub subagents: Vec<crate::SubagentInfo>,
    /// The session's tasks (per-session store, spec §5.3) — the GUI's
    /// tasks panel; active ones carry their resume contract.
    pub tasks: Vec<Value>,
}

/// The session's OM gauge (ticket #22): observation tokens against the
/// session's configured Reflector threshold, plus the unobserved pending
/// tokens. The status bar renders the ratio when idle and the activity
/// (the `om_status` event) while a run is in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmSnapshot {
    pub observation_tokens: u32,
    pub pending_tokens: u32,
    /// The configured Reflector threshold (the gauge's denominator).
    pub reflector_threshold: u32,
}

/// The ephemeral snapshot (spec §8): metadata skeleton + bounded OM + live
/// state + cursor. `om` is the session's current OM gauge (ticket #22).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub workspace: Workspace,
    pub session: SessionMeta,
    pub entries: Vec<EntryMeta>,
    pub om: OmSnapshot,
    pub live: LiveState,
    /// The last entry id: a durable cursor for `entries-since` reads.
    pub cursor: String,
}
