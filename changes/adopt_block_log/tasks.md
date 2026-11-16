# Tasks: adopt_block_log

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [ ] IDs: this change claims `R-session-35..40, 45 and 46`; `add_turn_loop` claims 41..44 and lands after
this one, so the session range is split until that change archives — the gap check counts open deltas, so
`spec -- --check` stays green across the split
- [x] gates: confirm `core has no UI logic`, `core has no runtime`, `core spawns nothing` stay green over the renamed schema (no new rows in this change)
- [x] R-session-35: delta clause in changes/adopt_block_log/specs/session/storage/spec.md, then rewrite the migration in place in `crates/tau-core/src/storage/mod.rs` — `events` → `blocks`, drop the `turn` table, the `committed` column and `events_no_late_insert`, add `requests`, rebuild `blocks_fts` (external content: drop, recreate, rebuild); pre-v1, so no store upgrade ships
- [x] R-session-35: `Store::commit(&Block)` — one transaction per block, and one transaction for the batch a stream closes at once, store-wide sequence
- [x] R-session-36: the request row (session, request, provider_id, model, four usage counts, reason) and its open write
- [x] R-session-37: state pointers on the block row; drop `turn.state` and `Store::turn_state`
- [x] R-session-38: the request column, nullable, and the user-input and injection cases in the lane
- [x] R-session-39: `Store::latest_request` and `Store::blocks_after` reads; the mutation that proves `Store::run` is not the resume read
- [x] R-session-45: the slot on the block row, and an injection block that fills one
- [x] R-session-46: the slot read, answering a session's blocks by the slot they record
- [x] R-session-40: the terminal stamp, and a request row with a reason and no block
- [x] R-session-02: the block-and-state transaction, proved by the crash child
- [x] R-session-03: the scan's mutable set becomes `sessions` and `requests` in `crates/tau-test/src/steps/session.rs`
- [x] R-session-04: the in-flight window scenario at block scale, with the checkpoint probe kept after the writer closes
- [x] R-session-05: `synchronous = NORMAL` stated for a committed block, not a committed turn (@R-session-05)
- [x] R-session-06: `crash_store` child — a block killed after its commit is present, a killed transaction leaves no row
- [x] R-session-08: `Store::fork` copies blocks and the request rows they name, and fork-resume appends without duplicating a copied block
- [x] R-session-10: the FTS index over `blocks`, with the search scenarios unchanged in result
- [x] R-session-11: `export_jsonl` writes one line per block, and the store is unchanged after the export
- [x] The mutable-state read in `crates/tau-test/src/steps/session.rs:579` names `sessions` and `turn`; it
names `sessions` and `requests` after this change, and R-session-03's clause says the two by name
- [x] R-session-13: the harness read answers from store queries over blocks, and `RunTurn`'s per-turn state
is dropped
- [x] R-session-28: the event time lands on the block row
- [x] R-session-31: the update and delete triggers name `blocks`
- [x] R-session-32: the composite FK to `requests`, and the orphan-row refusal
- [x] R-session-32, 37: the keys bind every row a commit writes — `blocks.session` references
  `sessions`, a state is stored as a pair and in order (`CHECK`), and a trigger refuses a pointer
  naming no block of the session. Scenarios refuse a block for an unrecorded session (mutation:
  dropping the reference answers `the block naming an unrecorded session was written`), half a
  state, a pair that runs backwards, and a pair naming nothing (mutation: `RAISE(IGNORE)` answers
  `the block whose state names no block was written`)
