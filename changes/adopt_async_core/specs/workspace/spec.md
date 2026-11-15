# Delta: workspace

## ADDED Requirements

### R-workspace-14  Core carries no runtime

THE SYSTEM SHALL keep no async runtime crate in `tau-core`'s dependency graph, at any feature set.
Acceptance: gate core has no runtime
Why: a runtime in core is inherited by every consumer of the library, and the UI is pinned to a framework with a scheduler of its own.

### R-workspace-15  Core spawns no task

THE SYSTEM SHALL spawn no task and drive no runtime inside `tau-core`: core returns futures and streams, and the binary that calls it spawns them.
Acceptance: gate core spawns nothing
Why: the caller owns the executor, so a spawn inside core picks a runtime for every consumer of the library.

## MODIFIED Requirements

### R-workspace-09  Lint gate denies the bug-catching groups

THE SYSTEM SHALL deny `clippy::correctness`, `clippy::suspicious`, `clippy::perf`, `clippy::unwrap_used`, `clippy::expect_used`, `clippy::await_holding_lock` and `clippy::await_holding_invalid_type` in `[workspace.lints]`, inherited by every crate, and keep the style and complexity groups at `warn`.
Acceptance: gate lints
Why: the two `await_holding_*` lints are the mechanical form of a rule the lanes cannot test.
