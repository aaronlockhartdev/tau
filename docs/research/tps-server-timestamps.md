# Research: server-side TPS from the responses-API stream (ticket #224)

Researched 2026-10-20. The goal: make the GUI's decode-rate stat reflect the
model's true tokens-per-second instead of a wall-clock figure that counts
prefill and network time. The candidate signal is the two `created_at` fields
on the `response.created` and `response.completed` SSE frames.

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

- The client *can* carry the two `created_at` values and compute their
  difference — that is the change this ticket implements in
  `tau-core`/`tau-protocol` (threaded through to `StreamEnd.duration_secs`).
  It is future-proof: if a provider ever stamps a distinct completion time,
  the number is already there.
- For a compliant server today the difference is 0, so the client treats a
  zero-length window as **absent** and the GUI falls back to its existing
  wall-clock estimate. No regression, no divide-by-zero.
- The wall-clock estimate (stream start → stream end) remains the v0
  source of truth for the displayed TPS. It over-counts prefill + transport,
  which is the inaccuracy the ticket wanted to fix — but that inaccuracy is a
  property of the *provider protocol*, not of the client. It cannot be fixed
  client-side without a server signal that the protocol does not provide.

## Decision (fallback UI)

Keep the wall-clock TPS estimate as the displayed value. Thread the
server-derived `duration_secs` through the protocol and *prefer* it in the
store when it is present and non-zero; when it is absent or zero (the common
case for a compliant server), fall back to the wall-clock figure. This is the
honest position: the stat is labelled as an estimate, and it is the best
estimate the wire allows. A true decode TPS needs a server change (a
completion timestamp) and is out of scope for this ticket.
