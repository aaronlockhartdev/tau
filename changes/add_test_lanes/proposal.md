# Add test lanes

## Context

A suite that is flaky, or slow enough to belong on a nightly job, is a suite an agent will skip, and a skipped suite is no gate. GUI verification through a real window and pixel assertions is the flakiest and slowest kind of test. Speed and determinism are requirements, not aspirations.

## Options

- Behaviour as Markdown prose: no step vocabulary to reuse and nothing machine-checkable. Evidence: none measured, none external. Cost: 0 dependencies, and every scenario repeats the vocabulary.
- Behaviour as Gherkin with cucumber-rs (0.23.x) and gherkin (0.16.x): step reuse, tags, and reports of undefined and unused steps, run by `cargo test`. Evidence: crates.io and the cucumber-rs book (read 2026-11-13). Cost: 2 dev-dependencies, a steps target.
- Surface as Playwright over a webview: browser automation against a real window. Evidence: playwright.dev requires a browser and a page (read 2026-11-13); no measurement here. Cost: a webview to own, browsers in CI, pixels that flake.
- Surface as `#[gpui::test]` on a GPUI tree: headless, in-process, no window server. Evidence: Zed's `gpui-test` skill describes running GPUI tests in process (Zed repo, read 2026-11-13). Cost: these tests join `cargo nextest`, 0 extra runners.

Cost of the chosen option, beyond the dependencies: the full suite runs on every commit, and M0 measures its wall clock.

## Decision

Three lanes.

- Behaviour: Gherkin in `tests/features/`, run by cucumber-rs (0.23.x), scenarios tagged `@R-…`.
- Surface: `#[gpui::test]` over a headless GPUI tree, asserting the event stream and state. No window server, no screenshot diff.
- Eval: real models, run by hand (existing invariant).

M0 measures the lanes and sets the wall-clock gates from those measurements.

## Consequences

`cargo nextest run` and `cargo test --test features` become the real gates, and the M0 note on them goes away. A slow lane fails CI by wall clock, so it gets fixed rather than moved to a nightly job. CI runs a real build, so cache misses and GPUI build cost appear now. Eval gains a cost-per-run number.

Open: the wall-clock budgets, once measured.
