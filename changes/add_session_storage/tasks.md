# Tasks: add_session_storage

- [x] proposal: one SQLite store, separation of duties by schema
- [x] R-session-01..13, 18: delta clauses in changes/add_session_storage/specs/session/storage/spec.md, with `R-session-12` withdrawn under `## REMOVED Requirements`
- [x] unblocked: the six crates exist, and `crates/tau-core/storage` owns the schema
- [x] R-session-01,02: one write connection; event and state in one transaction (crates/tau-core/storage), with scenarios in crates/tau-test/tests/features/session.feature and their steps in crates/tau-test/src/steps/session.rs
- [ ] R-session-03,04,05: append-only log; short readers; `synchronous = NORMAL`
- [x] R-session-06,07: the crash child `crates/tau-test/src/bin/crash_store.rs`, killed by a signal, with scenarios in crates/tau-test/tests/features/session.feature, and a unit test in crates/tau-core for the marker's transaction
- [ ] R-session-08,09: fork copies a range with a parent; compaction appends a summary
- [ ] R-session-10,11,13: FTS5 search; export command; harness queries the store
- [ ] R-session-01..11, 13: scenarios in crates/tau-test/tests/features/session.feature
- [x] R-session-28,29: `events.ts` and `sessions.created` in Unix milliseconds UTC; scenarios in crates/tau-test/tests/features/session.feature
- [x] R-session-30,31: triggers that refuse a late event and any update or delete of a log row; scenarios in crates/tau-test/tests/features/session.feature
- [x] R-session-32: the composite foreign key from `events` to `turn`; scenario in crates/tau-test/tests/features/session.feature
- [x] check: nextest · features · spec --check · steps --check · doc lint · links
- [ ] spec-review (reviewer ticks)
- [x] Scenario: R-session-18 store opens at `<project>/.tau/state.db` (`crates/tau-core/src/project.rs` names the path; the scenario is in crates/tau-test/tests/features/session.feature)
- [ ] at archive: the session row's Leaves cell in spec/index.md names storage/spec.md
