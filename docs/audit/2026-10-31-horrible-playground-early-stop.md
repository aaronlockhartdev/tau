# Why `horrible-playground` stopped mid-task

Diagnosis only: no behavior is changed by this document. The session is
`b509ffab8c04` (`.tau/sessions/b509ffab8c04.jsonl`, 683 rows, 682 entries,
last write 2026-10-31 03:34 local). Its earlier report on the same session's
Observer overflow is `docs/audit/2026-10-31-om-observer-overflow.md`; this
document covers the *terminal* failure, which that report does not reach.

This diagnoses the work the session was doing: GitHub issue #348, the OM route.
The `om_pass.rs` cites below describe that file before #348 landed, and every
line cite tracks the tree at the time of writing — `turn/mod.rs` and
`provider/stream.rs` move with each edit, so a cite here is a name plus a
position, and the name outlives the position.

## Symptom

The session was working ticket #348 in `/Users/aaron/git/tau-348`. Its last
model call asked for one `bash` tool call, the tool answered, and the agent
never spoke again. The durable record ends with a tool result no assistant
ever answered, and the only trace of why is one diagnostic card the GUI
renders as `provider cut the turn (eof_without_done) after 0 block(s)`
(`app/svelte/lib/display/diagnostics.ts:76`). The session was idle, not
failed: `process()` returned `Ok`, nothing was queued, and nothing re-armed a
turn — the Observer and compaction run at turn end and in the background
(spec §4, ADR-0004), so a turn that ends early is never resumed.

## What the record says

Times local, 2026-10-31. Ids are the record's own sequence numbers.

| id | time | kind | fact |
| --- | --- | --- | --- |
| 00000562 | 02:55:15.447 | diagnostic | `om_failed` — `400: request exceeds the context window: prompt (211490 tokens) leaves no room to answer in the context (204800)` |
| 00000563 | 02:55:15.469 | diagnostic | `om_stalled { activation: 30000, pending: 145217 }` |
| 00000667 | 03:27:38.959 | diagnostic | `turn_cutoff { blocks: 3, ended_by: "length" }` |
| 00000678 | 03:33:18.076 | assistant | `input_tokens 193477, output_tokens 1444`, calls `[bash]`, 3 378 chars of reasoning, no text |
| 00000679 | 03:33:18.145 | tool | the `bash` result (411 chars) |
| 00000680/81 | 03:33:41.59 | om | the observation pair written at turn end |
| **00000682** | — | **absent** | **a hole in the sequence: the id was minted, no entry was written** |
| 00000683 | 03:34:33.325 | diagnostic | `turn_cutoff { blocks: 0, ended_by: "eof_without_done" }`, parent 00000681 |

Two facts in that table decide the diagnosis:

