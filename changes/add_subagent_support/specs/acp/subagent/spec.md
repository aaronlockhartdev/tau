# Delta: acp/subagent

## ADDED

### R-acp-01  A child is a session on the wire

WHEN a child session starts THE SYSTEM SHALL announce it with a `subagent_update` upsert on the parent's stream, carrying the child's session id.

Acceptance: @R-acp-01

### R-acp-02  Child events reach the client unprompted

WHILE a child runs THE SYSTEM SHALL deliver the child's events to the connected client without a per-child request.

Acceptance: @R-acp-02

### R-acp-03  Child state uses the RFD's states

THE SYSTEM SHALL report a child as `running`, `requires_action`, `unknown`, or `idle` with a stop reason.

Acceptance: @R-acp-03

### R-acp-04  A child's approval request reaches the client

WHEN a child requests a permission THE SYSTEM SHALL surface the request to the client attributed to the child session.

Acceptance: @R-acp-04

### R-acp-05  A client can steer a child

WHEN a client sends a prompt scoped to a child session THE SYSTEM SHALL queue it to the child's next turn boundary.

Acceptance: @R-acp-05
Why: the draft forbids client-initiated prompting, so tau ships the RFD's named follow-up for its own app.

### R-acp-06  Wire types are tau's, in one crate

THE SYSTEM SHALL define the subagent wire types in `crates/tau-protocol`.

Acceptance: gate contracts current
