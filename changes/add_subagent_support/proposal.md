# First-class subagents

## Context

A side task such as a codebase search floods the parent conversation with output the parent never cites again. A subagent does that work in its own context window and returns a summary, which is the main context-saving device a harness has. tau has to show that work live, and let the user open a worker, steer it, answer its permission requests, and stop it, from both frontends. A subagent is a session, so this touches session, protocol, acp, tools, surface and config at once, and the storage rules from `add_session_storage` apply unchanged.

## Options

- A. ACP subagent sessions. Each child is a real session id, announced by a `subagent_update` upsert on the parent's stream, with events delivered automatically on the same connection, directed messages between sessions, restricted Client-initiated mutations per child, and per-child `cancel`. Evidence: <https://agentclientprotocol.com/rfds/subagents> (draft, read 2026-11-13); the Rust SDK [agent-client-protocol](https://crates.io/crates/agent-client-protocol) 5.2.0, published 2026-11-10 with 6.2M downloads (2026-11-13). Cost: the RFD is a draft that is still moving (its revision history has an entry dated 2026-11-13), and its conformance set forbids client-initiated prompting, so steering needs the extension the RFD names as its follow-up.
- B. Subagents as tool calls. The delegation is a tool call whose result is text, as Claude Code's [`Agent` tool row](https://code.claude.com/docs/en/sub-agents) and OpenAI's [`agent.asTool()`](https://developers.openai.com/api/docs/guides/agents/orchestration) present it (both read 2026-11-13). Cost: 0 new wire types, and a worker with no stream, no per-child cancellation, and a nested tree that flattens into one transcript.
- C. Peer background sessions under a roster. Each worker is a top-level session run by a supervisor process and listed in one view, as Claude Code's [agent view](https://code.claude.com/docs/en/agent-view) and Codex's [`/agent` thread switcher](https://developers.openai.com/codex/config-reference) do (docs, read 2026-11-13). Cost: no parentage on the wire, roster state lives in the client, and a supervisor process to run, restart, and keep sessions alive through sleep.
- D. An external orchestrator: terminal panes per agent, or child processes. Evidence: measured here, every delegated run in the writing of this repo was a separate `pi -p` process fed by stdin, with no session state and no wire, and its result pasted back by a human (repro: `pi -p --no-session --no-tools` with a piped context, 2026-11-13). Cost: 0 protocol, interaction that only the terminal provides, and children invisible to ACP clients and to storage.

## Decision

A, with the RFD's named follow-up built in from the start: a per-child `prompt` capability, so tau's own app can steer a child. tau defines both sides of its desktop connection, so it can adopt the extension that [Claude Code's fork panel](https://code.claude.com/docs/en/sub-agents#observe-and-steer-running-forks) and [OpenCode's child sessions](https://opencode.ai/docs/agents/) already provide; a client that only implements the draft gets `cancel` and observation.

The reference implementations carry the mechanics: the RFD for wire semantics (`subagent_update`, directed `session_message`, states `running` / `requires_action` / `unknown` / `idle` with `stopReason`, cancellation cascade, replay); Claude Code's sub-agent and agent-view docs for the definition format, limits, roster states, and the observe-and-steer UX; OpenCode's [agent files](https://opencode.ai/docs/agents/) for frontmatter shape.

tau's own rules:

- Definition files at `.agents/agents/<name>/agent.md`, frontmatter `name`, `description`, `model`, `tools`, `permissions`, `mode`, body as the system prompt, global at `~/.agents/agents/` and workspace at `.agents/agents/`, workspace winning. This follows the [`.agents` Protocol](https://dotagentsprotocol.com/) layout (read 2026-11-13), the same convention as the skills directory `adopt_skills_convention` adopted.
- Two tools: `task(subagent, task, model?, tools?, isolation?, background?)` spawns, and `send(to, message)` messages a named session, with a sibling roster of valid `to` values in context.
- A child is a session row with a `parent_id` in `.tau/state.db`, and a directed message is an event on each participant's transcript. Children are sessions already, so the store gains no table, no mirror, and nothing to reconcile.
- A child inherits the parent's permission rules and cannot obtain rights the parent lacks. A child's permission request is answered in the parent's UI, named by the child, and stays attributed to the child session.
- Steering a child that is mid-turn queues to the next turn boundary. Cancelling a child cancels its work and the descendants that work is waiting on; closing a parent cancels every descendant.
- Limits are config keys with defaults: `subagent.max_concurrent` 20, `subagent.max_depth` 3, and the `task` tool absent at the depth limit. The parent's reported cost includes its children, and the UI never sums parent and child costs into a tree total.
- A child's context is assembled by `adopt_context_file_loading` and `adopt_skills_convention`. A fork inherits the parent's history and shares its prompt cache, and cannot fork further.
- `isolation: "worktree"` runs a child in a worktree at `../tau-<id>` under the Working tree rules, sharing `CARGO_TARGET_DIR`.
- Child streams follow the live-output invariant in `spec/index.md`: they render from core's in-memory stream, and storage never gates them.
- The behaviour lane drives parent and children from one tape; the surface lane asserts the roster, the needs-input count, steering, and cancellation headlessly.

## Consequences

Deltas land in session (child sessions, directed messages, parent_id), protocol and acp (the RFD shapes and the `prompt` extension), tools (`task`, `send`, limits, roster), surface (roster view, attach, steer, cancel), and config (the limit keys). `contracts/` regenerates from the new wire types. Storage gains `parent_id` and directed-message event kinds, with no schema duplication. tau's UI state names map onto the RFD's, so a roster needs no tau-only state machine. The RFD is a draft and the Rust SDK's subagent coverage is `unverified`, so the wire types ship in `crates/tau-protocol` with tau's own versioned mapping, and the M0 spike verifies the SDK. Definition descriptions ride in the tool list, so each one costs tokens per turn; the config keys bound how many load. One dependency: `agent-client-protocol` 5.2.0 in `crates/tau-acp`, kept if the spike finds its types usable. Clauses are in `changes/add_subagent_support/specs/`: `tools/delegation` as `R-tools-01..14`, `session/child` as `R-session-14..17`, `surface/roster` as `R-surface-19..22`, `acp/subagent` as `R-acp-01..06`.

Eval: on demand. A fan-out task, such as reviewing every changed file, is the case that shows the context saving and the wall-clock cost.

Open: none.