- **The hole at 682.** The call's assistant entry is minted before the
  provider is called (`turn/mod.rs:141-147`, ADR-0008), and
  `append_assistant` returns `Ok(())` without writing when the result has no
  text, no reasoning and no calls (`turn/mod.rs:509-520`, "no content is no
  entry"). The id is burned, the entry never exists, and the file shows a
  sequence gap. So the record proves the last call produced **nothing at
  all** — not a partial answer that was truncated.
- **75 seconds of nothing.** The tool result is at 03:33:18.145; the cutoff is
  at 03:34:33.325. The call sat ~75 s and came back with an empty body.
  `requests.idle_timeout_secs` is 120 (`docs/config.md:133`), so the stream
  never crossed the idle deadline — it ended by EOF before it did.

## Feedback loop

Built before any hypothesis was chosen, in a throwaway worktree
(`git worktree add /tmp/diag-348 HEAD`, HEAD `71ed1eb`), on the existing
in-repo seams (`agent::testkit::{make_agent, ScriptedProvider, sse,
entries_of}` — the same seam `agent/tests_cutoff.rs:73` uses to pin
`eof_without_done`). A 24-second build, deterministic, no network, no GUI.

- `crates/tau-core/src/agent/tests_dead_stream.rs::a_dead_stream_ends_the_turn_and_leaves_the_work_hanging`
  — the mechanism, **green**: script one completed `bash` call then a stream
  that answers nothing. The run reproduces the incident exactly: the record is
  `user, assistant, tool, diagnostic`; the diagnostic is
  `turn_cutoff/eof_without_done/blocks 0`; the record ends on an unanswered
  tool result; `process()` is `Ok`; the provider was called exactly twice
  (no retry); nothing is queued.
- `…::a_call_that_answered_nothing_is_retried_before_the_turn_gives_up` — the
  fix contract, **red**: `assertion left == right failed: the call that
  produced no blocks is retried — left: 2, right: 3`.

Red-capable on the bug, green on a fix. Command:
`cargo nextest run -p tau-core --lib 'agent::tests_dead_stream'`.
The tests are not committed to `master`; they exist only in the scratch
worktree, because this task is diagnosis.

## Hypotheses, and what falsified them

| Hypothesis | Verdict | Evidence |
| --- | --- | --- |
| The user cut it (stop, force-send, close) | **killed** | `reported_cutoff` (`turn/mod.rs:588-590`) returns `None` for `Completed` **and** `Killed`; a user cut is recorded as `interrupted`, not `turn_cutoff`. A cutoff record means nobody asked for it. |
| The context gate refused the call (the #343/#344 path) | **killed** | A gate refusal is a `context_overflow { prompt, budget }` record plus `AgentError::ContextOverflow` out of `process()` — pre-wire at `gate_cap` (`turn/mod.rs:104-117`) or after the provider's own numbers (`turn/mod.rs:374-378`). The record holds no `context_overflow` at all (its four diagnostics are `om_failed`, `om_stalled`, two `turn_cutoff`s), and `turn_cutoff` is written only after a call returns (`turn/mod.rs:182-187`) — the call reached the wire. |
| The OM overflow killed the turn | **killed as the terminal mechanism, kept as the trigger** | An OM failure records `om_failed`/`om_stalled`, never `turn_cutoff`; the pass is best-effort and the turn proceeds (`turn/om_pass.rs:79-83,115-117`). |
| Idle-timeout, then a retry ladder that gave up | **killed** | The idle deadline is 120 s; the call ended at ~75 s. A retryable failure records no `turn_cutoff` (`retryable` → `Err` path, `provider/stream.rs:293-303`), and the provider was called twice in the whole repro run. |
| A parse drop hid a real response | **killed** | A parse failure is `ProviderError::MalformedStream`, not retryable, recorded as `provider_rejected` — not `turn_cutoff` (`turn/mod.rs:394`). And `blocks: 0` means no frame decoded. |
| A provider stream that answers nothing ends the session's work, silently | **confirmed** | Reproduced (green test above), and it matches every entry in the record's tail. |

## The cause

Three links, each in a different place. Only the second is the reason the
session *ended*; the first is why it was vulnerable, the third is why nobody
could see it.

1. **The prompt sat at the declared ceiling (trigger).** The last provider
   charge was 193,477 input tokens. The declared ceiling for
   `dev-qwen/qwen3.8-flash-next-iq3_s` (`context_window = 200000`) is
   `prompt_budget = 200000 − 4096 − 4096 = 191,808` (`docs/config.md:99-102`,
   `CONTEXT_GATE_MARGIN` = `MIN_ANSWER_ROOM` = 4096). The gate budgets on the
   *estimate*: the provider's count for the last request plus `len()/4` for
   every entry appended since, with a 20 % serialization allowance
   (`estimate.rs:63-69`, `om/mod.rs:294-297` — "soft estimator", and
   `request.rs:397-399`). On a code- and tool-dense tail (this session: 382
   tool entries) that heuristic under-counts, because a real tokenizer runs
   near 3 bytes per token there; the estimate under-counted and the real
   prompt went 1,669 tokens past the ceiling the gate is supposed to hold.
   The same edge explains 00000667: the clamp
   cap to ~2.4k tokens, and the turn ended `length` after 3 blocks. The
   upstream condition is the one the prior report documented — the raw window
   grows unbounded and the Observer's own route is ungated, which is why the
   Observer request reached 211,490 tokens and drew the 400 (prior report
   Defect A; re-verified at HEAD: `provider::OmRoute::run`
   (`provider/mod.rs:187-224`) builds and sends a body with no gate, and
   `record_om_failure` (`turn/om_pass.rs:253-269`) latches `om_failed` once
   per stage per run and never reaches the `om_stalled` check, so a permanently
   dead route is durable exactly once).
2. **A call that produces nothing is the one dead-stream shape tau does not
   treat as a failure (the reason it ended).** A body that ends at EOF with no
   completion frame is `StreamOutcome::EofWithoutDone` returned as **`Ok`**
   (`provider/stream.rs:447-452` → `:489`). Every retry rule keys on
   `Err` (`provider/stream.rs:293-303`, and the two in-body paths that *do*
   return `Err` when nothing was received: `Ok(Err(_))` → `Request`,
   idle deadline → `IdleTimeout`, with the code's own words: *"a pre-body
   failure (nothing received) still fails and retries"*). So a 0-block dead
   stream skips the ladder the provider layer already runs for a dead stream,
   and the turn loop breaks on any outcome that is not `Completed`
   (`turn/mod.rs:201-228`, the bare `break` at `:227`) — a rule whose
   justification (spec §7, ADR-0034: keep the partial, never re-send it) does
   not apply when the partial is empty. A `break` on an empty partial throws
   away the only thing the call could have contributed, and the turn — the
   whole agentic loop, the user's task — is over.
3. **The silence is by design, and the design has a hole (why nobody could
   react).** The empty result writes no entry (`turn/mod.rs:509-520`), burning
   the minted id 682 and leaving the sequence gap. What survives is one
   diagnostic card (`turn/mod.rs:182-187`), and the GUI's rendering of it is a
   sentence about a cut, with no statement that the model produced nothing and
   no path back to work. Nothing re-arms the turn: compaction and the Observer
   are background-only, no continuation hint fires, no queue item is pending.
   ADR-0034 was written for exactly this cluster — *"eleven turns whose
   provider stream ended without a completion signal … agents randomly stop"*
   — and its decision made the fact **visible**; the reaction was left as
   follow-on work, and this is that hole.

## Owner

- `crates/tau-core/src/provider/stream.rs` (the outcome/retry split at
  `:22-32`, `:444-451`) plus `crates/tau-core/src/agent/turn/mod.rs`
  (`:201-228` the break, `:515-519` the empty append) — the missing reaction
  to a call that produced nothing.
- `crates/tau-core/src/agent/turn/estimate.rs` + `om/mod.rs:294-297` — the
  gate's budget is `len()/4` for appended material, and the real prompt
  crossed the declared ceiling by 1,669 tokens (`turn/mod.rs:104-117`).
- The trigger chain (unbounded raw window, ungated Observer route,
  once-latched stall record) is owned by the prior report's Defect A; #348 is
  the borrowed-endpoint half.

## The fix contract, stated as a test (not implemented)

A provider call that produced no text, no reasoning, and no calls is a
transport failure of the class the provider layer already retries: retry it,
bounded, on the same sink; if the retries also answer nothing, end the turn —
and then the record must not be silent: the sequence hole is a lie about what
happened, so the durable trace of a 0-block cut must say the model produced
nothing and that the task is unfinished, and the user must have a way to
resume it. A 0-block cut must never be counted as a normal turn end.

Red test, green when satisfied:
`agent::tests_dead_stream::a_call_that_answered_nothing_is_retried_before_the_turn_gives_up`.
A fix must also keep `agent/tests_cutoff.rs:73` green — a cut with a partial
stays a kept partial, not a retry.

## Evidence ceiling

- **The provider-side proximate cause of the empty body is not established,
  and tau cannot establish it.** At the terminal call the prompt was ~194k
  against the gateway's 204,800-token window (the number in the 400 text), so
  the gateway had ~10k of room; the response bytes for that call are not
  recorded anywhere (`wire::response` is `trace!`-level only,
  `provider/wire.rs:28-30`). A 194k-prefill slot eviction on the shared
  worker is the leading hypothesis, not a finding. The fix contract above
  deliberately does not depend on which it was.
- **The wire log could not corroborate the terminal call.** The supplied
  `/tmp/tau.log` (26 MB) covers 2026-10-31T04:14:29Z → 07:16:59Z (the
  telemetry hour-rotation ceiling, `telemetry.rs:12-13`): 2,954 wire entries,
  117 `200 OK`, 3 `400`s — all three on `om_transcript` requests, with the
  body `prompt (211490 tokens) … context (204800)`. Nothing after 07:16:59Z,
  i.e. the session's last ~18 minutes, including the terminal call, are
  outside the log.
- **The evidence directory is gone.** `/Users/aaron/Downloads/tau-348-diagnosis/`
  (the log, the record copy, the screenshots) existed when this run started
  and no longer exists; the record is re-read from `.tau/sessions/`, so every
  number above is verifiable, and the log-derived numbers are not. The
  session's own report made the same point about the log's 1-hour ceiling —
  shipping a debug build whose only provider-trace artifact self-deletes in an
  hour is what makes an "agent stopped" report un-diagnosable.
- **The binary that produced the log cannot be pinned to a commit.** The
  log's wire events (`wire.request.om_transcript`, `wire.request.response`)
  match no tracked source (`git log --all -S 'wire.request'` → empty; HEAD
  emits `llm wire` with a `label` field, `provider/request.rs:174-185`).
  Claims about provider-layer *behavior* in this document are therefore made
  about HEAD (`71ed1eb`), with the repro run against it.
- Secret handling: the config carries `api_key = "<REDACTED>"` for
  `dev-qwen`; no key appears in any quoted payload.
