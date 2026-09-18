//! The core↔GUI message boundary (spec §8, ADR-0006): one crate of
//! tagged-union messages — commands in, events out — JSON-serializable and
//! free of Tauri types. Tauri commands/events are transport #1; a future
//! `tau serve` carries the same types over a network transport.
//!
//! v0 narrowing (map #14, §14 U4): the command surface is the full §8 list;
//! the event list carries only what the built core can produce — turn,
//! queue, session, and system groups. Sub-agent, task, and om event groups
//! are absent until tickets #22/#23/#24 wire their producers; the document
//! narrows as features finalize (ADR-0006).

pub mod coalesce;
pub mod snapshot;

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

/// A file read for the GUI (the left pane's files tab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileText {
    pub text: String,
    pub truncated: bool,
}

/// A dispatch error, serializable so any transport can return it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtocolError {
    /// The command is part of the v0 surface but its producer ticket has not
    /// landed yet (subagents: #23, tasks: #24, dynamic providers: v0 is
    /// config-file-driven).
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
    Workspace(snapshot::Workspace),
    Workspaces(Vec<snapshot::Workspace>),
    Session(snapshot::SessionMeta),
    Sessions(Vec<snapshot::SessionMeta>),
    Snapshot(snapshot::Snapshot),
    Entries(Vec<snapshot::ViewEntry>),
    Providers(Vec<ProviderInfo>),
    Agents(Vec<AgentType>),
    File(FileText),
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

    SessionList {
        workspace: String,
    },
    SessionNew {
        workspace: String,
        title: Option<String>,
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

    TaskCreate {
        session: String,
        title: String,
    },
    TaskUpdate {
        session: String,
        task: String,
        note: String,
    },
    TaskAssign {
        session: String,
        task: String,
        worker: String,
    },
    TaskEvidence {
        session: String,
        task: String,
        criterion: String,
        summary: String,
        passed: Option<bool>,
    },
    TaskCancel {
        session: String,
        task: String,
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
}

/// An event pushed from the core to clients (spec §8). Every event carries
/// the workspace id and, where session-scoped, the session id, over one
/// multiplexed channel; stream/tool events correlate by `call_id`
/// (idempotent-cumulative, #13) and every id is stable across retries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A new LLM call starts; the GUI opens a live bubble for `call_id`.
    StreamStart {
        workspace: String,
        session: String,
        call_id: String,
    },
    /// A coalesced stream delta (text and/or reasoning appended since the
    /// last flush for this call).
    StreamDelta {
        workspace: String,
        session: String,
        call_id: String,
        text: String,
        reasoning: Option<String>,
    },
    /// The call ends and its assistant entry is committed; `interrupted`
    /// means the stream was cut (force/stop) and the partial stands (spec
    /// §6/§7). The GUI reconciles the live bubble against the entry.
    StreamEnd {
        workspace: String,
        session: String,
        call_id: String,
        interrupted: bool,
        usage: Option<Usage>,
    },
    /// A tool the model requested starts running (spec §8 tool events).
    ToolStart {
        workspace: String,
        session: String,
        call_id: String,
        tool_call_id: String,
        name: String,
    },
    /// A tool finishes; `output` is idempotent — the GUI replaces on
    /// receive, so a lost batch self-heals on the next page read.
    ToolEnd {
        workspace: String,
        session: String,
        call_id: String,
        tool_call_id: String,
        name: String,
        output: Value,
    },
    /// The full pending-lane state (steering on top, follow-up below —
    /// spec §7's vertical queue). Full-state replacement: idempotent by
    /// construction.
    Queue {
        workspace: String,
        session: String,
        items: Vec<snapshot::QueuedItem>,
    },
    SessionEvent {
        workspace: String,
        session: String,
        kind: SessionEventKind,
    },
    System {
        workspace: String,
        session: Option<String>,
        kind: SystemEventKind,
    },
}

/// Session-group events the built core produces (spec §8 session group).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEventKind {
    /// The active branch moved (fork/branch commands, §8).
    BranchMove { leaf: String },
    /// A compaction span was appended; `entry` is the compaction record and
    /// `first_kept` the first raw entry still in context (spec §3).
    Compaction { entry: String, first_kept: String },
}

/// System-group events (workspace and provider state; errors).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemEventKind {
    WorkspaceOpened {
        name: String,
        cwd: String,
    },
    /// Provider list changed; v0 providers are config-file-driven, so the
    /// only live producer is a config reload (absent in v0 — the variant
    /// stays for the §8 surface, emitted by nothing until then).
    #[allow(dead_code)]
    ProviderChanged,
    Error {
        message: String,
    },
}

