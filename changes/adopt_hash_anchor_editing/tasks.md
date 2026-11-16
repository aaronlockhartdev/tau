# Tasks: adopt_hash_anchor_editing

- [ ] Scenario: R-tools-22,24,25 anchored rows, one-file resolve, stale refusal
- [ ] Implement: anchors as a session state table
- [ ] Scenario: R-tools-23 no line-number editor registered
- [ ] Scenario: R-tools-26,27 diff rows returned, single undo
- [ ] Scenario: R-tools-28,29 outline off by default, skeleton rows
- [ ] Implement: anchor store in the session store
- [ ] Implement: outline through tree-sitter behind the config key
- [ ] archive: `spec/tools/editing/spec.md` is a new leaf, so `spec/tools/spec.md`'s count and Leaves cell
(+8, `editing`), `spec/index.md`'s tools Leaves cell and the Corpus count move by the numbers read off the
files
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
