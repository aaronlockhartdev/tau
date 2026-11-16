# Delta: workspace

Apply to `spec/workspace/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-workspace-16  Binaries own the log sink

THE SYSTEM SHALL install a log subscriber only in a binary, never in a library.
Acceptance: gate core installs no subscriber
Why: a subscriber's non-blocking writer starts a thread, and core starts no thread.

### R-workspace-17  Logs land on stderr

THE SYSTEM SHALL write log output to stderr, at the default level `info`.
Acceptance: @R-workspace-17
Why: a surface owns its own pane, and a log on stdout lands where its reader is writing.

### R-workspace-18  Core speaks no HTTP

THE SYSTEM SHALL keep the client that speaks HTTP to a model provider out of `tau-core` and out of its dependency graph.
Acceptance: gate core has no HTTP client
Why: the stream is the boundary, and a provider SDK in core would carry a runtime in with it.

### R-workspace-19  Libraries emit, never sink

THE SYSTEM SHALL emit log records through the `tracing` facade from a library.
Acceptance: @R-workspace-19
Why: a library that writes to a stream steals a line from the reader, and only the binary knows where output goes.
