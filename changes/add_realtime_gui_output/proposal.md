# Real-time GUI output

## Context

The user watches the agent work as it happens: tokens, tool calls, diffs, memory activation. A surface that lags the session is not usable, and lag that only shows up under a flaky pixel test never gets fixed.

## Options

- One event stream in core, frontends render it: carried by the invariant that no frontend owns state, so one stream is the only shape that cannot diverge between them. Supporting evidence: Zed drives its editor on GPUI with agent output arriving token by token (Zed repo, read 2026-11-13; `unverified` at the API level, so the M0 spike carries the decision). Cost: the protocol widens to streaming events and `contracts/` regenerates.
- Frontends poll session state: each frontend re-implements reconciliation, and the poll interval is the lag ceiling. Evidence: none measured, none external. Cost: two reconciliation paths to keep equal.

## Decision

One event stream in core carries every state change. The desktop app (GPUI) renders it, and ACP projects it. No frontend polls, and no state lives in a view.

Rendering targets:

- Token deltas append in place; a block re-renders only when its structure changes.
- Tool calls, approvals, and diffs stream as they happen.
- The transcript scrolls live without a layout stall, with a frame budget fixed by a spike.
- The text field, IME, and paste behave under load.

## Consequences

The protocol widens to streaming events, and `contracts/` regenerates. The surface lane asserts stream equivalence in a headless GPUI test, so the gate has no pixels, no window server, and no scheduler.

Open: incremental markdown strategy for streaming code blocks and tables: re-render, append, or defer an open fenced block.
