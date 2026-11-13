# add_realtime_gui_output

## Context

The user watches the agent work as it happens: tokens, tool calls, diffs, memory activation.
A surface that lags the session is not usable, and lag visible only through a flaky pixel test never gets fixed.

## Decision

Core emits one session event stream (assistant deltas, tool lifecycle, plan updates, memory
activation, errors) and every frontend renders from that stream: the GPUI app incrementally, the ACP
adapter onward. No frontend polls, no frontend re-derives state from a transcript file, core emits
no UI concepts (invariant in `spec/index.md`).

Consequences owned by tau: deltas must survive a surface reconnect (replay from the session log);
rendering must not block the event loop at token rate; the observing state from
`adopt_observational_memory` is part of this stream.

Open: incremental markdown strategy for streaming code blocks and tables — re-render, append, or
defer fenced blocks until closed. `[NEEDS CLARIFICATION]`
