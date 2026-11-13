# adopt_hash_anchor_editing

## Context

Line-number and fuzzy-matched edits silently hit the wrong line. Anchor-addressed editing is the
precondition for letting an agent edit files at all.

## Decision

Mirror `pi-hashline-edit-pro` 7.1.1 exactly: 5-character per-line anchors, `anchor│content` rows,
anchor-only addressing, stale/ambiguous refusal with fresh anchors, the nine tools, anchor-ownership
file resolution, post-edit diff anchor states, single-level per-file undo. The reference defines the
grammar and error codes — README + `docs/`
([repo](https://github.com/YuGiMob/pi-hashline-edit-pro)); do not restate it in the spec.

Consequences owned by tau: the anchor table is core per-session state (upstream persists it in
SQLite); every file-reading tool must emit anchored rows, since a bare-text read breaks the editing
contract; write tools return fresh anchors; binary and UTF-16 inputs are refused, not decoded.

Open: expose `outline` (off by default upstream) at v1? `[NEEDS CLARIFICATION]`
