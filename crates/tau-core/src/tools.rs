//! The four core tools (spec §5.4): read / write / edit / bash. Paths are
//! absolute or workspace-relative (relative resolves against the workspace
//! root — tools are not sandboxed, ADR-0007). `read`/`edit` are
//! hash-anchored per the pi-better-edit scheme (§5.4).

use crate::hashline;
use crate::provider::ToolSpec;
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
/// panics — the model reads the diagnostic and recovers, spec §5.4). Plain
/// text is the common shape; `read` can also return an image block a
/// vision-capable endpoint can see (#34).
pub use tau_protocol::payload::ToolOutput;

/// A tool's result: the model-facing output plus an explicit ok/error verdict
/// set by the tool itself (which knows whether it succeeded) — the source of
/// truth for the GUI status indicator.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: ToolOutput,
    pub ok: bool,
}

impl ToolResult {
    #[must_use]
    pub fn ok(output: ToolOutput) -> Self {
        Self { output, ok: true }
    }

    #[must_use]
    pub fn err(output: ToolOutput) -> Self {
        Self { output, ok: false }
    }

    /// The wire status string for `ToolPayload.status`.
    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.ok { "ok" } else { "error" }
    }

    /// A typed verdict from a `Result<String, String>` source: `Ok` maps to a
    /// success, `Err` to a failure — a surface tool's `Result` becomes a
    /// `ToolResult` without the dispatcher re-reading the text.
    pub(crate) fn from_result(r: Result<String, String>) -> Self {
        match r {
            Ok(s) => Self::ok(s.into()),
            Err(e) => Self::err(e.into()),
        }
    }
}

/// The model-facing definitions of the four core tools.
#[must_use]
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: "function".into(),
            name: "read".into(),
            description: "Read a file. Text files come back as hash-anchored lines: \
                 each line is 'HASH│content' (HASH = 3 chars). The hash is the anchor \
                 you pass to edit. Optional offset (1-based) and limit page long files. \
                 Image files (png, jpg, gif, webp, bmp, tiff) come back as an image you \
                 can see on vision-capable endpoints — use read to view an image."
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
            kind: "function".into(),
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
            kind: "function".into(),
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
            kind: "function".into(),
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
            kind: "function".into(),
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
#[must_use]
pub fn subagent_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            kind: "function".into(),
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
            kind: "function".into(),
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
            kind: "function".into(),
            name: "subagent_stop".into(),
            description: "Stop a running sub-agent by name (its displayed title; its session id is accepted too): its in-flight stream is cut (the partial is kept) and it ends in the stopped state — it stays resumable. A sub-agent that is not running returns its current state (already resting/done/failed/stopped) and is left alone.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
        ToolSpec {
            kind: "function".into(),
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

/// The child-side tool (spec §5.2, ADR-0001 revision): the only channel
/// back to the parent. `done:true` requires a structured output and records
/// it as the result an observed `done` will carry (a secondary hint — the
/// turn-end observation confirms it against the child's plan file, or
/// overrides it); a note records a declared `waiting_on` the child's rest
/// rides as its badge.
#[must_use]
pub fn parent_notify_spec() -> ToolSpec {
    ToolSpec {
        kind: "function".into(),
        name: "parent_notify".into(),
        description: "Notify your parent. done:true with an object output declares your result (recorded; the turn ends after this call) — your parent is woken with that output when your turn's observation confirms your plan is complete. Without done, the note is recorded; waiting_on declares what you are waiting for (parent | user | subagent) — it rides your rest as a hint.".into(),
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

/// A non-child session's tool set: the core tools + the sub-agent tools.
#[must_use]
pub fn agent_tool_specs() -> Vec<ToolSpec> {
    let mut v = tool_specs();
    v.extend(subagent_tool_specs());
    v
}

/// A child session's tool set: the core tools + `parent_notify` — no
/// sub-agent tools (a child cannot spawn, ADR-0001 depth cap).
#[must_use]
pub fn child_tool_specs() -> Vec<ToolSpec> {
    let mut v = tool_specs();
    v.push(parent_notify_spec());
    v
}

mod impls;
pub mod surface;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_surface;
