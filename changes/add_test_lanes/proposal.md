# add_test_lanes

## Context

A suite that is flaky, or slow enough to belong on a scheduler (15 minutes, worst in GUI
verification), is a suite an agent will skip — and a skipped suite is no gate at all.

## Decision

Three lanes, with the wall clock as a first-class requirement.

| Lane | Tool | Determinism source | Runs |
|---|---|---|---|
| Behaviour (core, protocol, tools, session) | `cucumber` + mock-LLM tape | recorded tape, no network | every commit |
| Surface (GPUI) | `#[gpui::test]` + `TestAppContext` | GPUI's test dispatcher: fake time, seeded scheduler interleavings, `SEED`/`ITERATIONS` reproduction | every commit |
| Eval (real LLM) | `tau-test` eval binary | none — stochastic | by hand, when deciding if a feature is worth building |

Mechanics and caveats come from the references, not this file: the cucumber-rs book
(`harness = false`, so the target runs outside nextest) and Zed's `gpui-test` skill (seeds,
iterations, retries, and the warning about async runtimes outside the test dispatcher — the reason
the two lanes never mix).

Consequences owned by tau: no network, no real LLM, no browser or screenshot automation in CI; GUI
verification is in-process state assertions, not pixels; every flake reproduces from a printed seed;
spikes measure the gates before they are written into the spec.

Proposed gates, unmeasured: full CI <5 min, behaviour <3 min, surface <2 min.
`[NEEDS CLARIFICATION]` — confirm after the M0 spike measures actual wall clock.
