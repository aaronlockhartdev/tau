# Delta: session/child

## ADDED

### R-session-14  A child records its parent

WHEN a child session is created THE SYSTEM SHALL record the parent session id on the child's row.

Acceptance: @R-session-14

### R-session-15  A directed message is an event

WHEN one session messages another THE SYSTEM SHALL append the message to each participant's event log.

Acceptance: @R-session-15

### R-session-16  Cancelling a child cascades

WHEN a child session is cancelled THE SYSTEM SHALL cancel the work it runs and the descendants that work waits on.

Acceptance: @R-session-16

### R-session-17  Closing a parent cancels its descendants

WHEN a parent session closes THE SYSTEM SHALL cancel every descendant.

Acceptance: @R-session-17
