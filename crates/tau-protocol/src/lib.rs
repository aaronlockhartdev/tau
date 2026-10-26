//! The core↔GUI message boundary (spec §8, ADR-0006): one crate of
//! tagged-union messages — commands in, events out — JSON-serializable and
//! free of Tauri types. Tauri commands/events are transport #1; a future
//! `tau serve` carries the same types over a network transport.
//!
//! v0 narrowing (map #14, §14 U4): the command surface is the full §8 list;
//! the event list carries only what the built core can produce — turn,
//! queue, session, and system groups; the sub-agent and om event groups
//! are absent until their producer tickets land. The document
//! narrows as features finalize (ADR-0006).
#![cfg_attr(test, allow(clippy::unwrap_used), allow(clippy::panic))]

pub mod diagnostic;
pub mod kind;
pub mod payload;
pub mod snapshot;

pub use diagnostic::{Diagnostic, StreamEnd};
pub use kind::EntryKind;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A message lane (spec §7; the GUI composer's 3-way selector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageLane {
    Force,
    Steering,
    FollowUp,
}

/// A spawn context mode (spec §5.1; fresh is the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    Fresh,
    Compacted,
    Fork,
}

/// One provider entry as the GUI sees it (no key material, §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
}

/// An agent type (spec §5.5: built-in `general` + `.md` files; discovery
/// beyond the built-in lands with ticket #24).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentType {
    pub name: String,
    pub description: String,
    pub builtin: bool,
}

/// A skill as the GUI sees it (the autocomplete's data source): `location`
/// is the absolute path to the `SKILL.md`; a skill with
/// `model_invocation: false` is absent from the catalog and the dropdown
/// is its only door.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub location: String,
    pub model_invocation: bool,
}

/// A file read for the GUI (the left pane's files tab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileText {
    pub text: String,
    pub truncated: bool,
}

/// One entry of a directory listing (the files pane's tree, ticket #32).
/// `path` is relative to the workspace root (the root itself is `.`), so
/// the GUI never sees host paths and can refetch any listed dir by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// In .gitignore: shown in the tree, greyed (the display set stays the
    /// filesystem walk — ignore status only changes styling).
    pub ignored: bool,
    /// Git change state for the entry; None outside a git workspace.
    pub git: Option<FileGit>,
}

/// A file's git change state (the tree's tint + line-change stats).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileGit {
    pub state: FileGitState,
    /// `git diff --numstat` counts; None when unmeasurable (binary file).
    pub added: Option<u32>,
    pub deleted: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileGitState {
    Modified,
    Untracked,
    Deleted,
}

/// A workspace's git status (the files tab's M/A/D badges and the
/// peek sheet's file switcher). Paths are relative to the workspace root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: String,
    pub modified: Vec<String>,
    pub untracked: Vec<String>,
    pub deleted: Vec<String>,
}

/// A dispatch error, serializable so any transport can return it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtocolError {
    /// The command is part of the v0 surface but its producer ticket has not
    /// landed yet (dynamic providers: v0 is config-file-driven).
    Unsupported {
        message: String,
    },
    NotFound {
        what: String,
    },
    Other {
        message: String,
    },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::Unsupported { message } | Self::Other { message } => message.as_str(),
            Self::NotFound { what } => what.as_str(),
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ProtocolError {}

