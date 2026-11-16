# Tasks: adopt_block_log

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [ ] gates: confirm `core has no UI logic`, `core has no runtime`, `core spawns nothing` stay green over the renamed schema (no new rows in this change)
- [ ] R-session-35: delta clause in changes/adopt_block_log/specs/session/storage/spec.md, then rewrite the migration in place in `crates/tau-core/src/storage/mod.rs` — `events` → `blocks`, drop the `turn` table, the `committed` column and `events_no_late_insert`, add `requests`, rebuild `blocks_fts` (external content: drop, recreate, rebuild); pre-v1, so no store upgrade ships
- [ ] R-session-35: `Store::commit(&Block)` — one transaction per block, and one transaction for the batch a stream closes at once, store-wide sequence
- [ ] R-session-36: the request row (session, request, provider_id, model, four usage counts, reason) and its open write
- [ ] R-session-37: state pointers on the block row; drop `turn.state` and `Store::turn_state`
- [ ] R-session-38: the request column, nullable, and the user-input and injection cases in the lane
- [ ] R-session-39: `Store::latest_request` and `Store::blocks_after` reads; the mutation that proves `Store::run` is not the resume read
- [ ] R-session-40: the terminal stamp, and a request row with a reason and no block
- [ ] R-session-02: the block-and-state transaction, proved by the crash child
- [ ] R-session-03: the scan's mutable set becomes `sessions` and `requests` in `crates/tau-test/src/steps/session.rs`
- [ ] R-session-04: the in-flight window scenario at block scale, with the checkpoint probe kept after the writer closes
- [ ] R-session-06: `crash_store` child — a block killed after its commit is present, a killed transaction leaves no row
- [ ] R-session-08: `Store::fork` copies blocks and the request rows they name, and fork-resume appends without duplicating a copied block
- [ ] R-session-10: the FTS index over `blocks`, with the search scenarios unchanged in result
- [ ] R-session-11: `export_jsonl` writes one line per block
- [ ] R-session-13: `Store::run` answers requests with their blocks; drop `RunTurn`'s per-turn state
- [ ] R-session-28: the event time lands on the block row
- [ ] R-session-31: the update and delete triggers name `blocks`
- [ ] R-session-32: the composite FK to `requests`, and the orphan-row refusal
- [ ] R-session-07: delete the `turn.committed` column, the `events_no_late_insert` trigger, the `RunTurn.committed` field, and the scenarios tagged for it
- [ ] R-session-30: delete the late-event scenarios; `blocks` immutability is R-session-31's
- [ ] R-session-33, R-session-34: the setup-race steps renamed to the new unit, behaviour unchanged
- [ ] lane: re-word `crates/tau-test/tests/features/session.feature` and `crates/tau-test/src/steps/session.rs` to blocks and requests; `STEPS.md` regenerates
- [ ] docs: `docs/glossary.md` — add `block`, `request`, `injection`, `slot`; drop `turn` and `committed marker`;
re-word `log`, `session`, `steer`, `session start output`; its `log` and `session` cells name the `events`
and its `run` cell reads "a session's turns in order, with the events each produced", the unit this change
retires, while `add_turn_loop` says "stop the loop" with no `loop` entry of its own — all of it re-worded here
- [ ] docs: `docs/stack.md` Storage row names the new tables and the clause ranges
- [ ] docs: `crates/tau-protocol/src/bin/contracts.rs:1` cites R-workspace-04, which is not live; point it at the invariant in `spec/index.md`
- [ ] re-word the open changes written in the old unit — `add_subagent_support` (its delta specs),
`add_realtime_gui_output`, `add_user_config`, `add_test_lanes`, `adopt_context_file_loading`,
`adopt_skills_convention`; a word-boundary grep (`grep -rilwE --exclude-dir=archive
'turn(s)?|event(s)?|committed' changes/*/`) finds those six and no other change; the hits in `_template`,
`adopt_block_log`, `add_turn_loop`, `adopt_observational_memory`, `change_spec_requirements` and
`spike_gpui_surface` are the new unit's own words, and `adopt_hash_anchor_editing`'s are `return`
- [ ] Measure: the cost claim in the proposal, reproduced with `sqlite3` and `page_size=4096` — 3000 blocks of
200-byte payload in the current schema, and the same with 3000 request rows added; the row sizes and the two
percentages in the proposal come off that run, which is a scratch probe, so it carries no committed repro.
- [ ] archive: lands before `add_turn_loop`, whose clauses name blocks and requests; at archive
`spec/session/storage/spec.md` gains 6 requirements and retires 2, its count and `spec/session/spec.md`'s
leaf count and `spec/index.md`'s Corpus count move by the same numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates` (runs every `run` row: ci lane reports the rest — spec tool,
spec lint, contracts current, doc lint, links, unit + surface, behaviour, steps current, dependencies
current, no local state tracked, toolchain pinned, core has no UI logic, core has no runtime, core spawns
nothing, core has no HTTP client, core installs no subscriber, lints); rows in state M0 — live output under
load, headless session, desktop — reported `skipped (M0)`; manual rows — workspace layout, eval — reported
by hand
- [ ] spec-review (reviewer ticks)
