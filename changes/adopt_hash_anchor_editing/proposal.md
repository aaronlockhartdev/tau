# Adopt hash-anchor editing

## Context

Line-number and fuzzy-matched edits silently hit the wrong line. Anchor-addressed editing is the precondition for letting an agent edit files at all. tau's own doc workflow runs on anchored edits, so the tool set is used by the corpus it documents, and its refusals and post-edit diffs are surface behaviour the app has to render. pi-hashline-edit-pro 7.1.1 is a shipped extension in daily use on this repo's documents, so the open question is how much of it tau mirrors.

## Options

- Mirror pi-hashline-edit-pro 7.1.1: its README and `docs/` (read 2026-11-13), a shipped extension in daily use on this repo's own documents. Cost: one tools leaf spec, an anchor store in session state.
- Line numbers with fuzzy matching: `unverified`, no external source; a stale line number edits the wrong line without an error.

## Decision

Mirror `pi-hashline-edit-pro` 7.1.1 exactly. For mechanics, read its README and `docs/`. That covers the anchor format and alphabet, anchor lifetime, stale and ambiguous refusals, batching, the post-edit diff and its anchor rows, undo, and the caps.

The tool set mirrors its default surface: `read`, `replace`, `replace_match`, `insert`, `copy`, `move`, `grep`, `undo_last_change`. `read` answers in `anchor│content` rows, and the built-in line editor is not registered.

Tau's own rules:

- The same tools face the user, ACP, and the desktop app: a tool surface in core.
- Anchors are session state, so they live in the session store and survive a restart.
- An anchor resolves to the file it was served for.
- Ship `outline`: a tree-sitter symbol skeleton, one anchored row per declaration, body spans read on demand. Off by default behind a config key, on in tau's own repo, with the corpus count as its cost meter.
- Ship the upstream config surface in tau user config, every knob at its upstream default: auto-read anchors, auto-read all, git repos only, ignore lists, diff context lines, per-tool toggles, required `path`, strict input, undo retention.
- These tool names are tau's canonical vocabulary, recorded in the glossary.

## Consequences

Cost: an anchor store per session in storage, and one tree-sitter dependency in the crate that serves the outline tool when enabled. Anchors are owned by core. The surface renders refusals and the post-edit diff. The tape format records anchored edits. The spec corpus uses anchored editing, so anchor churn becomes visible.

Eval: none needed.

Open: none.
