//! The tool surface (R1): the single owner of the role→tool mapping —
//! which specs each session role presents to the model, and where a call
//! routes. The turn loop dispatches against this table and matches no tool
//! names; the spawn-time agent-type filter lives here, not smeared across
//! the loop, the spawn, and the agent.

use super::impls::{bash_inner, edit, read, write};
use super::{ToolCall, ToolResult};
use crate::agent::AgentSession;
use crate::harness::SessionRole;
use crate::provider::ToolSpec;
use crate::subagent::{ChildLink, Supervisor};
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

/// A non-child session's tool set: the core tools + the sub-agent tools.
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

/// The dispatch context: everything a tool call may reach, named. The core
/// tools use only `cwd`; the session-scoped tools use `session`,
/// `supervisor`, and `child`. A future capability that is not a property of
/// the session (an MCP registry, a network client) adds a field here rather
/// than a positional parameter.
pub struct DispatchCtx<'a> {
    pub session: &'a AgentSession,
    pub cwd: &'a Path,
    pub image_max_bytes: Option<u64>,
    pub supervisor: Option<Arc<Supervisor>>,
    pub child: Option<Arc<ChildLink>>,
}

/// The routing table (R1): one owner for "where does a call go" — the
/// scoped recall, the supervisor's sub-agent tools, the child's
/// `parent_notify`, and the core tools.
///
/// Invariant: route by name, not by which link exists — a parent session
/// carries a supervisor AND core tools (ticket #23's original
/// `if let Some(sup)` swallowed every core tool into `route_parent`, which
/// only knows sub-agent names), and a child carries a link AND core tools.
pub async fn dispatch(ctx: DispatchCtx<'_>, call: &ToolCall) -> ToolResult {
    match call.name.as_str() {
        "recall" => ToolResult::from_result(ctx.session.recall_scoped(&call.args)),
        // Cross-session: routed through the supervisor, which runs both
        // sides on the sessions' own stores (review B3).
        name if name.starts_with("subagent_") => match ctx.supervisor.as_ref() {
            Some(sup) => ToolResult::from_result(crate::subagent::route_parent(sup, call)),
            None => ToolResult::err(format!("{name}: not available in this session").into()),
        },
        "parent_notify" => match ctx.child.as_deref() {
            Some(link) => ToolResult::ok(link.notify(&call.args).into()),
            None => ToolResult::err("parent_notify: not available in a top-level session".into()),
        },
        // Core tools are stateless: they reach only the cwd.
        "read" => read(ctx.cwd, &call.args, ctx.image_max_bytes).await,
        "write" => write(ctx.cwd, &call.args).await,
        "edit" => edit(ctx.cwd, &call.args).await,
        "bash" => bash_inner(ctx.cwd, &call.args, &ctx.session.stop_flag()).await,
        other => ToolResult::err(format!("unknown tool: {other}").into()),
    }
}
