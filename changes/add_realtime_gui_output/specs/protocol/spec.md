# Delta: protocol

Apply to `spec/protocol/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-protocol-05  Events carry their time

WHEN core emits an event THE SYSTEM SHALL carry its event time on the stream.

Acceptance: @R-protocol-05
Why: a time the interface shows must ride the stream, not a storage read.
