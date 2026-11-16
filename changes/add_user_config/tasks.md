# Tasks: add_user_config

- [ ] Scenario: R-config-01..03 root, XDG layer, project layer merged
- [ ] Scenario: R-config-04,05 write preserves, live key applies
- [ ] Scenario: R-config-06,07 restart key marked in file and announced
- [ ] Scenario: R-config-08 frontends share one config state
- [ ] Scenario: R-config-09 every listed key applies with its default
- [ ] Scenario: R-config-10 no config value in a session event
- [ ] Scenario: R-config-11,12 table merge, array replace, reload either layer
- [ ] Implement: TOML read, write, and layer merge in core via `toml_edit`
- [ ] Implement: `notify` on both layers, announced on core's event stream
- [ ] Implement: frontends subscribe; ACP exposes the same state
- [ ] R-config-13: the key table in `docs/stack.md`'s Config row, naming the queue cap, the token budget,
the wall clock limit and the repeated-tool-call stop keys `R-agent-11` and `R-agent-12` ask for, the
observation and reflection thresholds `R-session-25` reads, and the memory context budget
`R-session-22` bounds memory by, each with a default (@R-config-13)
- [ ] docs: `docs/stack.md`'s Config row gains the pins (`toml_edit`, `notify`)
— the keys themselves are R-config-13's unit above
- [ ] archive: `spec/config/reload/spec.md` is a new leaf, so `spec/config/spec.md`'s Leaves cell,
`spec/index.md`'s config Leaves cell and the Corpus count move by the numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
