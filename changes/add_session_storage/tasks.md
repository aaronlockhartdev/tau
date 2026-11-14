# Tasks: add_session_storage

- [x] proposal: one SQLite store, separation of duties by schema
- [x] R-session-01..13, 18: delta clauses in changes/add_session_storage/specs/session/storage/spec.md, with `R-session-12` withdrawn under `## REMOVED Requirements`
- [x] unblocked: the six crates exist, and `crates/tau-core/storage` owns the schema
- [x] R-session-01,02: one write connection; event and state in one transaction (crates/tau-core/storage), with scenarios in crates/tau-test/tests/features/session.feature and their steps in crates/tau-test/src/steps/session.rs
- [x] R-session-03,04,05: event content lands in the log and in no state table (every table and column of the store is scanned by name, NULL values included), a read between turns leaves no read transaction open (a `TRUNCATE` checkpoint on a reader connection reports no lock and zero frames left), `synchronous = NORMAL` read from the store's own connection; scenarios in crates/tau-test/tests/features/session.feature
- [ ] R-session-04's `WHILE` window has no acceptance: no step opens a turn, so no reader is measured across one. The shape is a second connection that opens a read transaction across a turn and still holds it at the checkpoint.
- [x] R-session-06,07: the crash child `crates/tau-test/src/bin/crash_store.rs`, killed by a signal, with scenarios in crates/tau-test/tests/features/session.feature, and a unit test in crates/tau-core for the marker's transaction
- [x] R-session-08,09: `Store::fork` copies an inclusive turn range into a session that names its parent, `Store::compact` appends a summary event and sets `sessions.visible_from_turn`; scenarios in crates/tau-test/tests/features/session.feature
- [ ] How a fork resumes has no clause: the copy lands uncommitted, so the fork's last turn is open and a next tool call writes into the copied turn under its old number, and readers see a complete turn as uncommitted.
- [ ] R-session-10,11,13: FTS5 search; export command; harness queries the store
- [ ] R-session-01..11, 13: scenarios in crates/tau-test/tests/features/session.feature
- [x] R-session-28,29: `events.ts` and `sessions.created` in Unix milliseconds UTC; scenarios in crates/tau-test/tests/features/session.feature
- [x] R-session-30,31: triggers that refuse a late event and any update or delete of a log row; scenarios in crates/tau-test/tests/features/session.feature
- [x] R-session-32: the composite foreign key from `events` to `turn`; scenario in crates/tau-test/tests/features/session.feature
- [x] check: nextest · features · spec --check · steps --check · doc lint · links
- [ ] spec-review (reviewer ticks)
- [x] Scenario: R-session-18 store opens at `<project>/.tau/state.db` (`crates/tau-core/src/project.rs` names the path; the scenario is in crates/tau-test/tests/features/session.feature)
- [ ] at archive: the session row's Leaves cell in spec/index.md names storage/spec.md
