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
The branching record of a conversation between a user and one agent instance: a tree of entries (turns, tool calls, compactions) that can branch in place. Sub-agents have their own sessions, linked to their parent session.
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
The option to deliver a message immediately while a turn is active, preempting what the agent is doing. Semantics are open — see the map ticket "What does force-send do to an in-flight turn?".
_Avoid_: interrupt-send, break

**Compaction**:
Tau's native reduction of session history to fit the context window, built on Observational Memory rather than plain summarization.
_Avoid_: summarization, truncation

**Observational Memory (OM)**:
The memory model introduced by Mastra that tau's compaction is built on: records of observations about the work, as opposed to a lossy summary of the dialogue.
_Avoid_: "the OM feature", "memory feature"

**Sub-agent**:
An agent instance spawned to work a delegated task, with its own session. Native to tau's core, not an extension.
_Avoid_: child agent, worker (implies a generic process)

**Task**:
A unit of work delegated to a sub-agent — the contract between a parent agent and a sub-agent. Semantics open — see the map ticket "What is tau's sub-agent & task model?".
_Avoid_: job, ticket (a ticket is a wayfinding/issue concept)

**Handoff**:
The transfer of ownership of a task between agents, including what context travels with it.
_Avoid_: delegation (the act of assigning, not the transfer)

**Provider**:
In v0, any OpenAI-compatible API endpoint (base URL + key + model id) the core can talk to. Tau has no provider catalog.
_Avoid_: model (a single entry within a provider), LLM backend

**Context files**:
Project/global instruction files (e.g. AGENTS.md) that are loaded into the system prompt. The exact set and discovery walk are open.
_Avoid_: prompt files
