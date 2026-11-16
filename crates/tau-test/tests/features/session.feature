Feature: Session storage

  # Each scenario ends on the step that reads the store and closes it, so steps that
  # read the store come last.

  @R-session-01
  Scenario: A session's store holds one write connection
    Given an open store
    When the store is opened again
    Then the second open is refused

  # A block is committed by the transaction that inserts it, so the log carries no
  # marker: the rows land, the sequence advances, and the request row is opened with
  # its model and nothing else.
  @R-session-35 @R-session-36 @R-session-28
  Scenario: A tool call's blocks commit on the store's sequence
    Given an open store
    When a request is opened and a tool call commits its blocks
    Then the blocks are stored in the order they were committed
    And each block carries the time it was given
    And the request row carries no payload column
    And the request row records its model with no usage and no reason

  # One block per commit, and one transaction for the batch a stream closes at once:
  # the batch's rows land together, on sequences with nothing between them.
  @R-session-35 @R-session-02
  Scenario: A batch a stream closes at once commits in one transaction
    Given an open store
    When a request is opened and a tool call commits its blocks
    And the blocks a stream closed at once are committed
    Then the batch lands as one commit, on adjacent sequences

  # The transaction is the unit that fails: the refusal lands after the first row of
  # the batch is written, and a refused batch leaves no row of its own.
  @R-session-02
  Scenario: A refused batch stores no block
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request is stamped terminal
    And a batch naming a terminal request is committed
    Then the refused batch stores no block

  @R-session-30
  Scenario: A request past its terminal accepts no further block
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request is stamped terminal
    And a block naming that request is inserted
    Then the store refuses the write

  # The key binds the store's own writes, and a block naming a request with no row is
  # the row the key refuses.
  @R-session-32
  Scenario: A block naming a request with no row is refused
    Given an open store
    # The setting belongs to a connection, so the store's own answer is the one that
    # binds its writes: a reader connection sets the pragma for itself.
    And the store's own connection enforces foreign keys
    # No request row is written for request 7, so it is the key that refuses this row.
    When a block for a request with no row is inserted
    Then the store refuses the write

  @R-session-33
  Scenario: A second instance meets a store mid-setup
    # The locking connection is a writer, so the setup the child attempts is locked.
    When a second instance opens the store while its setup is locked
    Then the store on disk is in WAL mode

  @R-session-33
  Scenario: A second instance meets a store mid-migration
    # The locking connection puts the file in WAL first, so the store's own upgrade
    # succeeds and the lock falls on the migrations, whose refusal has the other shape.
    When a second instance opens the store while its migrations are locked
    Then the store on disk is in WAL mode

  @R-session-34
  Scenario: A setup lock that outlasts the budget is refused
    # The retry is bounded, so an instance meeting a setup that will not clear is refused
    # with a busy store rather than parked. The lock stands on a file not yet in WAL, so the
    # leg attempted is the journal upgrade, where a standing lock is refused on the spot: the
    # budget is ~1 s of backoff, well inside the step's 5 s beat. The migration leg, where
    # each attempt burns a 100 ms busy timeout, is the scenario above.
    When the setup lock outlasts the store's retry budget
    Then the second instance is refused with a busy store

  @R-session-31
  Scenario: The log refuses update and delete
    Given an open store
    When a request is opened and a tool call commits its blocks
    When a stored block row is updated
    When a stored block row is deleted
    Then the store refuses the write

  @R-session-29
  Scenario: A session records its creation time
    Given an open store
    Then the session records a creation time

  # The child process is killed by a signal: an application crash, which is what
  # `synchronous = NORMAL` promises durability across (R-session-05). A power loss is
  # not claimed. The marker read is now an absence: a block is committed by the
  # transaction that inserts it, so the log carries no committed column at all.
  @R-session-06 @R-session-07
  Scenario: A block committed by a process that is killed is returned in full
    Given a store whose block was committed by a process that is killed
    Then the committed block is returned in full
    And the log carries no committed marker

  @R-session-02
  Scenario: A process killed with a transaction open leaves no block
    Given a store whose rows were written by a process that is killed
    Then the killed transaction leaves no block

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
    When a request is opened and a tool call commits its blocks
    When the log is read between commits
    Then the log checkpoints with no reader in the way

  @R-session-03
  Scenario: A block write keeps log content out of the mutable tables
    Given an open store
    When a request is opened and a tool call commits its blocks
    Then the state tables hold no log content

  @R-session-05
  Scenario: The store opens with synchronous NORMAL
    Given an open store
    Then the store's synchronous is NORMAL

  # A copied range without the request rows its blocks name is a broken record, so the
  # fork copies both, and the copied request keeps its terminal in the child.
  @R-session-08
  Scenario: A forked session carries the copied blocks, their requests, and its parent
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request is stamped terminal
    And the session is forked over its blocks
    Then the fork holds the copied blocks, their request, and names its parent

  # The search is answered by the FTS5 index over the stored blocks, so the query is
  # index syntax: a column name that no scan of the payload text can answer, and a term
  # no block holds, which must answer empty.
  @R-session-10
  Scenario: A search of stored blocks is answered by the index
    Given an open store
    When a request is opened and a tool call commits its blocks
    And the session is forked over its blocks
    Then the index answers a search of the stored blocks

  # The lines are the store's rows. The export step reads the store's own change counter
  # on either side of itself, which is the fact "leave the store unchanged" states; the
  # closing step proves the export's read leaves the log free to checkpoint.
  @R-session-11 @R-session-04
  Scenario: Exporting a session writes JSONL and leaves the store alone
    Given an open store
    When a request is opened and a tool call commits its blocks
    And the session is exported
    Then the export holds one line per block and the store answers the same
    And the log checkpoints with no reader in the way

  # The eval harness reads a run as a store query over blocks, in the store's sequence,
  # with no state of its own and no per-tool log format. A request that streamed no block
  # is a row of `requests`, not a unit the read invents.
  @R-session-13
  Scenario: A run is answered from store queries over blocks
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request that streamed no block is opened and stamped
    Then the run answers the session's blocks in sequence

  # The clause's window, opened: the block's own transaction sits open on a second
  # connection while the store's reader API runs. The closing step's checkpoint is the
  # probe, and it runs after that transaction closes: a writer in flight blocks a
  # checkpoint by itself, which is a different fact from the one this clause states.
  @R-session-04
  Scenario: Store reads inside an open block commit leave the log free to checkpoint
    Given an open store
    When a request is opened and a tool call commits its blocks
    And the log is read while a block commit is in flight
    Then the log checkpoints with no reader in the way

  # A row of pointers is not payload text, which is what keeps the scan green, and the
  # context rebuilds from the log alone.
  @R-session-37
  Scenario: A block records its state as pointers into the log
    Given an open store
    When a block carrying state pointers is committed
    Then the state answers as pointers into the log

  # User input and injections arrive from no round-trip, so they name no request; the
  # blocks a request streamed name it.
  @R-session-38
  Scenario: A block names the request it came from, or none
    Given an open store
    When a user input block and an injection block are committed
    And a request is opened and a tool call commits its blocks
    Then the blocks answer their requests, and the two name none

  # Resume runs on every open, and the whole-session read materialises every payload, so
  # the narrow reads are the ones a resume answers from.
  @R-session-39
  Scenario: A resume reads the newest request and the blocks after it
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request is stamped terminal
    And a second request is opened, commits a block and is stamped terminal
    And a steering block is committed
    Then the resume reads answer the newest request and the blocks after it
    And the whole-session read answers every block again

  # One API reports input tokens at the open and another only at the close, so a null is
  # a fact the store keeps, and a request that streamed no block is a row of its own.
  @R-session-40
  Scenario: Usage and reason arrive at the terminal
    Given an open store
    When a request that streamed no block is opened and stamped
    Then the newest request answers its counts and its reason
    And a count the provider never reported stays null

  # One API reports cache counts and another does not; four counts of distinct size prove each
  # value lands in the column it names, which no read of one column can show.
  @R-session-40
  Scenario: A provider's four counts land in their own columns
    Given an open store
    When a request whose provider reported every count is opened and stamped
    Then each usage count answers in its own column

  # A context rebuilt from the log needs the slot, and the loop's order is not a record.
  @R-session-45
  Scenario: An injection block records the slot it fills
    Given an open store
    When a user input block and an injection block are committed
    Then the injection block names the slot it fills

  # A resume rebuilds observations, and `blocks_after` reaches only what follows the
  # newest request.
  @R-session-46
  Scenario: A slot's blocks answer
    Given an open store
    When a user input block and an injection block are committed
    Then the slot answers the blocks that name it

  # The stamp is one write per request: R-session-40's terminal lands once, after the last
  # block. A second stamp would rewrite the counts the provider reported, so the store
  # refuses it, and a process killed between the two writes leaves the blocks with a request
  # that is still open.
  @R-session-40
  Scenario: A second terminal for one request is refused
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request is stamped terminal
    And a second terminal is stamped for that request
    Then the first terminal answers its counts and its reason

  @R-session-40
  Scenario: A process killed after its blocks and before its terminal leaves an open request
    Given a store whose blocks committed and whose terminal was not stamped
    Then the blocks answer and the request carries no reason

  # The keys bind every row a commit writes: the session a block names is recorded, and a
  # state is a pair of pointers, so half a pair is not a state.
  @R-session-32
  Scenario: A block naming a session that is not recorded is refused
    Given an open store
    When a block naming a session that is not recorded is inserted
    Then the store refuses the write

  @R-session-37
  Scenario: A block carrying half a state is refused
    Given an open store
    When a block carrying half a state is inserted
    Then the store refuses the write

  # The child's copy of a state pair names the child's rows. A range that starts past a
  # pointer has no row to name, so the child carries no state.
  @R-session-37
  Scenario: A fork re-points the state it copies and drops the state it cannot
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a block is committed with a state naming the first block
    And the session is forked over its blocks
    Then the child's state names the child's copy of that block
    When the session is forked from its second block
    Then the second child carries no state

  # A forked session continues on a request of its own: the copied request keeps its terminal,
  # so the child opens a new one and appends without touching the copy.
  @R-session-08
  Scenario: A forked session appends without duplicating a copied block
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a request is stamped terminal
    And the session is forked over its blocks
    And the child opens a request and commits a block
    Then the child holds the copied blocks and its own

  # A state is a pair of pointers into the session's own blocks: it runs forwards, and each
  # pointer names a block the session has. A pair naming nothing is a state no read answers.
  @R-session-37
  Scenario: A block whose state runs backwards is refused
    Given an open store
    When a block whose state runs backwards is inserted
    Then the store refuses the write

  @R-session-37
  Scenario: A block whose state names no block is refused
    Given an open store
    When a block whose state names no block is inserted
    Then the store refuses the write

  # A pair is stored whole, in order, and naming blocks the session has already written: the
  # state a read can rebuild. Four arms, four checks: the pair's wholeness (the two half-pair
  # scenarios), its direction (the scenario above), a pointer naming nothing at its end (the
  # scenario below), and a start naming nothing while the end names a row, which needs a gap in
  # the session's sequences and so lives in tau-core's `a_state_pointer_whose_start_names_nothing_is_refused`.
  @R-session-37
  Scenario: A block whose state ends on no block is refused
    Given an open store
    When a request is opened and a tool call commits its blocks
    And a block whose state ends on no block is inserted
    Then the store refuses the write

  @R-session-37
  Scenario: A block whose state has an end and no beginning is refused
    Given an open store
    When a block whose state has an end and no beginning is inserted
    Then the store refuses the write

  # A session's state is the pair its newest state-carrying block left: the context a request
  # is built from is the one the last round-trip closed at.
  @R-session-37
  Scenario: A session's state is the newest pair, not the oldest
    Given an open store
    When a block is committed with a state naming the first block
    And a block is committed with a state naming the second block
    Then the session's state names the pair the second block left
