# adopt_block_log

## Context

The store's unit is the turn: `turn(session, turn, state, committed)` with `events` keyed to it
(`crates/tau-core/src/storage/mod.rs:189-215`). One turn row carries a committed marker that gates
inserts, so the loop that fills it must close the turn last, and a state row per turn is where the
model's context was going to live.

`docs/glossary.md:31` already defines a turn as one request to the model and what it produced, while
R-session-02 makes a *tool call* the commit — the corpus names the request and the rule commits the
call, and the schema has no word for the call. The turn loop needs that word: a model request streams
reasoning, text, and several tool calls, and each of those closes at its own moment on the wire.

The provider stream is block-shaped. Anthropic streams `message_start`, `content_block_start`,
`content_block_delta`, `content_block_stop` (platform.claude.com, read 2026-11-15); the Responses API
streams output items with `added`/`delta`/`done` per part and reports true usage only in
`response.completed` (community.openai.com field guide, 53 event types, read 2026-11-15). Chat Completions
closes every block at `finish_reason` in one batch, and reports usage only in a final chunk, when asked
(developers.openai.com reference, read 2026-11-15).

State cannot be bytes on a mutable row: R-session-03's scan flags log content that appears in a
mutable table, and R-session-10's Why rules derived structures to be rebuilt, not reconciled.
Pointers from a mutable row into the log are the shape that keeps both clauses green.

## Options

- A. Blocks are the log's rows, one commit each; one request row per provider round-trip carries the
  model, usage, and terminal reason; state is pointers, appended on the block row; the committed
  marker retires. Evidence: the wire closes blocks at its own events (above); a scratch `sqlite3` probe,
  with no committed repro and an open Measure unit in tasks.md, at `page_size=4096`: 3000 blocks with
  200-byte payloads is 888,832 bytes, and 3000 request rows add 278,528 (≈93 bytes each, +31% at one
  request per block, ≈8% at four blocks per request);
  R-session-03 stays green because pointers are not payload text. Cost: no new dependency; one extra
write per request open and one per terminal; latency under contention, not cost: the archive's two-writer
  run measured a worst commit of 41 ms
  with `busy_timeout` 5 s (`changes/archive/2026-11-15-add_session_storage/tasks.md`); a schema rewrite.
- B. Keep the turn row and add a block key under it, keeping the marker. Evidence: it preserves 10
  clauses and the lane as written. Cost: the caller still closes the turn last, the marker still gates
  inserts, and a fork that copies a turn range copies rows that arrive uncommitted
  (R-session-08's Why names that defect today); the unit stays unnamed.
- C. Keep the schema and define "block" in prose. Evidence: none. Cost: two vocabularies — the last
  three review rounds each flagged a comment naming a rule the corpus does not carry, and this adds
  one at the centre of the store.

## Decision

A. The log's rows are blocks; a request row is one provider round-trip; state is pointers on the
block row; the committed marker and its trigger are deleted, because a block is committed by the
transaction that inserts it.

Consequences. The migration is rewritten in place: pre-v1, existing stores break by decision, no
upgrade path ships. `events` is renamed `blocks` (the FTS5 table is external-content, so it is dropped
and recreated against the new name, then rebuilt). The tables a reader may mutate become `sessions` and
`requests`, so the R-session-03 scan's mutable set changes with them. `Store::commit` takes a block,
`Store::run` answers requests with their blocks, `fork` copies blocks **and** the request rows they
name (R-session-08), `export_jsonl` writes block lines. Two reads land: the newest request of a
session, and the blocks after a sequence — `Store::run` materialises every payload and is the wrong
read for a resume.
Rules land as: R-session-35..40 in `spec/session/storage/spec.md` (the block unit, the request row,
state pointers, a block's request, the resume read, usage stamped at the terminal); R-session-02, 03,
04, 05, 06, 08, 10, 28, 31, 32 modified in place to the new unit; R-session-07 and R-session-30
retired with `Supersedes:`. A MODIFIED clause keeps its ID and is never renumbered; its heading follows the
unit, so four re-word: `Event and state commit together`, `Readers never hold across a turn`, `A committed
turn survives restart`, `A log row names a turn`. Terms in `docs/glossary.md`: `block`, `request`,
`injection`, `slot` added; `turn` and `committed marker` dropped; `log`, `run`, `session`, `steer` re-worded.
added; `turn` and `committed marker` dropped; `log`, `run`, `session`, `steer` re-worded.
Six open changes carry wording in the old unit — `add_subagent_support`, `add_realtime_gui_output`, `add_user_config`,
`add_test_lanes`, `adopt_context_file_loading`, `adopt_skills_convention` — and are re-worded by this
change, or the corpus ships two vocabularies.
`adopt_observational_memory` needs more than wording: its `R-session-19` promised to replace the oldest
messages, and `R-session-31` refuses any update to a log row, so that clause now appends an observation
block naming the range it supersedes.
The `crates/tau-protocol/src/bin/contracts.rs:1` comment cites R-workspace-04, which is not a live ID
(workspace runs 09..15): the citation moves to the invariant at `spec/index.md:7`.
Eval: none needed — a log's unit is mechanical, and the lanes count commits and rows.

Open: the fork copy is bounded by nothing, and the measured fix is a `blobs` table keyed by unique bytes
(`changes/archive/2026-11-15-add_session_storage/proposal.md`: `blobs (id INTEGER PRIMARY KEY, bytes TEXT
NOT NULL UNIQUE)`), 150,528 bytes per branch against 970,752 copied; it opens as its own change. Two writers
on one session stay refused by nothing (the gap recorded in
`changes/archive/2026-11-15-add_session_storage/tasks.md`). Oversized and binary payloads stay inline in
`payload` and indexed by FTS until the blobs change lands.
