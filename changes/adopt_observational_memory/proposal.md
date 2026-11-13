# Adopt observational memory

## Context

A tau session must run for days without compaction. Rolling summaries destroy earlier decisions, re-grow context rot, and break prompt caching.

## Options

- Mirror Mastra OM: `@mastra/memory` ≥1.1.0 docs, Observational Memory section, and `packages/memory` source (read 2026-11-13); the docs state 5–40x compression and cache-first ordering. Cost: one session leaf spec, observation and reflection tables.
- Rolling summary: no external evidence in its favour; it discards earlier decisions, regrows context rot, and invalidates the prompt cache.

## Decision

Mirror Mastra's observational memory as used by Mastra Code. For mechanics, read `@mastra/memory` (≥1.1.0) docs, Observational Memory section, and the `packages/memory` source. That covers the three layers, the Observer and Reflector roles, the token thresholds and their defaults, the compression ratios, and the cache-first ordering.

Tau's own rules:

- Observer and reflector run on the session's own model, configured like any model call. Cost is bounded by the context budget in user config, not by a separate ceiling.
- The observer sees the same context files the agent sees, so it honours `AGENTS.md`.
- Compression is lossy, so a decision is durable only once it reaches a spec. A decision worth keeping is written into `spec/` through a change, never left in prompt state.

## Consequences

Memory is a core component; the desktop view shows its state, and ACP exposes it as session metadata. Storage adds observation and reflection tables to session storage. Token budgets move from prompt text into storage. Cost and latency are measured by eval.

Open: none.
