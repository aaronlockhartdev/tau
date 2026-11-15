# Tasks: adopt_observational_memory

- [ ] Scenario: R-session-19,20 observation and reflection thresholds, and the range a block supersedes
- [ ] Order: `adopt_block_log` archives first — R-session-19 and R-session-20 name blocks, which is that change's unit
- [ ] Scenario: R-session-21,22,25 session model, config budget, thresholds
- [ ] Scenario: R-session-23 observer sees the loaded context
- [ ] Scenario: R-session-24 prompt prefix stays byte-identical
- [ ] Scenario: R-session-26,27 counts to frontend, observations stored
- [ ] Implement: the observation and reflection slots as blocks in the log, no table, each naming the range it supersedes
- [ ] Implement: observer and reflector prompts
- [ ] Eval: observations versus the raw message tail, on demand
- [ ] Storage: the naming an observation carries (which range it supersedes) is a column of the memory leaf, with the leaf's own spec, and no clause here promises it
- [ ] spec-review before commit
