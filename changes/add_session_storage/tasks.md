# Tasks: add_session_storage

- [x] proposal: one SQLite store, separation of duties by schema
- [x] R-session-01..13: delta clauses in changes/add_session_storage/specs/session/storage/spec.md
- [ ] blocked on the planned `scaffold_workspace`: no crates to own the schema yet
- [ ] R-session-01,02: one write connection; event and state in one transaction (crates/tau-core/storage)
- [ ] R-session-03,04,05: append-only log; short readers; `synchronous = NORMAL`
- [ ] R-session-06,07: crash assertion; committed marker in the committing transaction
- [ ] R-session-08,09: fork copies a range with a parent; compaction appends a summary
- [ ] R-session-10,11,13: FTS5 search; export command; harness queries the store
- [ ] R-session-12: refuse a network filesystem
- [ ] R-session-01..13: scenarios in crates/tau-test/tests/features/session.feature
- [ ] check: nextest · features · spec --check · steps --check · doc lint · links
- [ ] spec-review (reviewer ticks)
