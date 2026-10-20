# Research: server-side TPS from the responses-API stream (ticket #224)

Researched 2026-10-20. The goal: make the GUI's decode-rate stat reflect the
model's true tokens-per-second instead of a wall-clock figure that counts
prefill and network time. The ticket's triage proposed a server-side signal:
the two `created_at` fields on the `response.created` and `response.completed`
SSE frames. **A live capture refutes that signal** — see the negative finding
below. The fix that actually ships is a client-side one: measure the
wall-clock window from the *first* assistant token, which excludes prefill.

## Method

One live capture of a streamed turn against the no-key dev endpoint
`https://llms.aaronlockhart.dev/v1/responses` (vLLM, model `qwen3.8-27b`),
`stream: true`, `stream_options.include_usage: true`. The raw stream is
`tau-224-sse-capture.txt` at the task's working tree. The turn produced a
reasoning item (30 tokens) and a one-word message (4 tokens): 34 output
tokens total.

## What the stream actually carries

Frame sequence (with the response's `sequence_number`):

| seq | type | carries `created_at` |
|----:|------|:----|
| 0 | `response.created` | yes — `1792517933` |
| 1 | `response.in_progress` | yes — `1792517933` |
| 2–12 | reasoning item add/delta/done | no |
| 13–18 | message item add/delta/done | no |
| 19 | `response.completed` | yes — `1792517933` |

The decisive fact: **there is exactly one distinct `created_at` value in the
whole stream.** `response.created`, `response.in_progress`, and
`response.completed` are all stamped with the same second. In the Responses
API `created_at` is the instant the response object was created (epoch
seconds), not a per-frame arrival clock — it is a single attribute of the
response, echoed on every frame that embeds the response object.

## The negative finding

**The dialect carries no prefill/decode progress signal and no completion
timestamp.** The two timestamps a client can read (`created` and `completed`)
are identical, so `completed.created_at − created.created_at` is **0**, not the
decode window. It measures nothing about decode rate. There is no
`response.finished_at`, no monotonic per-token timestamp, no prefill-boundary
frame. (vLLM serves the response object verbatim; OpenAI's public API does the
same — `created_at` is the creation instant.)

So the "server TPS = `output_tokens / (completed.created_at −
created.created_at)`" formula from the ticket's triage is **not viable**: its
denominator is 0 for a compliant server. Shipping it as-is would divide by
zero (or, if guarded, always report no TPS and silently fall back to the very
wall-clock estimate it was meant to replace).

## What this means for the fix

- **The real fix is client-side: measure from the first token, not the
  stream start.** The `stream_start` event fires when the call begins — before
  prefill. The first assistant `entry_upsert` fires when the first token
  lands. Measuring `output_tokens / (now − first_delta)` (and the live
  text-length estimate the same way) excludes the prefill gap, which is what
  made the stat "read much lower" than the true decode rate. This is the
  change `app/svelte/lib/store.svelte` makes: a per-session `firstDeltaMs`
  clock, opened by the first assistant upsert, replaced the stream-start
  clock as the TPS denominator.
- **The server `duration_secs` plumbing stays as the integration surface.**
  The client threads the two `created_at` values through
  `tau-core`/`tau-protocol` to `StreamEnd.duration_secs` and *prefers* it in
  the store when it is present and non-zero. Today a compliant server yields
  a 0 window, so the number is treated as absent and the first-delta
  wall-clock path runs. It is the only path to an *exact* server stat: a
  user can extend their endpoint to emit a real completion clock, and the
  client will pick it up with no further change. That is future-proofing an
  integration seam, not speculative code — and a 0/absent window degrading
  to the wall-clock path is the correct semantics either way.
- **No delta → no TPS.** If the stream ends before any assistant token
  (an early error), there is no first-delta clock and the guard reports no
  TPS rather than dividing by zero.

## Decision

Display TPS is the wall-clock measurement **from the first assistant token**:
`output_tokens / (stream_end − first_delta)`, with the live gauge running a
text-length estimate over the same window. The server-derived `duration_secs`
overrides it when a future endpoint provides a non-zero completion clock. The
stat is labelled an estimate; it is the best the wire allows, and it no
longer over-counts prefill. An exact server-side TPS needs an endpoint change
(a completion timestamp) and is out of scope for this ticket.
