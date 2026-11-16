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
- [ ] Implement: the key table, naming the queue cap, the token budget, wall clock limit and repeated-tool-call
stop keys `R-agent-11` and `R-agent-12` ask for, the observation and reflection thresholds `R-session-25` reads,
and the context budget `R-session-22` bounds memory by
- [ ] archive: at archive `spec/config/reload/spec.md` is a new leaf, so `spec/config/spec.md`'s Leaves cell
and `spec/index.md`'s config Leaves cell and Corpus count move by the numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
