Feature: Session storage

  # Each scenario ends on the step that reads the store and closes it, so steps that
  # read the store come last.

  @R-session-01
  Scenario: A session's store holds one write connection
    Given an open store
    When the store is opened again
    Then the second open is refused

  @R-session-02 @R-session-28
  Scenario: A tool call's events and state change commit together
    Given an open store
    When a tool call commits its events and its state change
    Then the stored events carry the times they were given
    And the events and the state change are stored together

  @R-session-02 @R-session-30
  Scenario: A refused commit stores no event and no state change
    Given an open store
    When a tool call commits its events and its state change
    # The turn is committed, so the schema refuses its next event, and it is refused
    # after the state row and the log rows of the transaction have been written.
    When a tool call's commit is refused
    Then the refused commit stores nothing

  @R-session-32
  Scenario: An event with no state row is refused
    Given an open store
    # No turn row is written for turn 7, so it is the key that refuses this row.
    When an event for a turn with no state row is inserted
    Then the store refuses the write

  @R-session-30
  Scenario: A committed turn accepts no further event
    Given an open store
    When a tool call commits its events and its state change
    When a late event for the committed turn is inserted
    Then the store refuses the write

  @R-session-31
  Scenario: The log refuses update and delete
    Given an open store
    When a tool call commits its events and its state change
    When a stored event row is updated
    When a stored event row is deleted
    Then the store refuses the write

  @R-session-29
  Scenario: A session records its creation time
    Given an open store
    Then the session records a creation time

  # The child process is killed by a signal: an application crash, which is what
  # `synchronous = NORMAL` promises durability across (R-session-05). A power loss is
  # not claimed.
  @R-session-06 @R-session-07
  Scenario: A turn committed by a process that is killed is returned in full
    Given a store whose turn was committed by a process that is killed
    Then the committed turn is returned in full

  @R-session-02
  Scenario: A process killed with a transaction open leaves the turn empty
    Given a store whose rows were written by a process that is killed
    Then the killed transaction leaves the turn empty

  @R-session-18
  Scenario: A session's store is the project's state file
    Given a git project directory
    When the store opens at the project's state file
    Then the store is the project's state file
    And the project directory is removed

  # The reads are the store's own reader API, in autocommit. The commit before them is
  # load-bearing: `TRUNCATE` reports a lock, and reports frames left, only when the log
  # has frames to copy.
  @R-session-04
  Scenario: Store reads leave the log free to checkpoint
    Given an open store
    When a tool call commits its events and its state change
    When the log is read between turns
    Then the log checkpoints with no reader in the way

  @R-session-03
  Scenario: A state change writes event content to the log only
    Given an open store
    When a tool call commits its events and its state change
    Then the state tables hold no log content

  @R-session-05
  Scenario: The store opens with synchronous NORMAL
    Given an open store
    Then the store's synchronous is NORMAL

  # The fork's copy is uncommitted in the new session: a committed turn refuses the
  # events a fork copies, so a copy that carried the marker would be refused by its own
  # log. The compaction appends; the log never loses an event.
  @R-session-08
  Scenario: A forked session carries the copied range and names its parent
    Given an open store
    When a tool call commits its events and its state change
    And the session is forked at the committed turn
    Then the fork holds the copied events and names its parent

  @R-session-09
  Scenario: A compacted session appends a summary and marks what is visible
    Given an open store
    When a tool call commits its events and its state change
    And the session is compacted at the next turn
    Then the summary is in the log and the visible range starts at its turn

  # The search is answered by the FTS5 index over the stored events, so the query is
  # index syntax: a column name that no scan of the payload text can answer, and a term
  # no event holds, which must answer empty.
  @R-session-10
  Scenario: A search of stored events is answered by the index
    Given an open store
    When a tool call commits its events and its state change
    And the session is forked at the committed turn
    Then the index answers a search of the stored events

  # The lines are the store's rows. The export step reads the store's own change counter
  # on either side of itself, which is the fact "leave the store unchanged" states; the
  # closing step proves the export's read leaves the log free to checkpoint.
  @R-session-11 @R-session-04
  Scenario: Exporting a session writes JSONL and leaves the store alone
    Given an open store
    When a tool call commits its events and its state change
    And the session is exported
    Then the export holds one line per event and the store answers the same
    And the log checkpoints with no reader in the way

  # The eval harness reads a run as a store query: the state table joined to the log, so
  # every turn arrives with its state, its committed marker and its events. The harness
  # binary is M0, so the query is what lands here; the open tasks line says so.
  @R-session-13
  Scenario: A run is answered from store queries
    Given an open store
    When a tool call commits its events and its state change
    And the session is compacted at the next turn
    And a turn that produced no events commits
    Then the run answers every turn of the session with its events

  # The clause's window, opened: the turn's own transaction sits open on a second
  # connection while the store's reader API runs. The closing step's checkpoint is the
  # probe, and it runs after that transaction closes: a writer in flight blocks a
  # checkpoint by itself, which is a different fact from the one this clause states.
  @R-session-04
  Scenario: Store reads inside an open turn leave the log free to checkpoint
    Given an open store
    When a tool call commits its events and its state change
    And the log is read while a turn's transaction is open
    Then the log checkpoints with no reader in the way
