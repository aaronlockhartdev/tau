# Adopt pi's external context loading

## Context

The harness loads `AGENTS.md`, `SYSTEM.md`, `APPEND_SYSTEM.md` from the agent directory and the project. Precedence, ordering, and fallback must be defined, not invented.

## Options

- Mirror pi: pi 1.5.2 `docs/configuration.md`, sections Agent directory, Project `.pi`, Context files (read 2026-11-13). Cost: one context leaf spec, no new dependency.
- Design tau's own loading: no external evidence; every frontend and every external agent learns a private rule, and the shared `AGENTS.md` convention stops working.

## Decision

Mirror pi's loading exactly. For mechanics, read pi 1.5.2 `docs/configuration.md` sections Agent directory, Project `.pi`, Context files, and Skills. Do not restate them here, and do not diverge.

Tau's own rules:

- Project directory is `.tau/`. `.pi/` is never read.
- Two files are supported: `SYSTEM.md` replaces the system prompt, `APPEND_SYSTEM.md` appends to it. A project file wins over the agent-directory file; only one of each loads.
- `AGENTS.md` auto-discovery is the only unrequested loading: root, plus the nearest one in a subtree on conflict. Other documents are read by path, when a skill or index points at them.

## Consequences

Context loading lives in core, so the ACP and desktop frontends load identically. Loading becomes observable as a prompt prefix, cached per session. Any pi change needs a delta here.

Open: does `.tau/` mirror pi's `AGENTS.override.md` and `CLAUDE.md` fallbacks, or load `AGENTS.md` alone?
