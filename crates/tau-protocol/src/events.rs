use super::*;
use crate::payload::{ResumeContract, Task};
use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
