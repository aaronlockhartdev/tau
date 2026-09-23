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
pub mod payload;
pub mod snapshot;

use crate::payload::{ResumeContract, Task};
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
    File {
        file: FileText,
    },
    Files {
        files: Vec<FileEntry>,
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

    SessionList {
        workspace: String,
    },
    SessionNew {
        workspace: String,
        title: Option<String>,
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
}

/// An event pushed from the core to clients (spec §8). Every event carries
/// the workspace id and, where session-scoped, the session id, over one
/// multiplexed channel; stream/tool events correlate by `call_id`
/// (idempotent-cumulative, #13) and every id is stable across retries.
/// The spec §8 HITL request/response pair exists in this shape with zero
/// live request types (ADR-0007: transparency, not enforcement) — a
/// future security ticket adds concrete types, not a new channel.
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
    /// The session's OM activity (the turn-end Observer/Reflector run):
    /// `observing`/`reflecting` while in flight, `idle` when it finishes —
    /// the status bar's om gauge.
    OmStatus {
        workspace: String,
        session: String,
        kind: OmStatusKind,
    },
    System {
        workspace: String,
        session: Option<String>,
        kind: SystemEventKind,
    },
    SubagentEvent {
        workspace: String,
        /// The parent session (the child is identified inside `kind`).
        session: String,
        kind: SubagentEventKind,
    },
    /// The session's task records changed (spec §8): full-state
    /// replacement of the task list — idempotent by construction, a lost
    /// batch self-heals on the next snapshot. Emitted on every task
    /// mutation (the model's tools and the app's task commands), so the
    /// GUI's tasks tab is event-driven, never polled.
    TaskChanged {
        workspace: String,
        session: String,
        tasks: Vec<Task>,
    },
    /// The workspace's skill registry changed (the file watcher, ticket
    /// #31): full-state replacement, idempotent by construction; a lost
    /// batch self-heals on the next `skill_list`. Session-less: the
    /// registry is per workspace, not per session.
    SkillListChanged {
        workspace: String,
        skills: Vec<SkillInfo>,
    },
    /// A watched dir of the workspace changed (the files-pane watcher,
    /// ticket #32): `changed` holds the stale dir paths (workspace-relative)
    /// as stale-dir invalidation — the client refetches the affected listed
    /// dirs, it never receives a tree. Session-less: the tree is per
    /// workspace, not per session. A lost batch self-heals on the next
    /// expand (every fetch is a fresh read).
    FileTreeChanged {
        workspace: String,
        changed: Vec<String>,
    },
}

/// Sub-agent-group events (spec §8): lifecycle transitions and wakes.
/// Idempotent-cumulative — each carries the full state of one handle, so
/// a lost batch self-heals on the next snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubagentEventKind {
    Spawned {
        handle: String,
        child: String,
        agent_type: String,
        context_mode: ContextMode,
        /// The child's header title, so the GUI can name the session stub
        /// at spawn time without a roundtrip.
        title: String,
    },
    /// A lifecycle transition (all five states, incl. stop and its
    /// provenance); `detail` carries the state's payload (done's output,
    /// failed's reason, idle's waiting_on, stopped's by).
    State {
        handle: String,
        child: String,
        state: String,
        detail: Option<Value>,
        note: Option<String>,
    },
    /// A child notification reached the parent (done / failed / a
    Notified {
        child: String,
        /// done | failed | waiting | stopped
        wake: String,
        text: String,
        output: Option<Value>,
    },
}

