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
The branching record of a conversation between a user and one agent instance: a tree of entries (`id`/`parentId`) that can branch in place. Stored as one JSONL file per session — append-only lines with a per-line CRC, oversized payloads as out-of-band sidecar blobs, dormant sessions zstd-archived (ADR-0005). Sub-agents have their own session files, linked to the parent.
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
A concurrent agent-loop instance in `tau-core` working a delegated task in its own session file linked to the parent; spawned asynchronously (opaque handle), with completions and messaging tool-based (ADR-0001).
_Avoid_: child agent, worker (implies a generic process)

**Task**:
A first-class unit of work — state machine (pending → in-progress → done), steps, acceptance criteria, evidence; completable by the parent agent or by a sub-agent. Assigning a task to a sub-agent spawns it with the task as brief (ADR-0001).
_Avoid_: job, ticket (a ticket is a wayfinding/issue concept)

**Handoff**:
The transfer of a task between agents: in = a self-contained brief (goal, context pointers, constraints, output schema); out = the sub-agent's schema-validated `finish` tool call (ADR-0001).
_Avoid_: delegation (the act of assigning, not the transfer)

**Provider**:
In v0, a named OpenAI-compatible endpoint (base URL + key + model ids) speaking the `responses` API only; no catalog, no keychain, no per-provider fidelity detection — that is the user's responsibility (ADR-0003).
_Avoid_: model (a single entry within a provider), LLM backend

**Handle**:
An opaque identifier returned by a sub-agent spawn; the target of the `message`/`stop`/`resume` operations (ADR-0001).
_Avoid_: id (ambiguous with entry/session ids), reference

**Sidecar blob**:
An out-of-band raw-byte file (zstd-compressed) referenced from a session entry; holds oversized payloads (images, large tool output) so the session log stays readable (ADR-0005).
_Avoid_: attachment (that is an in-message concept), spool

**Recall**:
The tool by which a model pages from an observation group back to the raw session entries (browsing-only in v0; no vector search) (ADR-0004).
_Avoid_: search (v0 recall browses; search is the deferred vector mode), lookup
**Context files**:
Project/global instruction files (e.g. AGENTS.md) that are loaded into the system prompt. The exact set and discovery walk are open.
_Avoid_: prompt files
