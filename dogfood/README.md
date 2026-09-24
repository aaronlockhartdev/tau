# Dogfood: tau building a small app, driven by tauri-pilot

A real dogfood run (2026-09-24): an agent drove the debug build of tau with
the tauri-pilot CLI the way a user would — opening a workspace, sending one
message, and letting the agent build a small app with tasks and sub-agents.

**The message** (one, sent to a fresh session `ruthless-rest`):

> Build me a small CLI app in this folder: todo.py, a Python todo CLI with
> add, list, done and delete, persisting to todos.json. First make a task
> list for the work and work through the tasks in order. Then spawn a
> sub-agent to write a test suite (test_todo.py) covering all four commands
> plus persistence, and run it.

**What it exercised, as observed in the UI:**

- the task system: `task-1` "Build todo.py CLI" created → started →
  evidenced (both criteria) → done, and surfaced in the tasks pane
  (`history · 1`, `done` badge)
- sub-agents: `nutritious-gold` spawned (fresh context), ran its own session,
  reported back via `parent_notify`, and landed in the sub-agents pane
  (`live · 1 running` → `done`)
- tools + files pane: bash/write calls rendered live; `todo.py`,
  `todos.json`, `test_todo.py` appeared as they were written
- the result: 14 pytest tests, all passing, verified by the parent itself

**Files:**

- `sessions/parent-ruthless-rest.jsonl` — the parent session (27 entries)
- `sessions/child-nutritious-gold.jsonl` — the sub-agent's session (35 entries)
- `todo.py`, `test_todo.py` — what the agent built
- `screenshot.png` — final UI state

Session paths were rewritten `~/tau-dogfood` (the workspace was deleted after
the run; these recordings are a realistic-session sample for the E2E
fixture work).

**Bugs the dogfood surfaced** (tracked for the v0 clean-up):

1. failing tool calls render a ✓ where they should render an ✗
2. opening a session should scroll to the bottom (not the top)
3. child sessions must not be archivable; in the archived dropdown they show
   twice (top-level and under the parent) — they should appear only under
   the parent
4. a sub-agent's child session should be auto-expanded in the tree
