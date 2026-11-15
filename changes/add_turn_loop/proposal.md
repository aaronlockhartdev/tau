# add_turn_loop

## Context

No clause in the corpus names a model request. The provider client is an unspecced I/O leaf, and the
turn loop that would drive it exists in no spec: `Store::commit`, `run`, `export_jsonl` and `fork` are
called only by lane steps, and `changes/change_spec_requirements/proposal.md` cannot archive because
R-workspace-08's acceptance `@R-workspace-08` has no scenario — nothing prints session start output.

`adopt_async_core` (archived 2026-11-15) decided the shape this change inherits: core speaks futures,
carries no runtime (R-workspace-14) and starts no task, thread, pool, reactor, future driver, or child
process (R-workspace-15). R-session-01 keeps one write connection per process, so a process running N
sessions runs them through one loop. `adopt_block_log` lands the unit this loop commits: a block per
commit, or every block a stream closes at once, one request row per round-trip, and state as pointers on
the block row.

The record decides most of the loop's shape. Anthropic reports input and cache tokens at
`message_start` and the stop reason at `message_delta`; the Responses API reports true usage only in
`response.completed` and closes every item at its `done`; Chat Completions closes all blocks at
`finish_reason` and reports usage in a final chunk only when asked (platform.claude.com and
developers.openai.com, read 2026-11-15). The provider's own stored conversation state expires —
responses are kept 30 days by default, and the documented fallback for an unresolvable id is to send
full input context (developers.openai.com conversation-state guide, read 2026-11-15).

The loop is where a session's context is assembled, so it is also where an injection becomes a record.

## Options

- A. The loop is an async function in `tau-core` over a provider event `Stream`, committing blocks
  through `Store` on the caller's thread, with provider HTTP in a leaf crate behind a trait. Evidence:
  R-workspace-14/15 are live and permit futures; the store's `&mut self` fits one thread, so no channel
  layer is needed; a request has three concurrent sources (deltas, tool children, cancellation), which
  a `select!` over futures expresses and a state machine re-implements. Cost: `futures` in core,
  `futures::select!` ergonomics (pinned futures, no default arm), and the trait boundary the leaf pays.
- B. A sync state machine returning an `Action` enum. Evidence: none measured. Cost: R-session-01 gives
  one writer per process, so multiplexing N sessions through a sync loop is a hand-written scheduler.
- C. The loop lives in a tokio-owning leaf crate, core keeps only the store. Evidence:
  `changes/archive/2026-11-15-adopt_async_core/tasks.md` recorded the store-actor request/reply layer
  this shape implies. Cost: a DTO layer across the boundary, and a loop no core scenario reaches.

## Decision

A. The loop is core's async function; the provider's wire shape is a trait in `tau-protocol`; the HTTP
adapters live in a leaf crate that the binary wires in.

Consequences. New domain `spec/agent/` carries the loop: R-agent-01..15 (block at close, deltas never
committed, context sent whole with no provider-side state, a call answered by a result block, the two
cancel rules, a request recorded with no block, the stream-gap rule, resume closing an open request,
steering at the boundary with a bounded queue, stop conditions from config, injection slots, injection
bytes recorded, session start output). `spec/protocol/` gains the stream event set, the tool's declared
shape and the trait that runs it in `tau-core`, the registry rule, kinds and reasons generated as one
vocabulary, and no HTTP in core (R-protocol-01..04 and 06..07), the schemas generated into `contracts/`
under the invariant in `spec/index.md`.
`spec/workspace/` gains the logging rules (R-workspace-16, R-workspace-17): libraries emit on the
`tracing` facade, only a binary installs a subscriber, and logs go to stderr at the default level.
`spec/session/storage/` gains the session row's facts (R-session-41..44: working directory, last model,
generated display name, and a UUID v7 key in canonical text).
Two gate rows join `AGENTS.md`: `core has no HTTP client` and `core installs no subscriber`, both tree
greps in the shape of `core has no runtime`.
The lane runs on an in-process stub provider in `crates/tau-test` implementing the trait; tapes are
deferred until a working loop can generate them, so the `headless session` gate row stays `M0` and
`crates/tau-test/suites/` stays empty.
This change also closes `change_spec_requirements`: the session start output line is printed, so
`@R-workspace-08` gains its scenario.
Cost, stated: new dependencies `futures` (core), `tracing` (facade, core), `uuid` (v7) and `petname`
(name generation, at the surface boundary); three store writes per request — open, per block, terminal — each
a transaction. Contention is measured, cost is not: the store's worst commit under two writers ran 41 ms
(`changes/archive/2026-11-15-add_session_storage/tasks.md`). Tokens: the context is resent whole every request, priced by
prefix caching, where one write and nine reads cost 2.15× the input against 10× uncached: 1.25× a cache
write and 0.1× a cache read (platform.claude.com prompt-caching page, read 2026-11-15).
Eval: none needed. The mechanics are lane-testable against the stub; conversation quality is the eval
lane's question, and that gate row is `manual` with no suite file to run.

Open: the queue cap's key and default, named by the config change (R-agent-11 refuses past it); the tape
format and the `headless session` gate flip, when a working loop can record one; the context cap policy
and what it drops, which is `adopt_observational_memory`'s ground and needs a measured budget;
`add_bash_tool` for the first real tool; the provider client change for HTTP, retries, cache keys, and the
leaf crate's name;
the `blobs` change for oversized and binary payloads; two writers on one session remain
refused by nothing (the gap in `changes/archive/2026-11-15-add_session_storage/tasks.md`); steering input
typed during a generation is queued in memory, so a crash loses it — persistence is a later decision
with the queue's durability as its trigger.
