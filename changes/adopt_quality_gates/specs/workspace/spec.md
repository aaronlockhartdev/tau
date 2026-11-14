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

### R-workspace-12  CI runs every `run` gate

WHEN a push or pull request lands THE SYSTEM SHALL run every gate row in state `run`, and THE SYSTEM SHALL fail the `ci covers gates` gate when a `run` row's command is absent from the workflow.
Acceptance: gate ci covers gates

### R-workspace-13  Test lane reports every failure in CI

WHEN the test lane runs in CI THE SYSTEM SHALL use nextest's `ci` profile, where `fail-fast` is off.
Acceptance: gate unit + surface
