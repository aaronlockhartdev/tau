# Delta: tools

Apply to `spec/tools/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-tools-30  Tools run through the injected registry

THE SYSTEM SHALL run a tool that starts a process in a crate other than `tau-core`, from the registry the binary injects.
Acceptance: @R-tools-30
Why: the first real tool spawns a process, and R-workspace-15 bans core starting one.

### R-tools-31  The tool trait is core's

THE SYSTEM SHALL declare the trait that runs a tool in `tau-core`.
Acceptance: @R-tools-31
Why: core consumes tools, so core names what it consumes, and no tool runs inside core.
