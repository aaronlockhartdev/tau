# Design session storage

## Context

A session is an append-heavy log that must survive a restart, with lookup state on top: anchor rows for the edit tools, observational memory's observations and reflections, per-session prompt prefixes, approvals. Sessions get listed, resumed, forked, and compacted over days, by two frontends. Harnesses split on this question. Claude Code, Codex, Copilot, and pi keep the transcript in append-only files; OpenCode keeps it in a SQLite database; Cursor and Hermes split one session across a file and a database, and the survey of agent storage describes Cursor so that reading one store gives half the picture.

## Options

- A. Everything in SQLite (`rusqlite` 0.42.0, `bundled`, WAL). Evidence: crates.io, 2026-11-13; `libsqlite3-sys` build.rs on master (read 2026-11-13) enables `SQLITE_ENABLE_FTS5`, `JSON1`, `RTREE`, `STAT4`; sqlite.org/wal.html (read 2026-11-13) states one writer at a time, all processes on one host, and no WAL over a network filesystem. Cost: one authority and one file, with fork, compaction, and export as row copies, and `sqlite3` needed to read a session at all.
- B. Transcript in append-only JSONL, harness state in SQLite. Evidence: pi on this machine stores one file per session at `~/.pi/agent/sessions/--<cwd>--/<ts>_<uuid>.jsonl` whose first record carries `parentSession` as a path (inspected 2026-11-13), alongside Claude Code's `~/.claude/projects/<encoded-cwd>/<id>.jsonl`, Codex's `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, and Copilot's `~/.copilot/session-state/<id>/events.jsonl` (agent-sessions survey, read 2026-11-13). Cost: two files, a reconcile pass at open, and an explicit rule that the database is derived, which is the part the Cursor and Hermes splits leave unstated.
- C. Transcript in JSONL, state in `redb` 4.5.0. Evidence: its README (read 2026-11-13) documents MVCC with concurrent readers and one writer, crash-safety by default, and multi-process access with several writers; crates.io, 2026-11-13. Cost: no C toolchain, and no query language, no full-text index, and no client that opens the file.
- D. JSONL with a side index, no database. Evidence: as B. Cost: no dependency, and a hand-written scan behind every lookup (session list, anchor, memory read).

Performance does not choose between these. The redb benchmark table (README, read 2026-11-13, Ryzen 9950X3D with NVMe) puts 1M individual writes at 920 ms for redb and 7040 ms for SQLite, roughly 1087 and 142 transactions per second with one fsync per commit; SQLite in WAL with `synchronous = NORMAL` batches fsync into thousands per second, which is where it runs. tau writes tens of events per second, so every option clears the workload with room to spare and the model is the bottleneck.

## Decision

B. The transcript is `.tau/sessions/<ts>_<uuid>.jsonl`, one file per session, the source of truth, first record carrying the session id, cwd, and a `parentSession` path. Harness state is `.tau/state.db`, SQLite (`rusqlite`, `bundled`, WAL): session index rows, anchors, observations, reflections, approvals, and an FTS5 index over transcript text.

Authority rule: the transcript is authoritative and the database is derived. Opening a session reconciles the file past its last indexed offset and rebuilds derived rows, so a crash leaves no event unreachable.

Live rule: every write goes through core, which appends the transcript line and publishes the event on the core stream (`add_realtime_gui_output`), so a frontend that is already open receives new events and never reads the file itself. The reconcile pass runs at open, and a second writer process is out of scope: core owns the only SQLite write connection.

## Consequences

`crates/tau-core/storage` owns the transcript codec and the schema. The behaviour lane gains a crash case: kill mid-write, reopen, reconcile, read the same events. Fork is a new file plus a `parentSession` path, and compaction appends a summary record, so neither copies rows. Tapes stay authored text in `tests/`, and JSONL stays in logs, per `design_user_config`. A project on a network filesystem is unsupported, since WAL needs shared memory on one host. `rusqlite_migration` 2.7.0 requires `rusqlite` ^0.41, so migrations start at 0.41. Storage adds no wire shape, so `contracts/` is untouched.

Eval: none needed. Storage is deterministic and covered by the behaviour lane.

Open: none.
