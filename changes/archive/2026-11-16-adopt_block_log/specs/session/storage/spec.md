# Delta: session/storage

Apply to `spec/session/storage/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-session-35  The block is the log's unit

THE SYSTEM SHALL record one block per commit, or every block a stream closes at once, ordered by a store-wide sequence.
Acceptance: @R-session-35
Why: a block is committed by the transaction that inserts it, so a marker closed after the fact is a second truth about the same row.

### R-session-36  A request records one round-trip

THE SYSTEM SHALL record one `requests` row per provider round-trip, with its provider, model, usage, and terminal reason, and no payload.
Acceptance: @R-session-36
Why: usage is a provider number the log cannot derive, and a request that streamed no block is a fact.

### R-session-37  State is pointers on the block

WHEN the store records a block THE SYSTEM SHALL record the state as pointers into `blocks`, on the `blocks`
row.
Acceptance: @R-session-37
Why: a row of pointers is not payload text, so R-session-03's scan stays green, and the context rebuilds
from the log alone.

### R-session-38  A block names its source

THE SYSTEM SHALL record in each block the request it came from, or none for user input and injections.
Acceptance: @R-session-38

### R-session-39  Resume reads the newest request

WHEN a session resumes THE SYSTEM SHALL answer the newest `requests` row and only the `blocks` rows that follow it.
Acceptance: @R-session-39
Why: a whole-session read materialises every payload, and resume runs on every open, so the narrow read's scope is what keeps its cost down.

### R-session-40  Usage arrives at the terminal

WHEN a request reaches its terminal THE SYSTEM SHALL stamp its usage and reason after the last block
that request produced, leaving an unknown count null.
Acceptance: @R-session-40
Why: one API reports input tokens at the open and another only at the close, so a null is a fact.

### R-session-45  A block records its slot

THE SYSTEM SHALL record in each block the context slot it fills, when it fills one.
Acceptance: @R-session-45
Why: a context rebuilt from the log needs the slot, and the loop's order is not a record.

### R-session-46  A slot's blocks answer

THE SYSTEM SHALL answer a session's blocks that name a given slot.
Acceptance: @R-session-46
Why: a resume rebuilds observations, and `blocks_after` reaches only what follows the newest request.

## MODIFIED Requirements

### R-session-02  Block and state commit together

WHEN a block closes THE SYSTEM SHALL commit that block and its state pointers in one transaction.
Acceptance: @R-session-02
Why: the transaction is the commit, so no caller is left to close a unit last.

### R-session-03  Log rows and mutable rows separate

WHEN a block is recorded THE SYSTEM SHALL append it to the `blocks` table and write mutable rows only in
the `sessions` and `requests` tables.
Acceptance: @R-session-03
Why: a copy of log content in a mutable table needs a reconciliation path, and it runs on the crash day.

### R-session-04  Readers never hold across a commit

WHILE a block commit is in flight THE SYSTEM SHALL hold no open read transaction.
Acceptance: @R-session-04
Why: an open read transaction stalls the auto-checkpoint and grows the log.

### R-session-05  Synchronous is NORMAL

WHEN the store opens THE SYSTEM SHALL set `synchronous = NORMAL`.
Acceptance: @R-session-05
Why: NORMAL keeps a committed block durable across an application crash; FULL is not needed for that.

### R-session-06  A committed block survives restart

WHEN the process restarts after a block committed THE SYSTEM SHALL return that block in full.
Acceptance: @R-session-06

### R-session-08  A fork copies a range

WHEN a session is forked THE SYSTEM SHALL copy the selected block range and the request rows it names into a new session and record the parent session.
Acceptance: @R-session-08
Why: every block names its request, so a copied range without those rows is a broken record.

### R-session-10  Search is an index query

WHEN a user searches sessions THE SYSTEM SHALL answer from the FTS5 index over stored blocks.
Acceptance: @R-session-10
Why: the index is derived from the log and rebuilt from it, so its tables are no place for a fact that needs reconciling.

### R-session-11  Export writes JSONL, the store stays

WHEN a session is exported THE SYSTEM SHALL write one JSONL line per block and leave the store unchanged.
Acceptance: @R-session-11
Why: the export is the log in another form, so a unit the export invents is a second vocabulary.

### R-session-13  The harness queries the store

WHEN the eval harness reads a run THE SYSTEM SHALL answer from store queries over blocks, not a per-tool
log format.
Acceptance: @R-session-13
Why: a per-tool log format would be a second record of the run, and it drifts from the log the store keeps.

### R-session-28  The store keeps the event time

WHEN the store appends a block THE SYSTEM SHALL record the event time the block carried.
Acceptance: @R-session-28
Why: a column default invents a second value for a fact the stream already carries.

### R-session-31  The log refuses update and delete

IF a `blocks` row is updated or deleted THEN THE SYSTEM SHALL refuse the write.
Acceptance: @R-session-31

### R-session-32  A block names a live request

IF a block names a request with no `requests` row, or one whose terminal reason is set, THEN THE SYSTEM SHALL refuse the write.
Acceptance: @R-session-32
Why: an orphan block breaks the export join, and a block after the terminal is a record the request never made.

## REMOVED Requirements

### R-session-07  The committed marker is stored

Acceptance: @R-session-07
Why: a renderer that waits on the marker couples the screen to the checkpoint.
Supersedes: R-session-07 → R-session-02 in `spec/session/storage/spec.md`.

### R-session-30  A committed turn is closed

Acceptance: @R-session-30
Why: a committed turn is the record readers answer from, and a late event would rewrite it.
Supersedes: R-session-30 → R-session-32 in `spec/session/storage/spec.md`.
