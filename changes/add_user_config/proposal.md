# Design the user config

## Context

One unified user config, applied live without restarting the app. Settings scattered across files, each needing a restart, is the failure this change removes.

## Options

- TOML with `toml_edit` (0.25.x): preserves comments, formatting, and key order, so a human-owned file survives a round-trip ([crates.io](https://crates.io/crates/toml_edit) and the [`toml_edit` docs](https://docs.rs/toml_edit), read 2026-11-13). Cost: 1 dependency.
- JSON with serde: no comments, no format preservation ([serde docs](https://serde.rs), read 2026-11-13). Cost: 0 dependencies, and a file the user cannot annotate.
- JSONL: an append-only log format, not a document format ([JSONL convention](https://jsonlines.org/), read 2026-11-13). Cost: a parser per consumer, for one document.
- `config` (0.15.x): layered merge with no write-back, so a user's comments are rewritten on save ([crates.io](https://crates.io/crates/config), read 2026-11-13). Cost: 1 dependency, and the human-owned file is lost.

Cost of the chosen option: two dependencies (`toml_edit`, `notify`), a config leaf spec, and a reload path in each frontend.

## Decision

TOML, one file, at `<agent-dir>/tau/config.toml`. `toml_edit` for read, write, and merge, and `notify` (8.2.x) for reload. `config` is not used; layering is a merge over `toml_edit` documents. JSONL is confined to logs and telemetry. Layout and key names are designed in the delta.

Reload rule: any key that can be applied live is applied live. A key that needs a restart is named as such in the file, and the app says so at the moment it changes.

## Consequences

Config is read in core, and the frontends subscribe to it, so ACP exposes the same state as the desktop app. The file lives outside the repo, so a secret is never committed, and it never enters a session log. Hot reload is testable through `tau-test` by writing the file mid-scenario. The reload rule and the key list land as `R-config-NN` in `spec/config/reload/spec.md`.

Eval: none needed. Reload and key application are deterministic, and the behaviour lane covers them.

Open: none.
