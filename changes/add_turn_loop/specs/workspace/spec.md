# Delta: workspace

Apply to `spec/workspace/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-workspace-16  Libraries emit, binaries sink

THE SYSTEM SHALL keep library logs on the `tracing` facade, and install a log subscriber only in a binary.
Acceptance: gate core installs no subscriber
Why: a subscriber's non-blocking writer starts a thread, and R-workspace-15 bans core starting one.

### R-workspace-17  Logs land on stderr

THE SYSTEM SHALL write log output to stderr, at the default level `info`.
Acceptance: @R-workspace-17
Why: a surface owns its own pane, and a log on stdout steals a line from the reader.
