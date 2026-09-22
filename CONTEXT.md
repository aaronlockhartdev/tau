# Tau

A lightweight coding-agent harness for macOS and Linux, built on Rust, Svelte, and Tauri. Heavily inspired by [pi](https://github.com/earendil-works/pi) — minimal core, GUI instead of terminal.

## Language

**Tau**:
The product: the desktop application that hosts coding agents.
_Avoid_: "the app" (ambiguous with the Tauri shell), "the project" (ambiguous with the repo)

**Harness**:
A runtime that hosts a coding agent: the process, its tools, and its interface to the user. Tau is a harness.
_Avoid_: framework, runtime (too generic)

**Core**:
The standalone Rust library crate (`tau-core`) that owns the agent loop, sessions, and tools. The Tauri app is a client of the core.
_Avoid_: engine, backend

**Session**:
The branching record of a conversation between a user and one agent instance: a tree of entries (`id`/`parentId`) that can branch in place. Stored as one JSONL file per session — append-only lines with a per-line CRC, oversized payloads as out-of-band sidecar blobs, manually zstd-archived on user request (ADR-0005). Sub-agents have their own session files, linked to the parent. **Placement**: workspace-scoped session data lives in the workspace's project directory, `{project root}/.tau/sessions/` (sidecar blobs and manual archives alongside); sessions with no project live in `~/.config/tau/sessions/`.
_Avoid_: conversation, transcript (a transcript is a linear rendering of a session)

**Turn**:
One assistant response and the tool calls it executes, from model request to completion.
_Avoid_: step, iteration

**Steering**:
A message the user queues while a turn is active; delivered after the current turn's tool-call batch completes, becoming the next turn.
_Avoid_: interrupt, nudge

**Follow-up**:
A message the user queues while the agent is working; delivered only after all of the agent's work has finished.
_Avoid_: pending message, backlog

**Force-send**:
An explicit user option to submit a message while a turn is active: kills the in-flight LLM stream (partial output kept as an `interrupted` message), lets the in-flight tool batch complete (configurable: complete | kill), and delivers the message at the head of the queue — ahead of queued steering/follow-ups.
_Avoid_: interrupt-send, break

**Compaction**:
Tau's native reduction of session history to fit the context window: an Observational Memory log plus a recent raw window — no lossy one-shot summarization (ADR-0004).
_Avoid_: summarization, truncation

**Observational Memory (OM)**:
The memory model introduced by Mastra that tau's compaction is built on (ADR-0004): an Observer LLM converts raw messages into a dense, append-only, date-grouped observation log; a Reflector LLM rewrites the log to keep it bounded; raw messages stay in storage, recoverable via `recall`.
_Avoid_: "the OM feature", "memory feature"

**Sub-agent**:
A concurrent agent-loop instance in `tau-core` working a delegated task in its own session file linked to the parent. Always asynchronous (the spawn returns the child's name; the session id stays the machine key); a child is an *ordinary* session — openable, steerable, branchable in the GUI. States: running · idle (explicit wait) · done · failed · stopped — all deliberately resumable; nothing auto-resumes (ADR-0001, ADR-0006).
_Avoid_: child agent, worker (implies a generic process)

**Task**:
A first-class unit of work: enforced state machine (pending → in-progress → done / blocked / cancelled), ordered steps with expected outputs, acceptance criteria, evidence. Stored as append-only events **in the owning session's file** (on assignment, the worker's session becomes the live record); tasks do not outlive their session (user override 2026-09-17). Completion is **evidence-gated**; completable by the parent or a sub-agent, whose finish resolves it as completed / handed_off / blocked (ADR-0001, ADR-0006).
_Avoid_: job, ticket (a ticket is a wayfinding/issue concept)

**Handoff**:
The transfer of a task between agents: in = a self-contained brief (goal, context pointers, constraints, output schema); out = the sub-agent's schema-validated `parent_notify {done: true}` (ADR-0001, ADR-0006).
_Avoid_: delegation (the act of assigning, not the transfer)

**Provider**:
In v0, a named OpenAI-compatible endpoint (base URL + key + model ids) speaking the `responses` API only; no catalog, no keychain, no per-provider fidelity detection — that is the user's responsibility (ADR-0003).
_Avoid_: model (a single entry within a provider), LLM backend

**Handle**:
An opaque identifier returned by a sub-agent spawn; the target of `message` (which also resumes a non-running child) / `stop` / `state` (ADR-0006).
_Avoid_: id (ambiguous with entry/session ids), reference

**Sidecar blob**:
An out-of-band raw-byte file (zstd-compressed) referenced from a session entry; holds oversized payloads (images, large tool output) so the session log stays readable (ADR-0005).
_Avoid_: attachment (that is an in-message concept), spool

**Recall**:
The tool by which a model pages from an observation group back to the raw session entries (browsing-only in v0; no vector search) (ADR-0004).
_Avoid_: search (v0 recall browses; search is the deferred vector mode), lookup

**Agent type**:
A named sub-agent configuration — system prompt, tools, model, default context mode — defined by `.md` files (system `~/.config/tau/agents/`, project `{project}/.tau/agents/`, project wins) or built in (`general`).
_Avoid_: agent class, profile

**Workspace**:
`(id, display name, cwd)` — the directory on the host where `tau-core` runs; the identity unit for sessions and tasks. Paths in the protocol resolve against it; local paths are never used as identity (ADR-0006).
_Avoid_: project (ambiguous with the git repo), cwd (just the path)

**Resume contract**:
A compact machine-generated summary of a task's current state — current step + expected output, evidence, gaps, blockers, next action; re-injected on compaction and the payload for resuming a paused/done sub-agent (ADR-0006).
_Avoid_: checkpoint (that implies persistence granularity), summary (too generic)

**Snapshot**:
The ephemeral point-in-time render state the core builds for the GUI — metadata skeleton + bounded OM + live state + cursor; never a file (ADR-0006).
_Avoid_: export, archive (that is the manual-archive zstd thing)
**Context files**:
Project/global instruction files loaded into the system prompt, following pi's pattern (2026-09-17): `AGENTS.md`/`CLAUDE.md` from `~/.config/tau/`, from parent directories walking up from the workspace cwd, and the cwd itself — all layers appended; a per-directory `AGENTS.override.md` replaces that directory's `AGENTS.md`/`CLAUDE.md`.
_Avoid_: prompt files

**Security model**:
In v0: **transparency, not enforcement** (ADR-0007, user decision 2026-09-17) — no sandbox, no permission popups, no per-tool policy; tools run with the user's privileges; sub-agent output marked untrusted is the one enforced boundary; the GUI's legibility affordances (per-tool cards, kill, force, sub-agent visibility) are the security feature; a post-v0 security ticket decides what enforcement comes back.
_Avoid_: trust flow, permission system (v0 has neither)
