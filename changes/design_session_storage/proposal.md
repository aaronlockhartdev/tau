# Design session storage

## Context

A session is an append-heavy event log that must survive a restart, with lookup state on top of it: anchor rows for the edit tools, observational memory's observations and reflections, and each session's prompt prefix. Two frontends, the desktop app and the ACP adapter, read the same sessions, and a session is resumed, forked, and compacted over days. The store answers queries (list sessions, resume by id, read an event range, look up an anchor) with no server, on the user's disk, and it stays readable when the app dies mid-write.

## Options

- A. SQLite through `rusqlite` 0.42.0, `bundled`, WAL on. Evidence: crates.io, 2026-11-13; sqlite.org/wal.html, read 2026-11-13, states one writer at a time, all processes on one host, and no WAL over a network filesystem; the agent-sessions survey (read 2026-11-13) records OpenCode moving its history from loose files into one SQLite database, and Cursor splitting a session across a JSONL transcript and a SQLite `store.db` so that reading one gives half the picture. Cost: 2 dependencies, a C build for bundled SQLite, one `.db` file per project.
- B. `redb` 4.5.0. Evidence: its README (read 2026-11-13) documents MVCC with concurrent readers and one writer, crash-safety by default, and multi-process access with several writer processes; crates.io, 2026-11-13. Cost: 1 dependency and no C toolchain, and every query is a hand-written index with no standard client able to open the file.
- C. Append-only JSONL per session plus a side index, the Claude Code and Codex shape. Evidence: `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl` and `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, one append-only file per session (agent-sessions survey, read 2026-11-13). Cost: no dependency and a greppable transcript, and a hand-written scan behind every lookup, including the anchor and memory reads.
- D. `sled` 0.34.7. Evidence: crates.io, read 2026-11-13, last release 2024-10-11. Cost: a storage engine with no maintainer.

## Decision

A. SQLite via `rusqlite`, `bundled`, WAL on, in one file at `.tau/sessions.db`, as the only session store: events, anchors, observations, reflections, and prompt prefixes. `fjall` is ruled out by its README (read 2026-11-13): a single database may not be loaded in parallel from separate processes. Tapes stay authored text in `tests/`, and JSONL stays in logs, per `design_user_config`.

Write rule: core owns the write connection and both frontends read through core, so SQLite's one-writer limit holds by construction.

## Consequences

`crates/tau-core/storage` owns the schema and its migrations. The behaviour lane gains a crash case: kill the process mid-write, resume the same session, read the same events. A project on a network filesystem is unsupported, stated in the surface spec. `rusqlite_migration` 2.7.0 requires `rusqlite` ^0.41, so migrations start at 0.41 and move to 0.42 when that crate catches up. Storage adds no wire shape, so `contracts/` is untouched.

Eval: none needed. Storage is deterministic and covered by the behaviour lane.

Open: none.