/// The result of a command; each variant is the typed shape the Svelte side
/// compiles against (ADR-0006: no richer in-process API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// The snapshot variant is the payload; the rest are small. Boxing the
// large variants would buy a few bytes at the cost of an allocation per
// command output on a surface that serializes to JSON anyway.
#[allow(clippy::large_enum_variant)]
pub enum CommandOutput {
    None,
    Workspace {
        workspace: snapshot::Workspace,
    },
    // Every struct-payload variant is a struct variant, not a newtype:
    // an internally tagged enum merges the tag into the payload's map, so a
    // newtype would flatten the payload's fields to the top level. The named
    // fields are the GUI's contract (protocol.ts mirrors them).
    Workspaces {
        workspaces: Vec<snapshot::Workspace>,
    },
    Session {
        session: snapshot::SessionMeta,
    },
    Sessions {
        sessions: Vec<snapshot::SessionMeta>,
    },
    Snapshot {
        snapshot: snapshot::Snapshot,
    },
    Entries {
        entries: Vec<snapshot::ViewEntry>,
    },
    /// A sidecar blob's decoded payload (ADR-0005).
    Blob {
        payload: Value,
    },
    Providers {
        providers: Vec<ProviderInfo>,
    },
    Agents {
        agents: Vec<AgentType>,
    },
    Skills {
        skills: Vec<SkillInfo>,
    },
    Subagents {
        subagents: Vec<SubagentInfo>,
    },
    Subagent {
        subagent: SubagentInfo,
    },
    /// The child's activity window (#203): its observation-log tail and the
    /// un-observed raw tail, projected compact and tail-capped. Renderer-only
    /// (not an LLM tool) — zero model-context cost.
    SubagentActivity {
        activity: payload::ChildActivity,
    },
    File {
        file: FileText,
    },
    Files {
        files: Vec<FileEntry>,
    },
    GitStatus {
        status: GitStatus,
    },
    GitShow {
        text: String,
    },
}