/// Token usage at a call's completion (mirrors the provider's shape, kept
/// protocol-owned so the crate stays transport-free).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command variant round-trips through its golden JSON shape.
    #[test]
    fn command_variants_roundtrip() {
        let commands = vec![
            Command::WorkspaceOpen {
                cwd: "/tmp/w".into(),
            },
            Command::WorkspaceList,
            Command::SessionList {
                workspace: "w1".into(),
            },
            Command::SessionNew {
                workspace: "w1".into(),
                title: None,
            },
            Command::SessionOpen {
                session: "s1".into(),
            },
            Command::SessionClose {
                session: "s1".into(),
            },
            Command::SessionDelete {
                session: "s1".into(),
            },
            Command::SessionFork {
                session: "s1".into(),
                at: "00000042".into(),
            },
            Command::SessionBranch {
                session: "s1".into(),
                at: "00000042".into(),
            },
            Command::SessionSnapshot {
                session: "s1".into(),
            },
            Command::SessionEntries {
                session: "s1".into(),
                since: None,
                range: Some(snapshot::EntryRange {
                    start: 0,
                    count: 100,
                }),
            },
            Command::MessageSend {
                session: "s1".into(),
                text: "hi".into(),
                lane: MessageLane::Steering,
            },
            Command::MessageStop {
                session: "s1".into(),
            },
            Command::SubagentTypes,
            Command::SubagentList {
                session: "s1".into(),
            },
            Command::SubagentState {
                handle: "h1".into(),
            },
            Command::SubagentSpawn {
                session: "s1".into(),
                agent_type: "general".into(),
                brief: "do the thing".into(),
                context_mode: ContextMode::Fresh,
            },
            Command::SubagentMessage {
                handle: "h1".into(),
                text: None,
            },
            Command::SubagentStop {
                handle: "h1".into(),
            },
            Command::TaskCreate {
                session: "s1".into(),
                title: "t".into(),
            },
            Command::TaskUpdate {
                session: "s1".into(),
                task: "t1".into(),
                note: "n".into(),
            },
            Command::TaskAssign {
                session: "s1".into(),
                task: "t1".into(),
                worker: "h1".into(),
            },
            Command::TaskEvidence {
                session: "s1".into(),
                task: "t1".into(),
                criterion: "c1".into(),
                summary: "s".into(),
                passed: Some(true),
            },
            Command::TaskCancel {
                session: "s1".into(),
                task: "t1".into(),
            },
            Command::ProviderList,
            Command::ProviderAdd {
                name: "p".into(),
                base_url: "u".into(),
                key_env: "K".into(),
                models: vec!["m".into()],
            },
            Command::ProviderSet {
                name: "p".into(),
                base_url: "u".into(),
                key_env: "K".into(),
                models: vec!["m".into()],
            },
            Command::ProviderDelete { name: "p".into() },
            Command::FileRead {
                workspace: "w1".into(),
                path: "a/b.txt".into(),
                offset: None,
                limit: None,
            },
        ];
        for cmd in &commands {
            let json = serde_json::to_string(cmd).unwrap();
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, cmd, "round-trip mismatch for {json}");
        }
    }

    #[test]
    fn command_tag_shape() {
        let json = serde_json::to_string(&Command::MessageSend {
            session: "s1".into(),
            text: "hi".into(),
            lane: MessageLane::Force,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"message_send","session":"s1","text":"hi","lane":"force"}"#
        );
    }

    /// Every event variant round-trips; the multiplexed-channel envelope
    /// (workspace + session ids on every event) is the shape a lossy
    /// transport needs (#13).
    #[test]
    fn event_variants_roundtrip() {
        let events = vec![
            Event::StreamStart {
                workspace: "w1".into(),
                session: "s1".into(),
                call_id: "c1".into(),
            },
            Event::StreamDelta {
                workspace: "w1".into(),
                session: "s1".into(),
                call_id: "c1".into(),
                text: "partial".into(),
                reasoning: Some("thinking…".into()),
            },
            Event::StreamEnd {
                workspace: "w1".into(),
                session: "s1".into(),
                call_id: "c1".into(),
                interrupted: false,
                usage: Some(Usage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                }),
            },
            Event::ToolStart {
                workspace: "w1".into(),
                session: "s1".into(),
                call_id: "c1".into(),
                tool_call_id: "call_1".into(),
                name: "read".into(),
            },
            Event::ToolEnd {
                workspace: "w1".into(),
                session: "s1".into(),
                call_id: "c1".into(),
                tool_call_id: "call_1".into(),
                name: "read".into(),
                output: Value::String("the file".into()),
            },
            Event::Queue {
                workspace: "w1".into(),
                session: "s1".into(),
                items: vec![snapshot::QueuedItem {
                    text: "next".into(),
                    lane: MessageLane::Steering,
                }],
            },
            Event::SessionEvent {
                workspace: "w1".into(),
                session: "s1".into(),
                kind: SessionEventKind::BranchMove {
                    leaf: "00000042".into(),
                },
            },
            Event::System {
                workspace: "w1".into(),
                session: None,
                kind: SystemEventKind::WorkspaceOpened {
                    name: "w".into(),
                    cwd: "/tmp/w".into(),
                },
            },
            Event::System {
                workspace: "w1".into(),
                session: Some("s1".into()),
                kind: SystemEventKind::Error {
                    message: "boom".into(),
                },
            },
        ];
        for ev in &events {
            let json = serde_json::to_string(ev).unwrap();
            let back: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, ev, "round-trip mismatch for {json}");
        }
    }
}
