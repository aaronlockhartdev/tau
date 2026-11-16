# Tasks: adopt_observational_memory

- [ ] Scenario: R-session-19..26 in `crates/tau-test/tests/features/session.feature` (these IDs are the session
domain), steps in `crates/tau-test/src/steps/session.rs`: thresholds, the range a block supersedes, the
observer seeing the loaded files, the byte-stable prefix, the counts at the frontend
domain), steps in `crates/tau-test/src/steps/session.rs`: thresholds, the range a block supersedes, the
observer seeing the loaded files, the byte-stable prefix, the counts at the frontend
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual by hand
- [ ] Order: `adopt_block_log` archives first — R-session-19 and R-session-20 name blocks, which is that change's unit
- [ ] Scenario: R-session-21,22,25 session model, config budget, thresholds
- [ ] Scenario: R-session-23 observer sees the loaded context
- [ ] Scenario: R-session-24 prompt prefix stays byte-identical
- [ ] Scenario: R-session-26,27 counts to frontend, observations stored
- [ ] Implement: the observation and reflection slots as blocks in the log, no table, each naming the range it supersedes
- [ ] Implement: observer and reflector prompts
- [ ] Eval: observations versus the raw message tail, on demand
- [ ] docs: `docs/glossary.md` — `observation` and `reflection` as appended blocks naming a range; the live row
reads "a condensation of a session's oldest messages, stored in the session", which places them outside the log
- [ ] lane: the threshold scenarios in `crates/tau-test/tests/features/memory.feature`, steps in
`crates/tau-test/src/steps/memory.rs`; `STEPS.md` regenerates
- [ ] archive: lands after `adopt_block_log` (its clauses own blocks); at archive `spec/session/memory/spec.md`
is a new leaf, so `spec/session/spec.md`'s Leaves cell, `spec/index.md`'s session Leaves cell
and Corpus count move by the numbers read off the files
and Corpus count move by the numbers read off the files
- [ ] spec-review before commit
