# adopt_observational_memory

## Context

A tau session must run for days without compaction. Rolling summaries destroy earlier decisions,
re-grow context rot, and break prompt caching.

## Decision

Mirror Mastra's observational memory as used by Mastra Code: the Observer / Reflector pair and the
three-layer context (recent messages · observations · reflections). The reference defines the
mechanics — thresholds, buffer ratios, activation, observation format, reflection rewrite, storage
requirements: `@mastra/memory` ≥1.1.0,
[docs](https://mastra.ai/docs/memory/observational-memory) and `packages/memory` source. tau
implements the same model, not a paraphrase of it.

Consequences owned by tau: two background agent roles with their own model config and per-session
cost budget; observation activation can block a turn, so the surface needs a visible observing
state; thresholds are user config keys; context-overflow recovery is tau's to design.

Open: observer/reflector models, per-session cost ceiling. `[NEEDS CLARIFICATION]`