/// A command from any transport client to the core (spec §8, settled #10).
/// Every variant carries the workspace id and, where session-scoped, the
/// session id — identity is the workspace object, never a bare path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    WorkspaceOpen {
        /// The workspace root on the host (a tool-argument path, §5.4 — not
        /// identity; the core derives the workspace object from it).
        cwd: String,
    },
    WorkspaceList,
    /// Close (archive) the workspace (B7): its `open` flag in the workspace
    /// index is persisted, so a closed tab stays closed across a restart.
    /// The workspace's sessions remain on disk and the workspace re-opens.
    WorkspaceClose {
        workspace: String,
    },

    SessionList {
        workspace: String,
    },
    SessionNew {
        workspace: String,
        title: Option<String>,
        /// Replace the base prompt line (the headless frame seam; `None`
        /// keeps the default). Context files and the skills catalog still
        /// append after it.
        base_prompt: Option<String>,
    },
    /// Rename the session: the new title replaces the header's (the file
    /// keeps its id; the title is a display name).
    SessionRename {
        session: String,
        title: String,
    },
    /// Switch the session's model (a plain string; provider resolution at
    /// turn time handles unknown models). Same model is a no-op; otherwise
    /// the meta's model is set and a quiet system entry records the change.
    SessionSetModel {
        session: String,
        model: String,
    },
    SessionOpen {
        session: String,
    },
    SessionClose {
        session: String,
    },
    SessionDelete {
        session: String,
    },
    /// Archive a top-level session (ADR-0005): its file is zstd-compressed
    /// into the workspace's `archive/` dir, the metadata carries `archived`
    /// (which the GUI's archive folder lists), and every sub-agent child of
    /// the session archives with it. A child id is refused (archive the
    /// parent); off the live read/write path — a running session refuses.
    SessionArchive {
        session: String,
    },
    /// Restore an archived session: the file moves back from `archive/` to
    /// `sessions/` (decompressed) and its `archived` flag clears; the
    /// session's sub-agent children, archived with it, restore with it.
    SessionRestore {
        workspace: String,
        session: String,
    },
    /// Create a new branch from entry `at` within the same session file
    /// (spec §8: fork is a branch, not a new session).
    SessionFork {
        session: String,
        at: String,
    },
    /// Move the session's active branch to entry `at`.
    SessionBranch {
        session: String,
        at: String,
    },
    SessionSnapshot {
        session: String,
    },
    /// Paged read (spec §8: no full-dump command). Exactly one of `since`
    /// (a durable cursor: the last entry id already seen) or `range`
    /// (`start` + `count` offsets).
    SessionEntries {
        session: String,
        since: Option<String>,
        range: Option<snapshot::EntryRange>,
    },
    /// Decode a sidecar blob (ADR-0005) of a paged-read entry: an
    /// oversized payload lives out-of-band in the workspace's `blobs/`.
    BlobRead {
        workspace: String,
        /// The blob file name (the owning entry's id).
        id: String,
        /// The pointer's hash: the read verifies it (ADR-0005).
        hash: String,
    },

    MessageSend {
        session: String,
        text: String,
        lane: MessageLane,
    },
    /// Interrupt the session's in-flight work: the stream is cut, the
    /// partial stands as `interrupted` (spec §7).
    MessageStop {
        session: String,
    },
    /// Delete a queued message by its position in the session's queue (the
    /// global index the queue pane shows, ticket #223): the item leaves the
    /// queue and never runs.
    QueueItemRemove {
        session: String,
        index: usize,
    },

    SubagentTypes,
    SubagentList {
        session: String,
    },
    SubagentState {
        handle: String,
    },
    SubagentSpawn {
        session: String,
        agent_type: String,
        brief: String,
        context_mode: ContextMode,
    },
    SubagentMessage {
        handle: String,
        text: Option<String>,
    },
    SubagentStop {
        handle: String,
    },
    /// The child's activity window (#203), fetched by handle like the other
    /// handle-scoped sub-agent commands. Renderer-only (not an LLM tool).
    SubagentActivity {
        handle: String,
    },

    /// The workspace's skill registry (discovery at command time; the GUI
    /// fetches on workspace open/switch and caches per workspace).
    SkillList {
        workspace: String,
    },

    ProviderList,
    ProviderAdd {
        name: String,
        base_url: String,
        key_env: String,
        models: Vec<String>,
    },
    ProviderSet {
        name: String,
        base_url: String,
        key_env: String,
        models: Vec<String>,
    },
    ProviderDelete {
        name: String,
    },

    FileRead {
        workspace: String,
        /// Absolute, or relative to the workspace root.
        path: String,
        /// 0-based first line to include.
        offset: Option<usize>,
        /// Maximum lines to return.
        limit: Option<usize>,
    },
    /// List one directory of the workspace's tree (ticket #32): the pane
    /// fetches a dir on expand — the lazy, invalidation-driven tree.
    FileList {
        workspace: String,
        /// The directory to list: absolute, or relative to the workspace
        /// root (`.` for the root itself).
        path: String,
    },
    /// Stop watching a directory of the workspace's tree (#115): the pane
    /// sends this on collapse so the watcher drops a dir it no longer shows.
    FileUnlist {
        workspace: String,
        /// The directory to stop watching: absolute, or relative to the
        /// workspace root (`.` for the root itself).
        path: String,
    },

    /// The workspace's git status (files tab badges, peek sheet switcher).
    GitStatus {
        workspace: String,
    },
    /// A file's content at a git revision (default HEAD) — the
    /// file-content view's diff base.
    GitShow {
        workspace: String,
        /// Absolute, or relative to the workspace root.
        path: String,
    },
}

/// An event pushed from the core to clients (spec §8). Every event carries
/// the workspace id and, where session-scoped, the session id, over one
/// multiplexed channel; stream/tool events correlate by `call_id`
/// (idempotent-cumulative, #13) and every id is stable across retries.
/// The spec §8 HITL request/response pair exists in this shape with zero
/// live request types (ADR-0007: transparency, not enforcement) — a
/// future security ticket adds concrete types, not a new channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
/// Token usage at a call's completion (mirrors the provider's shape, kept
/// protocol-owned so the crate stays transport-free).
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    /// Prompt tokens served from the server's prefix cache (0 = no hit data).
    pub cached_prompt_tokens: u64,
}

mod events;
pub use events::{
    Event, OmStatusKind, SessionEventKind, SubagentEventKind, SubagentInfo, SystemEventKind,
};
#[cfg(test)]
mod tests;
