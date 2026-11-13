# Delta: tools/delegation

## ADDED

### R-tools-01  Subagent definitions are discovered

WHERE definitions live at `~/.agents/agents/<name>/agent.md` and `.agents/agents/<name>/agent.md` THE SYSTEM SHALL load both, with the workspace definition winning.

Acceptance: @R-tools-01

### R-tools-02  A definition becomes a capability list and a prompt

WHEN a definition loads THE SYSTEM SHALL read `name`, `description`, `model`, `tools`, `permissions`, and `mode` from its frontmatter and take its body as the child's system prompt.

Acceptance: @R-tools-02

### R-tools-03  task spawns a child session

WHEN the model calls `task` with a definition and a task THE SYSTEM SHALL start a child session running that definition.

Acceptance: @R-tools-03

### R-tools-04  send reaches a named session

WHEN the model calls `send` with a target THE SYSTEM SHALL deliver the message to that session.

Acceptance: @R-tools-04

### R-tools-05  Send targets are listed

WHEN a session's context is assembled THE SYSTEM SHALL list the session ids valid as `send` targets.

Acceptance: @R-tools-05

### R-tools-06  Concurrency is capped at the config key

WHEN the running children of one parent reach `subagent.max_concurrent` THE SYSTEM SHALL refuse a further child and report the cap.

Acceptance: @R-tools-06

### R-tools-07  Depth is capped at the config key

WHEN a session's depth reaches `subagent.max_depth` THE SYSTEM SHALL omit the `task` tool from its context.

Acceptance: @R-tools-07

### R-tools-08  A child inherits its parent's permissions

WHILE a child runs THE SYSTEM SHALL apply the parent's permission rules to the child.

Acceptance: @R-tools-08

### R-tools-09  A child gains no right its parent lacks

WHEN a child requests a permission THE SYSTEM SHALL refuse any right absent from the parent's rules.

Acceptance: @R-tools-09

### R-tools-10  A fork copies the parent's history

WHEN a child is a fork THE SYSTEM SHALL copy the parent's history into the child's context.

Acceptance: @R-tools-10

### R-tools-11  A fork shares the parent's prompt cache

WHEN a fork runs THE SYSTEM SHALL read the parent's prompt cache for its first turn.

Acceptance: @R-tools-11

### R-tools-12  A fork cannot spawn

WHILE a session is a fork THE SYSTEM SHALL omit the `task` tool from its context.

Acceptance: @R-tools-12

### R-tools-13  A parent's cost includes its children

WHEN a parent session's cost is reported THE SYSTEM SHALL include the cost of its children.

Acceptance: @R-tools-13

### R-tools-14  Isolation runs a child in a worktree

WHEN a `task` call requests `isolation: "worktree"` THE SYSTEM SHALL run the child in a worktree under the Working tree rules in `AGENTS.md`.

Acceptance: @R-tools-14
