# Delta: session/storage

Apply to `spec/session/storage/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-session-01  One write connection
WHILE a session is open THE SYSTEM SHALL hold one write connection to `.tau/state.db` in core.
Acceptance: @R-session-01

### R-session-02  Event and state commit together
WHEN a tool call produces events THE SYSTEM SHALL commit those events and their state changes in one transaction.
Acceptance: @R-session-02

### R-session-03  The log is append-only
WHEN state changes THE SYSTEM SHALL append to the `events` table and write mutable rows only in the state tables.
Acceptance: @R-session-03
Why: a copy of log content in a mutable table needs a reconciliation path, and it runs on the crash day.

### R-session-04  Readers never hold across a turn
WHILE a turn is in flight THE SYSTEM SHALL hold no open read transaction.
Acceptance: @R-session-04
Why: an open read transaction stalls the 1000-page auto-checkpoint and grows the log.

### R-session-05  Synchronous is NORMAL
WHEN the store opens THE SYSTEM SHALL set `synchronous = NORMAL`.
Acceptance: @R-session-05
Why: NORMAL keeps a committed turn durable across an application crash; FULL is not needed for that.

### R-session-06  A committed turn survives restart
WHEN the process restarts after a turn committed THE SYSTEM SHALL return that turn in full.
Acceptance: @R-session-06

### R-session-07  The committed marker is a storage fact
WHEN a turn's transaction commits THE SYSTEM SHALL set that turn's committed marker in the same transaction.
Acceptance: @R-session-07
Why: a renderer that waits on the marker couples the screen to the checkpoint.

### R-session-08  Fork copies a range, records a parent
WHEN a session is forked THE SYSTEM SHALL copy the selected event range into a new session and record the parent session.
Acceptance: @R-session-08

### R-session-09  Compaction appends a summary
WHEN a session is compacted THE SYSTEM SHALL append a summary event and mark the visible range.
Acceptance: @R-session-09

### R-session-10  Search is an index query
WHEN a user searches sessions THE SYSTEM SHALL answer from the FTS5 index over stored events.
Acceptance: @R-session-10

### R-session-11  Export writes JSONL, the store stays
WHEN a session is exported THE SYSTEM SHALL write JSONL and leave the store unchanged.
Acceptance: @R-session-11

### R-session-12  No network filesystem
IF the project lives on a network filesystem THEN THE SYSTEM SHALL refuse to open the store.
Acceptance: @R-session-12
Why: WAL needs shared memory on one host.

### R-session-13  The harness queries the store
WHEN the eval harness reads a run THE SYSTEM SHALL answer from store queries, not a per-tool log format.
Acceptance: @R-session-13
