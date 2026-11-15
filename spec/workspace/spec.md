# workspace index

Owns: root `Cargo.toml`, `crates/`, `.gitignore`, `contracts/`, `rust-toolchain.toml`, `docs/stack.md` (the decided pins). Leaves: none. Requirements: 7.

## Requirements

### R-workspace-09  Lint gate denies the bug-catching groups

THE SYSTEM SHALL deny `clippy::correctness`, `clippy::suspicious`, `clippy::perf`, `clippy::unwrap_used`, `clippy::expect_used`, `clippy::await_holding_lock` and `clippy::await_holding_invalid_type` in `[workspace.lints]`, inherited by every crate, keep the style and complexity groups at `warn`, and allow `unwrap_used` and `expect_used` in test code, where a panic is a failure report.
Acceptance: gate lints
Why: the two `await_holding_*` lints are the mechanical form of a rule the lanes cannot test.

### R-workspace-10  Lint gate covers every feature and target

WHEN the lint gate runs THE SYSTEM SHALL lint `--workspace --all-targets --all-features`.
Acceptance: gate lints

### R-workspace-11  Advisory gate reads both advisory sources

THE SYSTEM SHALL gate `Cargo.lock` with `cargo deny check` and enable Dependabot for cargo and github-actions, so no advisory source is the only one.
Acceptance: gate dependencies current

### R-workspace-12  CI runs the gate table

WHEN a push or pull request lands THE SYSTEM SHALL run the `ci lane` gate row, whose command runs the command cell of every other gate row in state `run` in `AGENTS.md` and reports rows in state `M0` and `manual` as skipped.
Acceptance: gate ci lane
Why: the table defines the lane; a workflow that lists steps of its own drifts from it.

### R-workspace-13  Test lane reports every failure in CI

WHEN the test lane runs in CI THE SYSTEM SHALL use nextest's `ci` profile, where `fail-fast` is off.
Acceptance: gate unit + surface

### R-workspace-14  Core carries no runtime

THE SYSTEM SHALL keep no async runtime, task-executor or reactor crate in `tau-core`'s dependency graph, at any feature set.
Acceptance: gate core has no runtime
Why: a runtime in core is inherited by every consumer, and the UI is pinned to a framework with a scheduler of its own.

### R-workspace-15  Core spawns no task

THE SYSTEM SHALL start no task, thread, task pool, runtime, reactor, future driver or child process inside `tau-core`.
Acceptance: gate core spawns nothing
Why: the caller owns the executor, so a task or thread started inside core picks a runtime for every consumer.
