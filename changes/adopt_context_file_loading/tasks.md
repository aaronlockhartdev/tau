# Tasks: adopt_context_file_loading

- [ ] Scenario: R-context-01,02 root guide loads, nearest wins
- [ ] Scenario: R-context-03,04 CLAUDE.md tie-break, `.pi/` never read
- [ ] Scenario: R-context-05,08 `.tau` project layer, one file per role
- [ ] Scenario: R-context-06,07 SYSTEM.md replaces, APPEND_SYSTEM.md appends
- [ ] Scenario: R-context-09,10 no unasked load, loaded set reported
- [ ] Implement: context assembly in core, shared by both frontends
- [ ] archive: `spec/context/load-order/spec.md` is a new leaf, so `spec/context/spec.md`'s count and
Leaves cell (+10, `load-order`), `spec/index.md`'s context Leaves cell and the Corpus count move by the
numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
