# Compaction is Observational Memory (no lossy summarization)

**Context**: pi's compaction is lossy — at near-overflow an LLM summarizes the old span into prose and the detail leaves the context. Tau's compaction is natively Observational Memory (OM, Mastra; research #2): an **Observer** LLM converts raw messages (including tool calls/results) into a dense, append-only, date-grouped, priority-marked observation log; a **Reflector** LLM periodically rewrites the log itself to keep it bounded; the live context is always *observations block + recent raw window*; raw messages remain in storage and are recoverable.

**Decision**: in v0, OM **is** the compaction — there is no pi-style one-shot summarization path:

- Per-session `OmRecord` (observations text, path-scoped cursor, generation) stored with the session (ADR-0005 carries it as appended entries).
- Observer fires at turn end when unobserved tokens ≥ threshold (default 30k); Reflector fires when observations tokens ≥ threshold (default 40k); **both thresholds are configurable**; the ported **dynamic threshold** (raw oscillates between a retention floor and the threshold) is the overflow guard.
- v0 includes **async buffering**: fire-and-forget Observer runs over the safe completed prefix (~6k increments), activated at threshold/idle with no LLM call.
- Observer/Reflector run on a **single global `om_model`** config (user decision 2026-09-17 — not per-provider; default: the session's model) — a cheap model can serve both roles.
- **Per-branch records**: forking copies the parent's `OmRecord` at the fork point; each branch then observes independently, cursor always evaluated on the active path. (Mastra has no branch concept — this is tau's addition.)
- The **`recall` tool** is in v0: page from an observation group's entry range back to raw session entries — browsing-only, no vector search (a designed-in extension: groups already carry stable ids + ranges; a later `search` mode = embeddings config + index sidecar).
- The Observer/Reflector prompts and threshold math are ported verbatim from Mastra (Apache-2.0 per the repo's `package.json`; the ported files sit outside the separately-licensed `ee/` directory; attribution at the port site).

**Consequences**: the glossary's "Compaction" is pinned by this ADR; the session format carries OM as appended span-referencing entries (ADR-0005); the GUI renders the observations block and offers `recall`; the recurring Observer/Reflector LLM cost is real (mitigated by a cheap `om_model`); OM's published evidence is for long-conversation *recall*, not coding-task fidelity — tau should measure rework rate / token spend / cache-hit rate before and after in v0.

**Considered**: pi-style summarization as the primary mechanism (rejected: it is lossy — the exact gap OM closes); on-demand OM only, at overflow (rejected: the continuous cadence is what keeps the context stable and prompt-cache-friendly); vector search in v0 (deferred: a third model dependency plus an index for the lower-value case in v0).
