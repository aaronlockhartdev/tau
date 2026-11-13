# Delta: harness/lanes

## ADDED

### R-harness-04  The behaviour lane runs the feature tree

WHEN the behaviour lane runs THE SYSTEM SHALL run every scenario in `crates/tau-test/tests/features/` through cucumber, each tagged with a requirement ID.

Acceptance: gate behaviour

### R-harness-05  The behaviour lane drives a tape

WHEN the behaviour lane runs THE SYSTEM SHALL answer model calls from the tape in `crates/tau-test`, with no network call.

Acceptance: gate behaviour

### R-harness-06  The surface lane runs headless

WHEN the surface lane runs THE SYSTEM SHALL drive components through `TestWindowExt` in a headless window, with no window server and no screenshot comparison.

Acceptance: gate unit + surface

### R-harness-07  Automated gates run on every commit

WHEN a commit is prepared THE SYSTEM SHALL run every automated gate in `AGENTS.md`, and report a manual gate as skipped.

Acceptance: spec-review check 15

### R-harness-08  Every lane stays in the commit

WHEN a lane stops fitting the time a commit takes THE SYSTEM SHALL keep running it on every commit, and SHALL NOT add a nightly job or a skip.

Acceptance: spec-review check 15
Why: a suite an agent skips is not a gate, and a nightly job is a skip with a timer.

### R-harness-09  CI builds the app crate

WHEN CI runs THE SYSTEM SHALL build `crates/tau-app`.

Acceptance: gate unit + surface
Why: a GPUI build skipped in CI surfaces as a broken desktop gate on someone else's commit.
