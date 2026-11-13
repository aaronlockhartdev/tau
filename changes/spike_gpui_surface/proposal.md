# Spike the GPUI surface

## Context

Three decisions wait on measurement: whether `gpui-kit`'s `TextView` meets the streaming contract, what one frame of a streaming transcript costs, and what the app crate costs to build and ship. `add_surface_components` and `add_realtime_gui_output` each name the measurement they need, and both pick a fallback that only a spike can trigger, so the list belongs in one change. The app crate is the only GPUI consumer, so the spike runs there and touches no other member.

## Options

- Spike in `crates/tau-app` against `gpui-kit`, headless, with `gpui-fps` for frame time. Evidence: the `unverified` claims in `add_surface_components` and `add_realtime_gui_output` (2026-11-13) are the questions; [gpui-kit's docs](https://gpui-kit.com/) (read 2026-11-13) show components rendering in headless windows. Cost: a throwaway example target, no dependency beyond the chosen one.
- Build the real surface and measure as it lands. Evidence: none; the named fallback (`gpui-base` only, tau styling it) is chosen on the spike's result, so measuring late means rewriting the transcript view if the result is bad. Cost: a surface rewrite.

## Decision

Spike first, in `crates/tau-app`, with `tasks.md` as the measurement list. Results land in this file, and `add_surface_components`, `add_realtime_gui_output`, and the `AGENTS.md` pin row read the result from here.

Consequences: the surface pin, the fallback choice, and the frame-cost gate name this change as their source, so the two proposals carry no measurement text.

Eval: none needed. The spike measures and the surface lane asserts.

Open: none.
