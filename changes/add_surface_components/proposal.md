# Surface component library

## Context

The desktop app needs a transcript that streams, a diff view, a prompt field with IME and @-mentions, dialogs, approval controls, a theme, and headless tests that assert focus and layout. The stack row named GPUI plus a pinned Zed main rev, which leaves every control to be written by hand against a framework that moves weekly. The choice is how much of that layer to buy.

## Options

- A. `gpui-kit` 0.9.1: one crate re-exporting `gpui-base` (behaviour, focus, accessibility, virtualization, theme tokens) and `gpui-component` (75+ styled components), over [`gpui-pre` 0.3.12](https://crates.io/crates/gpui-pre). Evidence: [crates.io](https://crates.io/crates/gpui-component), 2026-11-13: Apache-2.0, 566k downloads; the project README ([github.com/longbridge/gpui-kit](https://github.com/longbridge/gpui-kit), read 2026-11-13) states the components are "used to build Longbridge Pro from day one" in a shipped commercial desktop app, and lists Markdown and HTML rendering, a `Diff` component, virtual lists, docking, AccessKit accessibility, and `gpui_kit::test` with `TestWindowExt` for headless pointer, keyboard, focus, layout and accessibility assertions. Cost: the app inherits gpui-kit's release cadence (three breaking minor bumps between 2026-09-28 and 2026-11-02), a heavy default feature set, and one company's direction.
- B. `gpui-base` only, with tau owning the presentation layer: unstyled interaction, focus, AccessKit semantics, virtual lists, theme tokens. Evidence: the [`gpui-base` README](https://crates.io/crates/gpui-base) (read 2026-11-13): "Being unstyled is an explicit API contract, not a missing feature", aimed at applications that own their design system. Cost: every control's presentation, and the Markdown, diff, and editor widgets, written in-house on the same dependency weight.
- C. Raw GPUI with in-house controls. Evidence: [crates.io](https://crates.io/crates/gpui), 2026-11-13: `gpui` 0.2.2 published 2025-10-22, eleven months stale, while `gpui-pre` snapshots Zed main weekly (0.3.12 on 2026-11-09). Cost: an eleven-month-old framework, Zed-main churn, and forms, overlays, tables, focus, selection, and IME edge cases written by hand.
- D. Ruled out: vendoring Zed's `crates/ui` and `crates/components`, and the small catalogs [`bezel-ui` 0.2.25](https://crates.io/crates/bezel-ui), [`adabraka-ui` 0.3.9](https://crates.io/crates/adabraka-ui), [`flix` 0.1.25](https://crates.io/crates/flix). Evidence: [`crates/ui/Cargo.toml` on main](https://github.com/zed-industries/zed/blob/main/crates/ui/Cargo.toml) (read 2026-11-13) declares `license = "GPL-3.0-or-later"` and neither crate is on crates.io; the three catalogs carry 3.7k downloads or fewer and list no diff, markdown, editor, or headless test story (crates.io, 2026-11-13). Cost: copyleft inside tau against a Zed-main moving target, and libraries that leave all four to tau.

The component library is confined to `crates/tau-app`, so the core decoupling invariant is untouched and the ACP frontend, which has no UI, gains no dependency.

## Decision

A. `gpui-kit = "0.9"`, depended on by `crates/tau-app` alone, with `gpui-component`'s styled layer. The transcript is `gpui::list` rows of `TextView` fed by `push_str`; file changes render through the `Diff` component; the prompt box uses `Input` and `Textarea`.

B is the named fallback, taken if the M0 spike (`spike_gpui_surface`) shows `TextView` cannot hold a closed block's layout while only the tail re-parses: `gpui-base`'s `TextView` keeps streaming updates and tau styles it.

## Consequences

`AGENTS.md` names `gpui-kit 0.9` and no Zed rev: the library pins `gpui-pre` snapshots of Zed main weekly and absorbs the upstream API breaks in its own release notes, so tau's Zed exposure arrives as a gpui-kit version bump, which is a change. A bump is taken when a needed fix or component lands. Rust 1.92 becomes the workspace MSRV ([gpui-kit's installation page](https://gpui-kit.com/docs/installation), 2026-11-13). The app crate brings its own async runtime beside core's tokio; tree-sitter grammars and `syntect` stay opt-in features, and the `outline` tool of `adopt_hash_anchor_editing` keeps its tree-sitter use in core. Core's own markdown parsing stays `pulldown-cmark`, so the library's parser is a rendering detail. Accessibility arrives through AccessKit in the interaction layer, so tau's first accessibility work is verification with a screen reader on each target platform. The rendering rules of `add_realtime_gui_output` land as `R-SURFACE-NN` in `spec/surface/transcript/spec.md`, and this change's pin and fallback land as `R-SURFACE-NN` in `spec/surface/components/spec.md`. `contracts/` is untouched, since components are a frontend concern. 

Eval: none needed. The spike and the surface lane cover this.

Open: none.
