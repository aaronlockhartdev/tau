# Surface component library

## Context

The desktop app needs a transcript that streams, a diff view, a prompt field with IME and @-mentions, dialogs, approval controls, a theme, and headless tests that assert focus and layout. The stack row named GPUI plus a pinned Zed main rev, which leaves every control to be written by hand against a framework that moves weekly. The choice is how much of that layer to buy.

## Options

- A. `gpui-kit` 0.9.1: one crate re-exporting `gpui-base` (behaviour, focus, accessibility, virtualization, theme tokens) and `gpui-component` (75+ styled components), over `gpui-pre` 0.3.12. Evidence: GitHub API, 2026-11-13: 20,590 stars, 1,256 forks, pushed 2026-11-13, 100+ contributors, top contributor 1,528 of ~2,100 commits; crates.io, 2026-11-13: Apache-2.0, 566k downloads on `gpui-component`; the project README (read 2026-11-13) states the components are "used to build Longbridge Pro from day one" in a shipped commercial desktop app, and lists Markdown and HTML rendering, a `Diff` component, virtual lists, docking, AccessKit accessibility, and `gpui_kit::test` with `TestWindowExt` for headless pointer, keyboard, focus, layout and accessibility assertions. Cost: the app inherits gpui-kit's release cadence (0.7.0 2026-09-28, 0.8.0 2026-10-19, 0.9.0 2026-11-02, each with breaking renames such as `Modal`→`Dialog`), a heavy default feature set, and one company's direction.
- B. `gpui-base` only, with tau owning the presentation layer: unstyled interaction, focus, AccessKit semantics, virtual lists, theme tokens. Evidence: the crate README (read 2026-11-13): "Being unstyled is an explicit API contract, not a missing feature", aimed at applications that own their design system. Cost: every control's presentation, and the Markdown, diff, and editor widgets, written in-house on the same dependency weight.
- C. Raw GPUI with in-house controls. Evidence: crates.io, 2026-11-13: `gpui` 0.2.2 published 2025-10-22, eleven months stale, while `gpui-pre` snapshots Zed main weekly (0.3.12 on 2026-11-09). Cost: an eleven-month-old framework, Zed-main churn, and forms, overlays, tables, focus, selection, and IME edge cases written by hand.
- D. Ruled out: vendoring Zed's `crates/ui` and `crates/components`, and the small catalogs `bezel-ui` 0.2.25, `adabraka-ui` 0.3.9, `flix` 0.1.25. Evidence: `crates/ui/Cargo.toml` on main (read 2026-11-13) declares `license = "GPL-3.0-or-later"` and neither crate is on crates.io; the catalogs' download counts are 3.7k, 3.6k and 2.7k (crates.io, 2026-11-13). Cost: copyleft inside tau against a Zed-main moving target, and libraries with no diff, markdown, editor, or headless test story.

The component library is confined to `crates/tau-app`, so the core decoupling invariant is untouched and the ACP frontend, which has no UI, gains no dependency.

## Decision

A. `gpui-kit = "0.9"`, depended on by `crates/tau-app` alone, with `gpui-component`'s styled layer. The transcript is `gpui::list` rows of `TextView` fed by `push_str`; file changes render through the `Diff` component; the prompt box uses `Input` and `Textarea`; the surface lane drives `TestWindowExt`.

B is the named fallback, taken if the M0 spike shows `TextView` cannot hold a closed block's layout while only the tail re-parses: `gpui-base`'s `TextView` keeps streaming updates and tau styles it. The rendering rules in `add_realtime_gui_output` are the contract either way.

## Consequences

`AGENTS.md` names `gpui-kit 0.9` and no Zed rev: the library pins `gpui-pre` snapshots of Zed main weekly and absorbs the upstream API breaks in its own release notes, so tau's Zed exposure arrives as a gpui-kit version bump, which is a change. A bump is taken when a needed fix or component lands. Rust 1.92 becomes the workspace MSRV (gpui-kit's installation page, 2026-11-13: its Linux platform dependency `oo7` 0.6.0 requires it). The app crate carries `resvg`, `html5ever`, `lsp-types`, `ropey`, `markdown`, `similar`, `notify`, `rust-i18n`, and `smol` as a runtime beside core's tokio; tree-sitter grammars and `syntect` stay opt-in features, and the `outline` tool of `adopt_hash_anchor_editing` keeps its tree-sitter use in core. Core's own markdown parsing stays `pulldown-cmark`, so the library's parser is a rendering detail. Accessibility arrives through AccessKit in the interaction layer, so tau's first accessibility work is verification with a screen reader on each target platform. `gpui-fps` is available for the spike's frame-time gate. `contracts/` is untouched, since components are a frontend concern. The proposal adds 28 lines to the corpus and no tokens per turn.

Eval: none needed. The spike and the surface lane cover this.

Open: none.
