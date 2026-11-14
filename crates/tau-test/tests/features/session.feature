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
