# Delta: protocol

Apply to `spec/protocol/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-protocol-01  The stream has one event set

THE SYSTEM SHALL number every event of a provider stream, in the order sent, over block start, delta, block
stop, usage, and terminal.
Acceptance: gate contracts current
Why: adapters speak the generated set, so the vocabulary is checked by a gate, not by prose.

### R-protocol-02  A tool declares its shape

THE SYSTEM SHALL generate each tool's name, description, and input schema into `contracts/`.
Acceptance: @R-protocol-02
Why: a schema no type builds is a claim about the tool, and a provider rejects the claim, not the code.

### R-protocol-03  One vocabulary names the stream

THE SYSTEM SHALL generate the block kind set, the request reason set, and the context slot name set into
`contracts/`.
Acceptance: @R-protocol-03
Why: clauses across several domains name a kind or a reason, and a hand-written list drifts from the type that checks it.
