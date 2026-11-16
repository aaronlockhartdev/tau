# Tasks: adopt_block_log

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [x] IDs: this change claims `R-session-35..40, 45 and 46`; `add_turn_loop` claims 41..44 and lands after
this one, so the session range is split — the leaf holds 35..40, 45, 46 and `add_turn_loop`'s delta
holds 41..44, so the gap check counts open deltas and `spec -- --check` stays green across the split
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
- [x] R-session-02: a block and its state pointers commit in one transaction — the refused batch
  stores none of it (`a_refused_write_at_the_end_of_a_commit_stores_nothing` and its scenario), and
  the crash child proves a killed transaction leaves no row
- [x] R-session-03: the scan's mutable set becomes `sessions` and `requests` in `crates/tau-test/src/steps/session.rs`
- [x] R-session-04: the in-flight window scenario at block scale, with the checkpoint probe kept after the writer closes
- [x] R-session-05: `synchronous = NORMAL` stated for a committed block, not a committed turn (@R-session-05)
- [x] R-session-06: `crash_store` child — a block killed after its commit is present, a killed transaction leaves no row
- [x] R-session-08: `Store::fork` copies blocks and the request rows they name, and fork-resume appends without duplicating a copied block
- [x] R-session-10: the FTS index over `blocks`, with the search scenarios unchanged in result
- [x] R-session-11: `export_jsonl` writes one line per block, and the store is unchanged after the export
- [x] The mutable-state read — `MUTABLE` in `crates/tau-test/src/steps/session.rs` — names `sessions`
  and `requests`, the tables a store setup migrates; the R-session-03 scan is the step that reads
  both names; the migrations leg reads the mode and the schema name, the journal-upgrade leg only the mode