- [x] R-session-37, R-session-08: a fork re-points a state by the pointer's rank among the copied
  sequences, and drops a pair naming a block the range leaves behind. Scenarios fork over the whole
  range and over part of it; `a_fork_moves_the_pointers_across_a_gap` forks across a gap another
  session left (mutation: mapping by `seq + (next - from)` answers `left: Some((8, 10)) right:
  Some((8, 9))` — the pair lands on the child's third copy, not its second)
- [x] R-session-40: `stamp_request` writes the terminal once — the statement carries
  `AND reason IS NULL`, and no row is `Error::Terminal`. A scenario stamps twice and reads the
  first counts back (mutation: dropping the guard answers `a second terminal stamped over the
  first; the reason is Some("refusal"), expected "completed"`)
- [x] R-session-40: `crash_store`'s `blocks` mode commits the blocks and stops before the stamp;
  a scenario kills in that window, and the reopen reads every block with an open request that a
  further block names
- [x] R-session-08: the forked child opens a request of its own and appends, and the scenario
  counts the copy plus the child's block with no sequence reused
- [x] R-session-39: the resume scenario logs two round-trips, so a read of the oldest request, not
  the newest, answers it red
- [x] R-session-34: the bounded scenario runs the journal-upgrade leg, where a standing lock is
  refused on the spot, so its budget is 20 attempts of a 50 ms backoff (~1 s) and the 5 s beat
  leaves ~4 s for the refusal to land. The migration leg is the slow one, ~2.95 s: every attempt
  burns its 100 ms busy timeout before the backoff, so a 3 s beat there leaves ~50 ms
- [ ] R-session-07: delete the `turn.committed` column, the `events_no_late_insert` trigger and the
`RunTurn.committed` field, and the scenarios tagged `@R-session-07` in the same archive commit that retires
the clause — the tag and the ref leave the corpus together, so `spec --check` stays green
- [ ] R-session-30: the late-event scenarios tagged `@R-session-30` go in the same archive commit that retires
the clause; `blocks` immutability is R-session-31's
- [x] R-session-33, R-session-34: the setup-race steps renamed to the new unit, behaviour unchanged
- [x] lane: re-word `crates/tau-test/tests/features/session.feature` and `crates/tau-test/src/steps/session.rs` to blocks and requests; `STEPS.md` regenerates
- [ ] docs: `docs/glossary.md` — add `block`, `request`, `injection`, `slot`, `user input`, `usage`, `terminal`
and `state`; drop `turn` and `committed marker`; re-word `log`, `run`, `session`, `steer` and `session start
output`. The live `log` cell names `events`, `run` reads "a session's turns in order, with the events each
produced", `turn` reads "one request to the model and what it produced, numbered per session", and
`session` names turns: all of it is this change's unit
- [ ] docs: `docs/stack.md` Storage row names the new tables and the clause ranges
- [ ] docs: `crates/tau-protocol/src/bin/contracts.rs:1`, `:66` and `:74` cite R-workspace-04, a clause
`change_spec_requirements` issues and that goes live at its archive; each comment marks the clause pending, and
the citation stays (R-workspace-04)
- [ ] docs: sweep the deltas this change does not re-format: bare `## ADDED` markers to `## ADDED Requirements`
(`add_subagent_support` 4 leaves, `add_surface_components`, `add_test_lanes`, `adopt_context_file_loading`,
`adopt_hash_anchor_editing`, `adopt_skills_convention`); headings over the six-word cap (`R-config-01`,
`R-config-05`, `R-acp-01`, `R-acp-04`, `R-acp-06`, `R-surface-01`, `R-surface-02`, `R-surface-07`,
`R-surface-21`, `R-tools-02`, `R-tools-06`, `R-tools-07`, `R-tools-09`, `R-tools-11`, `R-tools-14`,
`R-tools-28`, `R-harness-04`); the `THE SYSTEM SHALL NOT` obligations that sit outside the five EARS
forms (`R-context-04`, `R-tools-16`, `R-tools-23`, `R-tools-28`, `R-surface-02`, `R-harness-08`); and the
`Apply to` line naming the live leaf each delta applies to, missing from those same 9 deltas
- [ ] re-word the open changes written in the old unit: `add_subagent_support` (its delta specs),
`add_realtime_gui_output`, `add_user_config`, `add_test_lanes`, `adopt_context_file_loading`,
`adopt_skills_convention` and `change_spec_requirements` (its proposal states the retired committed marker as
landing live). `grep -rilw --exclude-dir=archive -e turn -e turns -e event -e events -e committed changes/`
finds those seven, plus `adopt_block_log`, `add_turn_loop`, `adopt_observational_memory` and
`spike_gpui_surface`, which speak the new unit themselves, not the old one
- [ ] Measure: the cost claim in the proposal, reproduced with `sqlite3` at `page_size=4096` — 3000 blocks of
200-byte payload in the current schema, and the same with 3000 request rows added. The row sizes and the two
percentages in the proposal come off a scratch probe, so they carry no committed repro until this lands.
- [ ] sweep: the clause layout is the template's — obligation, `Acceptance:` and `Why:` adjacent, with no
blank line between them. The live corpus has none, and none of the deltas these changes own has one. A
clause whose `Acceptance:` sits in its own paragraph reads as a second obligation. This counts them, skipping
`REMOVED` headings because a REMOVED block carries no obligation:

`awk 'FNR==1{prev=""} /^Acceptance:/{if(prev=="" && pprev !~ /^###/) n++} {pprev=prev; prev=$0} END{print n+0}' $(find changes -name "*.md" -not -path "*/archive/*")`

It prints 66 today: `add_subagent_support` 27, `adopt_context_file_loading` 10, `adopt_skills_convention` 8,
`adopt_hash_anchor_editing` 8, `add_surface_components` 7, `add_test_lanes` 6. This sweep takes the 60 that
belong to no change in this range; `add_test_lanes` re-formats its own 6. The form is the template's, in
`changes/_template/specs/_domain_/_area_/spec.md`

- [ ] archive: lands before `add_turn_loop`, whose clauses name blocks and requests; at archive
`spec/session/storage/spec.md` gains 8 requirements and retires 2; `spec/session/spec.md`'s storage leaf
count and `spec/index.md`'s Corpus count move by the same numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates` (runs every `run` row: ci lane reports the rest — spec tool,
spec lint, contracts current, doc lint, links, unit + surface, behaviour, steps current, dependencies
current, no local state tracked, toolchain pinned, core has no UI logic, core has no runtime, core spawns
nothing, core has no HTTP client, core installs no subscriber, lints); rows in state M0 — live output under
load, headless session, desktop — reported `skipped (M0)`; manual rows — workspace layout, eval — reported
by hand
- [ ] spec-review (reviewer ticks)
