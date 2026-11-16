# Tasks: adopt_context_file_loading

- [ ] Scenario: R-context-01,02 root guide loads, nearest wins
- [ ] Scenario: R-context-03,04 CLAUDE.md tie-break, `.pi/` never read
- [ ] Scenario: R-context-05,08 `.tau` project layer, one file per role
- [ ] Scenario: R-context-06,07 SYSTEM.md replaces, APPEND_SYSTEM.md appends
- [ ] Scenario: R-context-09,10 no unasked load, loaded set reported
- [ ] Implement: context assembly in core, shared by both frontends
- [ ] archive: `spec/context/load-order/spec.md` is a new leaf of 10 clauses, so `spec/context/spec.md`'s
Leaves cell gains `load-order/spec.md (10 requirements)` and its count line stays, `spec/index.md`'s Leaves
column gains the link, and the Corpus count moves by 10
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
