# Sub-agents and task hand-off are native to the core

Pi deliberately ships without sub-agents (its philosophy: "No sub-agents" — build them with extensions or tmux). Tau diverges: sub-agents and task hand-off are first-class features of the core, not an extension. Rationale: delegation is central to the workflows tau targets, and the GUI can present sub-agent sessions cleanly — a gap pi and its sub-agent extensions have. The concrete sub-agent/task model (what a task is, hand-off protocol, session topology) is open on the map.

**Considered**: pi-style minimalism (sub-agents as an extension or out of scope). Rejected: the GUI's ability to show nested sessions is tau's differentiator, and that requires the core to own sub-agent sessions.

**Supplement (2026-09-16, model settled on the map)**:
- **Execution**: in-process — a sub-agent is a concurrent agent-loop instance in `tau-core` (the child-process alternative dies with ADR-0002).
- **Session**: each sub-agent has its own session file linked to the parent (parent session id + originating tool-call id).
- **Context modes**: `fresh` (default — self-contained brief + its own system prompt + context files), `compacted` (the parent's OM observations + recent raw + a `recall` tool scoped to the parent session), `fork` (branched copy of parent history) — chosen explicitly per delegation.
- **Asynchronous**: every spawn returns a handle immediately; completions are pushed to the parent as events (the result is injected at the parent's next turn boundary).
- **Hand-off**: brief in (goal, context pointers, constraints, output schema); out = a schema-validated `finish` tool call; the child's output is marked untrusted before the parent's model reads it.
- **Messaging**: two-way `notify`/`message` tools — a running child receives parent messages on its own steering lane; a paused child is resumed by a message.
- **Task**: a separate first-class primitive (state machine pending → in-progress → done; steps; acceptance criteria; evidence); completable by the **parent or a sub-agent**; assigning a task to a sub-agent spawns it with the task as brief.
- **Caps**: config-driven concurrency (default: unlimited) and depth (default: 1 — a sub-agent cannot spawn sub-agents; at the limit the spawn tool is absent).
- **Lifecycle** (refined on #10, 2026-09-16): stop = soft interrupt → paused (kept partial marked interrupted, task resume contract rides along); resume = revive from the child's session file with a deliberate message; all non-running states (idle, done, stopped) are deliberately resumable by parent or GUI — **nothing auto-resumes anything**; the earlier “user-stopped child refuses auto-resume” rule is abolished.
