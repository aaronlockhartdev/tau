//! The four core tools (spec §5.4): read / write / edit / bash. Paths are
//! absolute or workspace-relative (relative resolves against the workspace
//! root — tools are not sandboxed, ADR-0007). `read`/`edit` are
//! hash-anchored per the pi-better-edit scheme (§5.4).

use crate::hashline;
use crate::provider::{ToolKind, ToolSpec};
use serde_json::json;
use std::path::{Path, PathBuf};

/// One tool call as the model emitted it (`arguments` is raw JSON).
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

/// The tool's output, as the model will see it (errors are results, not
/// panics — the model reads the diagnostic and recovers, spec §5.4).
pub type ToolOutput = String;

/// The model-facing definitions of the four core tools.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: ToolKind::Function,
            name: "read".into(),
            description: "Read a text file as hash-anchored lines: each line is \
                 'HASH│content' (HASH = 3 chars). The hash is the anchor you pass \
                 to edit. Optional offset (1-based) and limit page long files."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 1},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "write".into(),
            description: "Write (create or replace) a text file with the given content.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "edit".into(),
            description: "Replace the line range [from, to] (inclusive) of a file \
                 with content. from/to are the 3-char hash anchors copied from read \
                 output (the 3 chars before │), bare. Stale or ambiguous anchors are \
                 rejected — re-read the file for fresh anchors. Pass empty content to \
                 delete the range."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "from": {"type": "string"},
                    "to": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "from", "to", "content"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "bash".into(),
            description: "Run a shell command with the workspace as the working \
                 directory. Returns the exit code, stdout, and stderr."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["command"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "recall".into(),
            description:
                "Browse the raw session entries an observation group covers. Pass the group id (16 hex digits, from the observation log). A compacted sub-agent passes scope: \"parent\" to browse the parent session's raw history (its frozen prefix points there). Returns the entries in the group's range."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "group": {"type": "string"},
                    "scope": {"type": "string", "enum": ["self", "parent"]}
                },
                "required": ["group"]
            }),
        },
    ]
}

/// The parent-side sub-agent tools (spec §5.3): present on non-child
/// sessions only — the depth cap (a child cannot spawn) is structural.
pub fn subagent_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_spawn".into(),
            description: "Spawn a sub-agent that works the brief in its own session and reports back via parent_notify. Returns its name — how you address it in the other sub-agent tools. If the work is already a task, pass task — the child starts with it assigned; a separate assign costs an extra round trip. context_mode: fresh (default) = no parent history; compacted = the parent's observation log as a frozen context prefix; fork = a branched copy of the parent session.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["general"]},
                    "brief": {"type": "string"},
                    "context_mode": {"type": "string", "enum": ["fresh", "compacted", "fork"]},
                    "task": {"type": "string"}
                },
                "required": ["type", "brief"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_message".into(),
            description: "Message a sub-agent by name (its displayed title; its session id is accepted too). A running child takes it on its steering lane; a non-running child is resumed with it. Omit text for a pure resume (\"continue from where you stopped\").".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "text": {"type": "string"}
                },
                "required": ["name"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_stop".into(),
            description: "Stop a running sub-agent by name (its displayed title; its session id is accepted too): its in-flight stream is cut (the partial is kept) and it ends in the stopped state — it stays resumable. A sub-agent that is not running returns its current state (already parked/done/failed/stopped) and is left alone.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "subagent_state".into(),
            description: "Inspect a sub-agent by name (its displayed title; its session id is accepted too): lifecycle state, what it is waiting for, its last message, usage, and its session id.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
    ]
}

/// The child-side tool (spec §5.3): the only channel back to the parent.
/// done:true requires a structured output and ends the child; a note keeps
/// it parked. waiting_on declares what a parking child waits for (the nudge
/// fork: done / a valid declaration / failed).
pub fn parent_notify_spec() -> ToolSpec {
    ToolSpec {
        kind: ToolKind::Function,
        name: "parent_notify".into(),
        description: "Notify your parent. done:true with an object output finishes the task (the output is the result handed back). Without done, the note parks you; waiting_on declares what you wait for: parent | user | subagent.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "done": {"type": "boolean"},
                "output": {"type": "object"},
                "waiting_on": {"type": "string", "enum": ["parent", "user", "subagent"]}
            },
            "required": ["text"]
        }),
    }
}

