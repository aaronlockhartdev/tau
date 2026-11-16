# Adopt observational memory

## Context

A tau session must run for days without compaction. Rolling summaries destroy earlier decisions, re-grow context rot, and break prompt caching.

## Options

- Mirror Mastra OM: <https://mastra.ai/reference/memory/observational-memory> and the `packages/memory` source (read 2026-11-13); the docs state 5–40x compression and cache-first ordering. Cost: one session leaf spec; an observation and a reflection are blocks in the log, so no table lands.
- Rolling summary: no external evidence in its favour; it discards earlier decisions, regrows context rot, and invalidates the prompt cache.

## Decision

Mirror Mastra's observational memory as used by Mastra Code. For mechanics, read `@mastra/memory` (≥1.1.0) docs, Observational Memory section, and the `packages/memory` source. That covers the three layers, the Observer and Reflector roles, the token thresholds and their defaults, the compression ratios, and the cache-first ordering.

Tau's own rules:

- Observer and reflector run on the session's own model, configured like any model call. Cost is bounded by the context budget in user config, not by a separate ceiling.
- The observer sees the same context files the agent sees, so it honours `AGENTS.md`.
- Compression is lossy, so a decision is durable only once it reaches a spec. A decision worth keeping is written into `spec/` through a change, never left in prompt state.

## Consequences

An observation is a block in the `observations` slot and a reflection a block in the `reflections` slot: the log stays append-only (R-session-31), so a condensation replaces nothing — an observation names the block range it supersedes, and the next context is built from the observations and the kept tail. `adopt_block_log` lands that unit, and archives before this change. The count reaches a frontend on core's event stream, the shape `add_realtime_gui_output` owns (R-surface-08). Token budgets move from prompt text into storage. The clauses are in `changes/adopt_observational_memory/specs/session/memory/spec.md` as `R-session-19..27`, archived to `spec/session/memory/spec.md`.

Eval: on demand. Whether observations beat the raw message tail is a real-LLM quality question, so the suite runs by hand before a threshold changes.

Open: the surface and ACP shape of a session's memory state are owned by no change — `R-session-26` carries a
count to a frontend, not the state itself — so they open as their own. The threshold defaults sit with the
config change, and `R-config-13` lists the memory context budget
among the keys. No change owns the rule that turns that budget into the threshold to write at —
`add_turn_loop` defers the cap policy to a later change — so it opens as its own; this change owns only the
threshold that triggers an observation.
