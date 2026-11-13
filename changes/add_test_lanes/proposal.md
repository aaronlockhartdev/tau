# Add test lanes

## Context

A suite that is flaky, or slow enough to belong on a nightly job, is a suite an agent will skip, and a skipped suite is no gate. GUI verification through a real window and pixel assertions is the flakiest and slowest kind of test. The suite is also the only evidence a commit has before it lands, so it has to finish inside the time a commit takes.

## Options

- Behaviour as Markdown prose: no step vocabulary to reuse and nothing machine-checkable. Evidence: `unverified`, nothing measured and no external source. Cost: 0 dependencies, and every scenario repeats the vocabulary.
- Behaviour as Gherkin with cucumber-rs (0.23.x) and gherkin (0.16.x): step reuse, tags, and reports of undefined and unused steps, run by `cargo test`. Evidence: [crates.io](https://crates.io/crates/cucumber) and the [cucumber-rs repo](https://github.com/cucumber-rs/cucumber) (read 2026-11-13). Cost: 2 dev-dependencies, a steps target.
- Surface as Playwright over a webview: browser automation against a real window. Evidence: [playwright.dev](https://playwright.dev/docs/intro) requires a browser and a page (read 2026-11-13); no measurement here. Cost: a webview to own, browsers in CI, pixels that flake.
- Surface as `#[gpui::test]` on a GPUI tree, driving components through gpui-kit's `TestWindowExt`: headless, in-process, no window server. Evidence: Zed's [`gpui-test` skill](https://github.com/zed-industries/zed) describes running GPUI tests in process (read 2026-11-13), and [gpui-kit's docs](https://gpui-kit.com/) (read 2026-11-13) render components in headless windows with pointer, keyboard, focus, layout and accessibility assertions. Cost: these tests join `cargo nextest`, 0 extra runners.

Cost of the chosen option, beyond the dependencies: the full suite runs on every commit.

## Decision

Three lanes.

- Behaviour: Gherkin in `crates/tau-test/tests/features/`, run by cucumber-rs (0.23.x), scenarios tagged `@R-…`.
- Surface: `#[gpui::test]` over a headless GPUI tree, driven by gpui-kit's `TestWindowExt`, asserting the event stream, state, focus and layout. No window server, no screenshot diff.
- Eval: real models, run by hand (existing invariant).

No wall-clock budget and no measured gate. Every lane runs on every commit, and a lane that stops fitting that gets fixed in place.

## Consequences

`cargo nextest run` and `cargo test -p tau-test --test features` become the real gates, and the M0 note on them goes away. A slow lane blocks the commit it belongs to, so it gets fixed in place. CI runs a real build, so cache misses and GPUI build cost appear now. Eval gains a cost-per-run number. The proposal adds 32 lines to the corpus and no tokens per turn.

Eval: none needed. The lanes are the gate, and eval keeps its own cost-per-run number.

Open: none.