- [x] R-session-13: the store side answers a run from queries over blocks, proved by the lane's
  harness-read step over `run` and `latest_request`. The `eval` lane is `AGENTS.md`'s `M0` row,
  and no `eval` bin or suite file is in the tree, so nothing proves the harness end to end
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
  session left (mutation: mapping by `seq + (next - from)` fails the equality with
  `left: (Some("p1"), Some("p3"))` and `right: (Some("p1"), Some("p2"))` on their own lines — the
  pointer lands on the child's copy of a block the parent never named)
- [x] R-session-40: `stamp_request` writes the terminal once — the statement carries
  `AND reason IS NULL`, and no row is `Error::Terminal`. A scenario stamps twice and reads the
  first counts back (mutation: dropping the guard answers `the first terminal answers its counts
  and its reason: a second terminal stamped over the first; the reason is Some("refusal"), expected
  "completed"; the input count is Some(1), expected 120`)
- [x] R-session-40: the stamp is scoped to its session, and each usage count rides its own column.
  `a_stamp_lands_on_the_session_it_names` stamps one session's request 1 and reads the other session's
  request 1 open (mutation: dropping `session = ?1` answers `session b's request 1 is still open`); a
  scenario stamps a provider that reported all four and reads them back column by column
  (mutation: swapping the two cache columns answers `each usage count answers in its own column:
  the request's cache read count is Some(70), expected 900; the request's cache write count is
  Some(900), expected 70`)
- [x] R-session-37: the guard's four arms each have a check of their own, each proved by deleting the
  arm. The pair's wholeness: `the block whose state has an end and no beginning was written` is the
  half that stores once the conjunct is gone, and `the block carrying half a state was written`
  becomes `the block carrying half a state failed as a state pointer names no block of the session,
  expected CHECK constraint failed` — the trigger's message, not the `CHECK`'s.
  Its direction: `the block whose state runs backwards was written`. A pointer naming
  nothing at its end: `the block whose state ends on no block was written`. A start naming nothing
  while the end names a row needs a gap in the session's sequences, so
  `a_state_pointer_whose_start_names_nothing_is_refused` covers it; neutering the start disjunct is
  caught by that test and nothing else. A pointer into a sibling session's sequence needs two
  sessions and a sequence only one of them owns, so
  `a_state_pointer_into_another_session_is_refused` covers it; dropping the trigger's
  `b.session = NEW.session` from both disjuncts turns three tests red — this one, the start-arm
  test, and the end-border test below — and leaves every scenario green. The `CHECK` fires before
  documentation — so a half pair and a pair that runs backwards land on the `CHECK`, and a pair
  naming nothing lands on the trigger: at its start, at its end, or across a session's border
- [x] R-session-37: the pair's session border has a check at each end, alone.
  `a_state_pointer_into_another_session_is_refused` names a sibling's row at both ends,
  `a_state_pointer_whose_start_names_another_session_is_refused` names one at the start while the
  end names a row of the carrying session, and
  `a_state_pointer_whose_end_names_another_session_is_refused` names one at the end while the start
  names one (mutations: dropping `b.session = NEW.session` from the end disjunct turns only the
  end-border test red; from the start disjunct, the start-border test and the start-arm test; the
  lane stays green under both)
- [x] R-session-36: `a_request_row_is_opened_once` opens one round-trip twice and refuses the second
  row, which is the table's key standing where R-session-36 promises one row per round-trip
  (mutation: dropping `PRIMARY KEY (session, request)` answers the second open with an `Ok` value)
- [x] R-session-36: the request row records its provider, model, counts and reason, and no payload: a
  scenario reads `pragma_table_info('requests')` and refuses the column's presence
- [x] lane: a scenario that opens on a `When` clears `world.findings` itself, so a setup scenario
  cannot fail by quoting the scenario before it
- [x] R-session-08: the copied request row carries its provider, model and four counts, not only its
  number and terminal (mutation: copying number and model alone answers `the fork's request names
  None as its provider, expected lane-request-1; the fork's request usage_in is None, expected
  Some(120); the fork's request usage_cache_read is None, expected Some(900); …`)
- [x] R-session-39: the clause states the narrow read's scope — the newest request and the blocks
  that follow it — because what a read costs in payloads is not observable through the store's API,
  and the scenario counts the rows each read answers
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
  burns its 100 ms busy timeout before the backoff, so a hypothetical 3 s beat there would leave ~50 ms of that budget, which is why the lane's beat is 400 ms
- [x] R-session-07, R-session-30: the archive retires both clauses, and their scenario tags leave the
corpus in the same commit — the late-event scenario is deleted and the killed-process scenario keeps
`@R-session-06` alone, so no tag names a retired clause and no live clause loses its ref. The marker
is absent from the schema and a scenario reads that absence
- [x] R-session-33, R-session-34: the setup-race steps renamed to the new unit, behaviour unchanged
- [x] lane: re-word `crates/tau-test/tests/features/session.feature` and `crates/tau-test/src/steps/session.rs` to blocks and requests; `STEPS.md` regenerates
- [x] docs: `docs/glossary.md` adds `block`, `request`, `injection`, `slot`, `user input`, `usage`,
  `terminal`, `state`, `stream`, `resume`, `payload`, `context` and `JSONL`; drops `turn` and
  `committed marker`; re-words `fork`,
  `log`, `run`, `session`, `steer`, `event time` and `session start output`. The live cells named
  `events`, turns and the committed marker — the unit this change retires — and `fork` named a
  prompt cache, which is the subagent change's unit. The proposal's term list predates this sweep;
  this list is the tree's
- [x] docs: `docs/stack.md` Storage row names `blocks` and `requests`, the state as a pair of
  pointers into the log, and the clauses the storage leaf covers at its archive: R-session-01..06,
  08, 10, 11, 13, 18, 28, 29, 31..40, 45, 46 — 07 and 30 retired by this change, 41..44 pending in
  `add_turn_loop`
- [x] docs: `crates/tau-protocol/src/bin/contracts.rs:1`, `:67` and `:75` cite R-workspace-04, a clause
  `change_spec_requirements` issues and that goes live at its archive; the comments cite the clause
  where it applies and carry no marker of their own, and the citation stays (R-workspace-04)
- [ ] docs: sweep the deltas this change does not re-format: bare `## ADDED` markers to `## ADDED Requirements`
(`add_subagent_support` 4 leaves, `add_surface_components`, `add_test_lanes`, `adopt_context_file_loading`,
`adopt_hash_anchor_editing`, `adopt_skills_convention`); headings over the six-word cap (`R-config-01`,
`R-config-05`, `R-acp-01`, `R-acp-04`, `R-acp-06`, `R-surface-01`, `R-surface-02`, `R-surface-07`,
`R-surface-21`, `R-tools-02`, `R-tools-06`, `R-tools-07`, `R-tools-09`, `R-tools-11`, `R-tools-14`,
`R-tools-28`, `R-harness-04`); the `THE SYSTEM SHALL NOT` obligations that sit outside the five EARS
forms (`R-context-04`, `R-tools-16`, `R-tools-23`, `R-tools-28`, `R-surface-02`, `R-harness-08`); and the
`Apply to` line naming the live leaf each delta applies to, missing from those six deltas
- [x] re-worded: the seven open changes no longer speak the retired unit as a live concept — directed
messages are blocks in each participant's log (`add_subagent_support`, and its client `prompt` queues to the
next request), a block shows its own event time (`add_realtime_gui_output`), no config value reaches a
session block (`add_user_config`), and skill and context costs are counted per request
(`adopt_skills_convention`, `adopt_context_file_loading`); `change_spec_requirements` names the
committed-marker clause as retired. `grep -rilw --exclude-dir=archive -e turn -e turns -e event -e
events -e committed changes/` answers 17 files (the command above, run at this archive), every hit
one of: a stream event (the provider's,
numbered by R-protocol-01; a child's, in `add_subagent_support`'s acp delta; core's event stream,
which R-surface-08 owns), the glossary's `event time`, the store's `commit` or git's `committed`,
the noun in a quote of the retired design (`change_spec_requirements`'s "the store of turns and
events"), the words "the turn loop" in `add_turn_loop`'s prose, and the verb "turns". That change
carries the sweep of its prose to "the request loop"
- [ ] Measure: the cost claim in the proposal, reproduced with `sqlite3` at `page_size=4096` — 3000 blocks of
200-byte payload in the current schema, and the same with 3000 request rows added. The row sizes and the two
percentages in the proposal come off a scratch probe, so they carry no committed repro until this lands.
- [ ] sweep: the clause layout is the template's — obligation, `Acceptance:` and `Why:` adjacent, with no
blank line between them. The live corpus has none, and none of the deltas these changes own has one. A
clause whose `Acceptance:` sits in its own paragraph reads as a second obligation. This counts them, skipping
`REMOVED` headings because a REMOVED block carries no obligation:

`awk 'FNR==1{prev=""} /^Acceptance:/{if(prev=="" && pprev !~ /^###/) n++} {pprev=prev; prev=$0} END{print n+0}' $(find changes -name "*.md" -not -path "*/archive/*")`

It prints 66 today: `add_subagent_support` 27, `adopt_context_file_loading` 10, `adopt_skills_convention` 8,
`adopt_hash_anchor_editing` 8, `add_surface_components` 7, `add_test_lanes` 6. This sweep takes the 60
in the five changes this range does not own; `add_test_lanes` re-formats its own 6. The form is the
template's, in
`changes/_template/specs/_domain_/_area_/spec.md`

Carried: the three lines above stay open, and each has a named home. The delta-format sweep and the
clause-layout sweep belong with `change_spec_requirements`, the change that owns the spec format, and
land there as its task lines — the format sweep's line there names the `IDs:` line too, which ten of
the twelve open changes lack (`add_realtime_gui_output` and `add_turn_loop` carry one). The Measure
belongs with the `blobs` change this proposal promises to open, which is not in `changes/` yet, because
the numbers it reproduces are that option's evidence and the schema to measure is the one this change
landed. The `spec-review` line below stays open until a reviewer ticks it.

Rows that read nothing today and stay green: `contracts current` compares an empty generated set (teeth
come with R-protocol-02 in `add_turn_loop`); `spec tool` prints a corpus count no row compares to
anything, so `spec/index.md`'s number carries no gate; `no local state tracked` asks whether the
tracked set under `target` and `.tau` is empty, and both paths are gitignored, so that set is empty by
construction and only `git add -f` reaches it. The closed name lists (`core has no runtime`, `core has
no UI logic`, `core has no HTTP client`, `core installs no subscriber`) match dependency names, so a
hand-rolled client in core's own source, or a subscriber installed there under another name, stays
green; the clauses those two rows answer to are pending in `add_turn_loop` (R-workspace-16..19) and
speak core's source and graph and every install site, so the rows are narrower than they will need to
be. `unit + surface` text-matches `fail-fast = false` inside the `[profile.ci]` section — an awk over
lines, not a config parse, so a header indented out of column 1, a section that never closes, or
`fail-fast=false` without spaces slips past it — and `-P ci` selects a profile by name. Separately,
and in the other direction: `core spawns nothing` greps prose too, so a comment naming `.spawn(`
reddens
it — a false red, not a false green.

- [x] archive: `spec -- archive adopt_block_log` ran; the leaf gained 8 (35..40, 45, 46) and retired 2
(07, 30), so `spec/session/storage/spec.md` reads 25 requirements and `spec/index.md`'s corpus 33,
both counted off the files. `add_turn_loop` lands after, and its 41..44 fill the gap this leaf leaves
- [x] check: `cargo run -p tau-tools --bin gates` — 17 `run` rows PASS, `live output under load`,
`headless session` and `desktop` report `skipped (M0)`, `eval` reports `skipped (M0)`; the
workspace layout row is checked by hand, and the lane runs 45 scenarios with 9 tau-core unit tests
- [ ] spec-review (reviewer ticks)
