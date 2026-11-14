# Delta: workspace

Apply to `spec/workspace/spec.md` at archive time.

## ADDED Requirements

### R-workspace-09  Lint gate denies the bug-catching groups

THE SYSTEM SHALL deny `clippy::correctness`, `clippy::suspicious`, `clippy::perf`, `clippy::unwrap_used` and `clippy::expect_used` in `[workspace.lints]`, inherited by every crate, and keep the style and complexity groups at `warn`.
Acceptance: gate lints

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
