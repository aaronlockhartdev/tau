//! The tool surface (R1): the single owner of the role→tool mapping —
//! which specs each session role presents to the model, and where a call
//! routes. The turn loop dispatches against this table and matches no tool
//! names; the spawn-time agent-type filter and the child task-refusal rule
//! live here, not smeared across the loop, the spawn, and the agent.

use super::{ToolCall, ToolOutput};
use crate::agent::AgentSession;
use crate::harness::SessionRole;
use crate::provider::ToolSpec;
use crate::subagent::{ChildLink, Supervisor};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// The specs a role's session presents to the model: the builders in
/// tools.rs, composed by role. The agent-type allowlist (a `.md` type's
/// tool subset) applies to the child role only.
#[must_use]
pub fn specs_for(role: &SessionRole, agent_tools: Option<&[String]>) -> Vec<ToolSpec> {
    match role {
        SessionRole::Root { .. } => root_specs(),
        SessionRole::Child { .. } => child_specs(agent_tools),
        SessionRole::Bare { .. } => bare_specs(),
    }
}

/// A non-child session's tool set: the core tools + the sub-agent tools +
/// the task tools.
#[must_use]
pub fn root_specs() -> Vec<ToolSpec> {
    super::agent_tool_specs()
}

/// A bare session's tool set: the core tools.
#[must_use]
pub fn bare_specs() -> Vec<ToolSpec> {
    super::tool_specs()
}

/// The child's tool set with the spawn-time agent-type filter (spec §5.5):
/// a `.md` type's `tools` allowlist subsets the child's default set.
#[must_use]
pub fn child_specs(agent_tools: Option<&[String]>) -> Vec<ToolSpec> {
    let base = super::child_tool_specs();
    match agent_tools {
        Some(allowed) => base
            .into_iter()
            .filter(|s| allowed.iter().any(|a| a == &s.name))
            .collect(),
        None => base,
    }
}

/// A child is a leaf: create/assign/cancel act on the parent's planning
/// state, never on the assigned record (spec §5.3); the refusal is the
/// caller's only view. Shared by the model's dispatch and the app's
/// task-command surface — the same rule on both paths.
#[must_use]
pub fn task_child_refusal(is_child: bool, name: &str) -> Option<String> {
    if is_child && matches!(name, "task_create" | "task_assign" | "task_cancel") {
        Some(format!(
            "{name}: not available in a child session — work the task your parent assigned"
        ))
    } else {
        None
    }
}

/// The routing table (R1): one owner for "where does a call go" — the
/// supervisor's `task_assign`, the session's other task tools, the scoped
/// recall, the supervisor's sub-agent tools, the child's `parent_notify`,
/// and the core tools.
///
/// Invariant: route by name, not by which link exists — a parent session
/// carries a supervisor AND core tools (ticket #23's original
/// `if let Some(sup)` swallowed every core tool into `route_parent`, which
/// only knows sub-agent names), and a child carries a link AND core tools.
pub async fn dispatch(
    session: &AgentSession,
    cwd: &Path,
    image_max_bytes: Option<u64>,
    supervisor: Option<Arc<Supervisor>>,
    child: Option<Arc<ChildLink>>,
    call: &ToolCall,
) -> ToolOutput {
    match call.name.as_str() {
        // Cross-session: routed through the supervisor, which runs both
        // sides on the sessions' own stores (review B3).
        "task_assign" => match supervisor.as_ref() {
            Some(sup) => {
                let task = call.args.get("task").and_then(Value::as_str).unwrap_or("");
                let worker = call
                    .args
                    .get("worker")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match sup.assign_task(task, worker) {
                    Ok(()) => format!("assigned {task} to {worker}").into(),
                    Err(e) => e.into(),
                }
            }
            None => "task_assign: this session has no sub-agents".into(),
        },
        // Tasks live in this session's own store (spec §5.3) — parent and
        // child alike. Routed before the sub-agent surface so a child
        // (which has no supervisor) still gets its tools.
        name if name.starts_with("task_") => session.task_tool_call(name, &call.args).into(),
        "recall" => session.recall_scoped(&call.args).into(),
        name if name.starts_with("subagent_") => match supervisor.as_ref() {
            Some(sup) => crate::subagent::route_parent(sup, call).into(),
            None => format!("{name}: not available in this session").into(),
        },
        "parent_notify" => match child.as_deref() {
            Some(link) => link.notify(&call.args).into(),
            None => "parent_notify: not available in a top-level session".into(),
        },
        _ => super::dispatch(cwd, call, image_max_bytes).await,
    }
}
