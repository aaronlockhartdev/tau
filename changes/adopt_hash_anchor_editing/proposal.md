# Adopt hash-anchor editing

## Context

Line-number and fuzzy-matched edits silently hit the wrong line. Anchor-addressed editing is the precondition for letting an agent edit files at all.

## Options

- Mirror pi-hashline-edit-pro 7.1.1: its README and `docs/` (read 2026-11-13), a shipped extension in daily use on this repo's own documents. Cost: one tools leaf spec, an anchor store in session state.
- Line numbers with fuzzy matching: no external evidence in its favour; a stale line number edits the wrong line without an error.

## Decision

Mirror `pi-hashline-edit-pro` 7.1.1 exactly. For mechanics, read its README and `docs/`. Do not restate them here, and do not diverge. That covers the anchor format and alphabet, anchor lifetime, stale and ambiguous refusals, batching, the post-edit diff and its anchor rows, undo, and the caps.

The tool set mirrors its default surface: `read`, `replace`, `replace_match`, `insert`, `copy`, `move`, `grep`, `undo_last_change`. `read` answers in `anchor│content` rows, and the built-in line editor is not registered.

Tau's own rules:

- The same tools face the user, ACP, and the desktop app: a tool surface in core.
- Anchors are session state, so they live in the session store and survive a restart.
- An anchor resolves to the file it was served for.
- Add `outline` (tree-sitter symbol skeleton), off by default.
- These tool names are tau's canonical vocabulary, recorded in the glossary.

## Consequences

Anchors are owned by core. The surface renders refusals and the post-edit diff. The tape format records anchored edits. The spec corpus uses anchored editing, so anchor churn becomes visible.

Open: adopt the upstream config surface in tau user config (auto-read all, diff context lines, per-tool toggles), and what default each ships with?
