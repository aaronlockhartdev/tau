# Delta: protocol

Apply to `spec/protocol/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-protocol-01  The stream has one event set

THE SYSTEM SHALL express a provider stream as a numbered sequence of block start, delta, block stop, usage, and terminal.
Acceptance: gate contracts current
Acceptance: @R-agent-01
Why: adapters speak the generated set, so the vocabulary is checked by a gate, not by prose.

### R-protocol-02  A tool declares its shape

THE SYSTEM SHALL generate each tool's name, description, and input schema into `contracts/`.
Acceptance: gate contracts current
Acceptance: @R-protocol-02

### R-protocol-03  Tools run through the injected registry

THE SYSTEM SHALL run a tool that starts a process in a crate other than `tau-core`, from the registry the binary injects.
Acceptance: gate core spawns nothing
Acceptance: @R-agent-04
Why: the first real tool spawns a process, and R-workspace-15 bans core starting one.

### R-protocol-04  Core speaks no HTTP

THE SYSTEM SHALL keep the client that speaks HTTP to a model provider in a leaf crate outside `tau-core`, and out of `tau-core`'s dependency graph.
Acceptance: gate core has no HTTP client
Why: the stream is the boundary, and a provider SDK in core would carry a runtime in with it.

### R-protocol-06  Kinds and endings share one vocabulary

THE SYSTEM SHALL generate the block kind set and the request reason set into `contracts/`.
Acceptance: gate contracts current
Acceptance: @R-protocol-06
Why: clauses across three domains name a kind or a reason, and a hand-written list drifts from the type that checks it.

### R-protocol-07  The tool trait is core's

THE SYSTEM SHALL declare the trait that runs a tool in `tau-core`.
Acceptance: @R-agent-04
Why: core consumes tools, so core names what it consumes, and no tool runs inside core.
