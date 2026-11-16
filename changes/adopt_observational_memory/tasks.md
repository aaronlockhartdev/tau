# Tasks: adopt_observational_memory

- [ ] Order: `adopt_block_log` archives first — R-session-19 and R-session-20 name blocks, which is that
change's unit; `add_turn_loop` lands the request loop; `R-session-26`'s count reaches a frontend on the
event stream `add_realtime_gui_output` owns (`R-surface-08`), so it lands after that change too;
`R-session-23` reads the context files `adopt_context_file_loading` loads
- [ ] Scenario: R-session-19 and 20: the two thresholds, and the range an observation names
- [ ] Scenario: R-session-21, 22 and 25: the session's own model, the config budget, the threshold numbers
- [ ] Scenario: R-session-23: the observer sees the context files the agent sees
- [ ] Scenario: R-session-24: the prompt prefix stays byte-identical across a condensation
- [ ] Scenario: R-session-26 and 27: the counts reach the frontend, and a resume rebuilds observations from the log
- [ ] Lane: those scenarios in `crates/tau-test/tests/features/session.feature` (the IDs are the session domain), steps in `crates/tau-test/src/steps/session.rs`; `STEPS.md` regenerates
- [ ] R-session-19, 20: observations and reflections as blocks in the log, no table, each naming the range
it supersedes, read back through `R-session-46`'s slot read
- [ ] R-session-21, 23: the observer and reflector prompts, on the session's model, over the loaded context
- [ ] R-session-26: the count path from the memory leaf to the frontend event, and the step that reads it
- [ ] docs: `docs/glossary.md` — `observation` and `reflection`, each a row of 15 words at most, as blocks
appended to the log, an observation naming the range it supersedes and a reflection naming the observations
it condenses; the live `observation` row reads "a condensation of a session's oldest messages, appended
to its observations slot", which places it outside the log
- [ ] Eval: observations versus the raw message tail, on demand
- [ ] archive: `spec/session/memory/spec.md` is a new leaf, so `spec/session/spec.md`'s Leaves cell, `spec/index.md`'s session Leaves cell and Corpus count move by the numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
