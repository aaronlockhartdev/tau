# Delta: workspace

## ADDED Requirements

### R-workspace-14  Core carries no runtime

THE SYSTEM SHALL keep no async runtime, task-executor or reactor crate in `tau-core`'s dependency graph, at any feature set.
Acceptance: gate core has no runtime
Why: a runtime in core is inherited by every consumer, and the UI is pinned to a framework with a scheduler of its own.

### R-workspace-15  Core spawns no task

THE SYSTEM SHALL start no task, thread, task pool, runtime, reactor, future driver or child process inside `tau-core`.
Acceptance: gate core spawns nothing
Why: the caller owns the executor, so a task or thread started inside core picks a runtime for every consumer.

## MODIFIED Requirements

### R-workspace-09  Lint gate denies the bug-catching groups

THE SYSTEM SHALL deny `clippy::correctness`, `clippy::suspicious`, `clippy::perf`, `clippy::unwrap_used`, `clippy::expect_used`, `clippy::await_holding_lock` and `clippy::await_holding_invalid_type` in `[workspace.lints]`, inherited by every crate, keep the style and complexity groups at `warn`, and allow `unwrap_used` and `expect_used` in test code, where a panic is a failure report.
Acceptance: gate lints
Why: the two `await_holding_*` lints are the mechanical form of a rule the lanes cannot test.
