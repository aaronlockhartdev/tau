# design_user_config

## Context

One unified user config, applied live without restarting the app. Settings scattered across files,
each needing a restart, is the failure this change removes.

## Options

### A. TOML, format-preserving, hot-reloaded
`toml_edit` 0.25.19 is "yet another format-preserving TOML parser", so tau can write a key without
rewriting the user's comments and ordering; `notify` 8.2.2 gives cross-platform file watching.
Human-editable, comments allowed, typed via serde. Evidence: crates.io 2026-11-13.

### B. JSONL
Append-only, no comments, no nesting ergonomics; suits event and session logs, not settings.
Rejected as config; kept for logs.

### C. Layered loader crate (`config` 0.15.27)
Adds user → project → env layering on top of A. Take it only if layering is required at v1.

## Decision

A: TOML in `toml_edit`, watched by `notify`, validated by a typed schema; a bad file keeps the last
good values and surfaces the error in the GUI rather than falling back silently. JSONL stays the
append-only format for logs and tapes.

Open: single file vs `config.toml` + secrets split; which keys apply live vs need restart — the
list is a spec, not a guess. `[NEEDS CLARIFICATION]`