/// A child's structured state (the protocol's mirror of the core's 5-state
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentInfo {
    pub handle: String,
    /// The child's session id — a child is an ordinary session.
    pub child: String,
    pub agent_type: String,
    pub context_mode: ContextMode,
    /// running | idle | done | failed | stopped
    pub state: String,
    pub waiting_on: Option<String>,
    pub last_message: Option<String>,
    pub usage: Option<Usage>,
    pub task: Option<Task>,
    pub resume_contract: Option<ResumeContract>,
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
/// The session's OM activity (the turn-end run's kind; the gauge's
/// `busy` states and its end).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmStatusKind {
    Observing,
    Reflecting,
    Idle,
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
    /// Prompt tokens served from the server's prefix cache (0 = no hit data).
    pub cached_prompt_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            Command::SessionRename {
                session: "s1".into(),
                title: "Brave Otter".into(),
            },
            Command::SessionSetModel {
                session: "s1".into(),
                model: "m2".into(),
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
            Command::SessionArchive {
                session: "s1".into(),
            },
            Command::SessionRestore {
                workspace: "w1".into(),
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
            Command::SkillList {
                workspace: "w1".into(),
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
            Command::FileList {
                workspace: "w1".into(),
                path: "src".into(),
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
                    cached_prompt_tokens: 0,
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
            Event::OmStatus {
                workspace: "w1".into(),
                session: "s1".into(),
                kind: OmStatusKind::Observing,
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
            Event::SessionEvent {
                workspace: "w1".into(),
                session: "s1".into(),
                kind: SessionEventKind::Compaction {
                    entry: "00000043".into(),
                    first_kept: "00000044".into(),
                },
            },
            Event::System {
                workspace: "w1".into(),
                session: None,
                kind: SystemEventKind::ProviderChanged,
            },
            Event::SubagentEvent {
                workspace: "w1".into(),
                session: "s1".into(),
                kind: SubagentEventKind::Spawned {
                    handle: "s1-1".into(),
                    child: "s2".into(),
                    agent_type: "general".into(),
                    context_mode: ContextMode::Compacted,
                    title: "rusty-nail".into(),
                },
            },
            Event::SubagentEvent {
                workspace: "w1".into(),
                session: "s1".into(),
                kind: SubagentEventKind::State {
                    handle: "s1-1".into(),
                    child: "s2".into(),
                    state: "done".into(),
                    detail: Some(json!({ "output": { "result": "ok" } })),
                    note: None,
                },
            },
            Event::SubagentEvent {
                workspace: "w1".into(),
                session: "s1".into(),
                kind: SubagentEventKind::Notified {
                    child: "s2".into(),
                    wake: "done".into(),
                    text: "finished".into(),
                    output: Some(json!({ "result": "ok" })),
                },
            },
            Event::TaskChanged {
                workspace: "w1".into(),
                session: "s1".into(),
                tasks: vec![payload::Task {
                    id: "t-1".into(),
                    title: "do it".into(),
                    status: "pending".into(),
                    steps: vec![],
                    criteria: vec![],
                    evidence: vec![],
                    blockers: vec![],
                    decisions: vec![],
                    notes: vec![],
                    worker: None,
                    created_in: None,
                    updated: 0,
                }],
            },
            Event::SkillListChanged {
                workspace: "w1".into(),
                skills: vec![SkillInfo {
                    name: "s".into(),
                    description: "d".into(),
                    location: "/p/.agents/skills/s/SKILL.md".into(),
                    model_invocation: true,
                }],
            },
            Event::FileTreeChanged {
                workspace: "w1".into(),
                changed: vec!["src".into(), "src/core".into()],
            },
        ];
        for ev in &events {
            let json = serde_json::to_string(ev).unwrap();
            let back: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, ev, "round-trip mismatch for {json}");
        }
    }

    /// Every `CommandOutput` variant serializes to JSON. Regression: the
    /// list variants were newtypes, and an internally tagged enum cannot
    /// serialize a sequence payload — `workspace_list` failed on the wire
    /// and the app launched with zero workspaces.
    #[test]
    fn command_output_variants_roundtrip() {
        let ws = snapshot::Workspace {
            id: "w1".into(),
            name: "tau".into(),
            cwd: "/tmp/tau".into(),
        };
        let meta = snapshot::SessionMeta {
            id: "s1".into(),
            workspace: "w1".into(),
            title: None,
            parent: None,
            created: 1,
            leaf: None,
            model: None,
            usage: None,
            archived: true,
        };
        let entry = snapshot::ViewEntry {
            id: "e1".into(),
            parent: None,
            kind: "message".into(),
            timestamp: 1,
            payload: json!({ "text": "hi" }),
            blob: None,
            first_kept: None,
        };
        let snap = snapshot::Snapshot {
            workspace: ws.clone(),
            session: meta.clone(),
            entries: vec![],
            om: snapshot::OmSnapshot {
                observation_tokens: 18_000,
                pending_tokens: 1_000,
                reflector_threshold: 40_000,
            },
            live: snapshot::LiveState {
                queue: vec![],
                turn: snapshot::TurnState::Idle,
                subagents: vec![],
                tasks: vec![],
            },
            cursor: "e1".into(),
        };
        let provider = ProviderInfo {
            name: "vllm".into(),
            base_url: "http://localhost:8000/v1".into(),
            models: vec!["m1".into()],
        };
        let agent = AgentType {
            name: "general".into(),
            description: "the built-in".into(),
            builtin: true,
        };
        let sub = SubagentInfo {
            handle: "h1".into(),
            child: "s2".into(),
            agent_type: "general".into(),
            context_mode: ContextMode::Fresh,
            state: "running".into(),
            waiting_on: None,
            last_message: None,
            usage: None,
            task: None,
            resume_contract: None,
        };
        let file = FileText {
            text: "x".into(),
            truncated: false,
        };
        let outputs = vec![
            CommandOutput::None,
            CommandOutput::Workspace { workspace: ws },
            CommandOutput::Workspaces { workspaces: vec![] },
            CommandOutput::Session { session: meta },
            CommandOutput::Sessions { sessions: vec![] },
            CommandOutput::Snapshot { snapshot: snap },
            CommandOutput::Entries {
                entries: vec![entry],
            },
            CommandOutput::Providers {
                providers: vec![provider],
            },
            CommandOutput::Agents {
                agents: vec![agent],
            },
            CommandOutput::Skills {
                skills: vec![SkillInfo {
                    name: "s".into(),
                    description: "d".into(),
                    location: "/p/.agents/skills/s/SKILL.md".into(),
                    model_invocation: false,
                }],
            },
            CommandOutput::Subagents {
                subagents: vec![sub.clone()],
            },
            CommandOutput::Subagent { subagent: sub },
            CommandOutput::File { file },
            CommandOutput::Files {
                files: vec![FileEntry {
                    name: "d".into(),
                    path: "d".into(),
                    dir: true,
                    size: 0,
                }],
            },
        ];
        for out in &outputs {
            let json = serde_json::to_string(out).unwrap();
            let back: CommandOutput = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, out, "round-trip mismatch for {json}");
        }
    }
}
