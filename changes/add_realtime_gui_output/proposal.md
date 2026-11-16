# Real-time GUI output

## Context

The user watches the agent work as it happens: tokens, tool calls, diffs, memory activation. A surface that lags the session is not usable, and lag that only shows up under a flaky pixel test never gets fixed. The transcript is the app's main surface, so its frame cost is the app's frame cost.

## Options

- One event stream in core, frontends render it: carried by the invariant that no frontend owns state, so one stream is the only shape that cannot diverge between them. Supporting evidence: Zed drives its editor on GPUI with agent output arriving token by token ([Zed repo](https://github.com/zed-industries/zed), read 2026-11-13; `unverified` at the API level, so the M0 spike carries the decision). Cost: the protocol widens to streaming events and `contracts/` regenerates.
- Frontends poll session state: each frontend re-implements reconciliation, and the poll interval is the lag ceiling. Evidence: none measured, none external. Cost: two reconciliation paths to keep equal.

## Decision

One event stream in core carries every state change. The desktop app renders it through `gpui-kit` (`add_surface_components`), and ACP projects it. No frontend polls, and no state lives in a view.

Rendering rules:

- Markdown renders as a block tree. A closed block is immutable: its layout is retained and never re-parsed.
- The block tree is `gpui-kit`'s `TextView` inside a `gpui::list`; the rules here are the contract it must meet, with `gpui-base`'s `TextView` as the fallback.
- Only the tail block is re-parsed per token delta.
- An open block shows as monospace text and renders fully once it closes; a table lays out as a grid at close.
- ATX headings only, so no line restyles a block above it.
- Tool calls, approvals, and diffs stream as they happen.
- The M0 spike measures this: `changes/spike_gpui_surface/tasks.md`.
- The text field, IME, and paste behave under load.

## Consequences

so a block that never committed rolls back with its transaction

R-surface-09's scenario asserts stream equivalence. Eval: none needed — the frame cost is a gate row and
the stream is a lane scenario.
R-surface-18 holds the surface to. Eval: none needed — the frame cost is a gate row and the stream is a
lane scenario.

Open: the frame budget R-surface-18 holds the surface to is the number `spike_gpui_surface` records, and
is fixed by that spike's measurement, not by this change.