/// The seven task tools (spec §5.4): free text + ids in; the core enforces
/// the state machines, the model never sees the enums. Available in every
/// session — a parent can complete small tasks itself (ADR-0001).
pub fn task_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_create".into(),
            description: "Create a task in this session: a title, an ordered list of steps (each with its expected output), and acceptance criteria. Returns the task id.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "steps": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "expected_output": {"type": "string"}}, "required": ["text", "expected_output"]}},
                    "criteria": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["title"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_assign".into(),
            description: "Assign a task to a worker session: its record copies into that session, which becomes the live one; this session's copy becomes a status pointer. worker is a session id. Prefer passing the task at spawn time; use this when the worker already exists.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "worker": {"type": "string"}
                },
                "required": ["task", "worker"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_start".into(),
            description: "Start working a task (pending, or unblock it).".into(),
            parameters: json!({
                "type": "object",
                "properties": {"task": {"type": "string"}},
                "required": ["task"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_evidence".into(),
            description: "Record evidence for a criterion: a summary, optionally a reproducible command and an artifact, and whether it passed. A task is done only when every criterion has passing evidence.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "criterion": {"type": "string"},
                    "summary": {"type": "string"},
                    "command": {"type": "string"},
                    "artifact": {"type": "string"},
                    "passed": {"type": "boolean"},
                    "step": {"type": "string"}
                },
                "required": ["task", "criterion", "summary"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_block".into(),
            description: "Block a task with a free-text reason and, optionally, what is needed to unblock it.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "reason": {"type": "string"},
                    "needs": {"type": "string"}
                },
                "required": ["task", "reason"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_finish".into(),
            description: "Finish a task. Fails unless every criterion is satisfied by passing evidence; force:true with a reason is the documented escape.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "force": {"type": "boolean"},
                    "reason": {"type": "string"}
                },
                "required": ["task"]
            }),
        },
        ToolSpec {
            kind: ToolKind::Function,
            name: "task_cancel".into(),
            description: "Cancel a task (with an optional reason).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "reason": {"type": "string"}
                },
                "required": ["task"]
            }),
        },
    ]
}

/// A non-child session's tool set: the core tools + the sub-agent tools +
/// the task tools.
pub fn agent_tool_specs() -> Vec<ToolSpec> {
    let mut v = tool_specs();
    v.extend(subagent_tool_specs());
    v.extend(task_tool_specs());
    v
}

/// The worker side of the task tools: a child works the record its parent
/// assigned — it starts, evidences, blocks, and finishes it (spec §5.3).
pub fn worker_task_tool_specs() -> Vec<ToolSpec> {
    const WORKER: &[&str] = &["task_start", "task_evidence", "task_block", "task_finish"];
    task_tool_specs()
        .into_iter()
        .filter(|s| WORKER.contains(&s.name.as_str()))
        .collect()
}

/// A child session's tool set: the core tools + parent_notify + the
/// worker-side task tools — no sub-agent tools (a child cannot spawn,
/// ADR-0001 depth cap), and no create/assign/cancel: a child is a leaf, so
/// a task it created itself would be invisible to the parent and would die
/// with it (spec §5.3).
pub fn child_tool_specs() -> Vec<ToolSpec> {
    let mut v = tool_specs();
    v.push(parent_notify_spec());
    v.extend(worker_task_tool_specs());
    v
}

/// Dispatch one tool call. Never panics on bad input — the diagnostic is the
/// result (the model's only recovery path).
pub async fn dispatch(cwd: &Path, call: &ToolCall) -> ToolOutput {
    match call.name.as_str() {
        "read" => read(cwd, &call.args).await,
        "write" => write(cwd, &call.args).await,
        "edit" => edit(cwd, &call.args).await,
        "bash" => bash(cwd, &call.args).await,
        other => format!("unknown tool: {other}"),
    }
}

mod impls;
use impls::*;

#[cfg(test)]
mod tests;
