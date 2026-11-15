# adopt_async_core

## Context

`tau-core` today is synchronous: `Store` is a blocking `rusqlite` struct and no core code is async
(`crates/tau-core/src/storage/mod.rs`, and `crates/tau-app/Cargo.toml` has no dependencies). The code
that will be async is already specced: frontends consume core's event stream and none of them polls
(`spec/index.md` invariant; R-surface-08, R-surface-09 in `changes/add_realtime_gui_output`), a
cancellation cascade must interrupt a child mid-turn (R-session-16, R-session-17 in
`changes/add_subagent_support`), the config watcher must apply a changed value with no restart (R-config-05,
R-config-12 in `changes/add_user_config`), and a model call is a stream that lives for seconds,
delivers deltas, and must be cancellable mid-stream. No clause anywhere requires a network call to a
provider, and the stack's Async row named `tokio` at `latest stable` with no owner, so the
corpus is written as if a runtime had been chosen. The choice is cheap now and expensive later: the
first async core code should be written in the decided shape, and a runtime dependency in `tau-core`
is one that every consumer inherits.

## Options

- **A. Core's boundary is futures; every binary brings its own runtime.** Core returns `Future`s and
  `Stream`s, spawns nothing, owns no timer, and touches no socket; time and HTTP arrive from the edge
  as traits, and the blocking store is reached through an actor thread. Evidence: Zed runs exactly
  this — `crates/gpui/Cargo.toml` has no tokio (its executor is its own `scheduler` crate, with
  `ForegroundExecutor::to_send()` documented as "enqueues a future to run on the main thread, even
  when submitted from another thread", `BackgroundExecutor::timer`, `block_on`, and fake-clock test
  hooks `advance_clock`/`tick`/`run_until_parked`, read 2026-11-15); its provider client
  `crates/anthropic/Cargo.toml` depends on `futures` and the `http_client` trait and on no runtime;
  the runtime sits in one leaf, `crates/reqwest_client`, which resolves `tokio::runtime::Handle::try_current()`
  first and only builds a `rt-multi-thread` runtime with `worker_threads(1)` when there is none —
  its own comment: "Since we now have two executors, let's try to keep our footprint small"
  (all read 2026-11-15). Measured here, on 2026-11-15: the store is not the long I/O. A commit measured
  ~0.06 ms in a probe since deleted; 300 commits per writer across two processes ran with 0 failures
  and a worst commit of 41 ms (`add_session_storage` tasks.md:25). A child instance meeting a setup
  lock counted 28 refusals before it opened — with the retry budget raised to 1000 attempts for the
  measurement — so 50 ms of backoff each is ~1.4 s spent waiting on a lock (`add_session_storage`
  tasks.md:24). A model stream lives for seconds, and nothing in the store is measured near that.
  `futures` joins `tau-core` (`Stream` is not in std); trait objects over async methods box
  (`Pin<Box<dyn Future>>`); every payload crossing a task boundary is `Send` + owned — the ACP SDK's
  handlers, transports and connection futures are `Send` (docs.rs, read 2026-11-15), and
  `crates/tau-acp` has no dependency to inherit that from today; `futures::select!` is more verbose than
  `tokio::select!` (no default arm, needs `&mut` futures); the desktop binary runs two schedulers,
  gpui's and the HTTP leaf's, and that bridge has known sharp edges: the leaf's own regression test
  covers a hand-rolled `StreamReader` that dropped its reader on `Pending` and silently truncated
  uploads (`crates/reqwest_client/src/reqwest_client.rs`, in Sources, read 2026-11-15).
- **B. tokio in core.** Core is `async` on tokio, edges spawn onto it. Evidence: codex-rs —
  `codex-rs/core/Cargo.toml` has `tokio` with `rt-multi-thread`, `macros`, `process`, `io-std`,
  `signal`, plus `eventsource-stream` for SSE and `async-channel`, and its SQL layer is sqlx 0.9 with
  `runtime-tokio` and `sqlite-bundled` (read 2026-11-15); Anki keeps the shape inside one crate:
  `rusqlite` next to `tokio`, `reqwest` and `axum`, with `tokio::task::spawn_blocking` in
  `rslib/src/sync/request/multipart.rs` (read 2026-11-15). Cost: `cargo tree -p tau-core` gains a
  runtime, so the shape stops being mechanically checkable; every consumer inherits tokio's
  assumptions (worker count, its timer, its IO driver), and `tau-app` must host tokio beside gpui's
  scheduler permanently; the provider HTTP leaf's runtime is then the core's, shared with the store
  actor's caller.
- **C. Sync core, an async leaf per I/O source.** Evidence: the shape works for one source, and
  `async-channel` is the usual bridge. Cost: tau has three reactor-shaped sources at once — provider
  streams, a child's two pipes at `subagent.max_concurrent` 20 and depth 3, and the config watcher
  plus timers (WAL checkpoint cadence, the setup backoff, the reload debounce) — so three seams and
  hand-written reactor work: one reader thread per pipe, flags checked at turn boundaries, a
  `Duration::sleep`-based debounce. Arithmetic, unverified: `add_subagent_support` plans
  `subagent.max_concurrent` 20 at depth 3, so a reader thread per pipe is ~60 threads before a stream
  per session adds one each, where one runtime makes them all tasks; no such code exists to measure.
  Cancellation loses `select!`: a blocking socket read ignores a cancel flag, so a cancel becomes a
  socket close from the canceller.

## Decision

A: core's public surface is futures, and no runtime crate lands in `tau-core`. What gets easier: both
of tau's edges take core as-is — `tau-app` spawns core futures on gpui's executor and gets its
results back through `to_send()`, a tokio web server polls the same futures on its own runtime, and
the behaviour lane (already tokio) needs nothing new; timers become deterministic, which is the only
clean fix for a timing-vacuous scenario (this change's predecessor shipped one: a setup-race scenario
that could pass having measured zero retries, closed by counting refusals). What tau keeps:
`Store` stays blocking `rusqlite` (Anki proves the shape), reached through an actor thread, so
neither edge needs a blocking pool.

Consequences: `tau-core` depends on `futures` and on no runtime, and spawns nothing — both become gate
rows and the two delta clauses R-workspace-14 and R-workspace-15 in `specs/workspace/spec.md`
(their acceptance is the gates, because the lanes cannot observe a dependency graph). The store actor
becomes the only place a `Store` is touched, so a blocking call inside a polled future is a
programmatic error: root `[workspace.lints]` gains clippy `await_holding_lock` and
`await_holding_invalid_type` at `deny` (codex-rs denies the same two, read 2026-11-15), landing on
R-workspace-09's lint row. `docs/stack.md`'s Async row keeps the pins and points at the two clauses.
The provider HTTP leaf is a crate, not a decision inside core:
the binary that has a tokio runtime shares it (`Handle::try_current()`), the binary that has none
gets a 1-worker runtime behind the client trait, which is Zed's exact resolution. No core code moves
in this change: nothing async exists yet, so the deliverable is the shape, its gates, and the two
measurements the shape depends on — added as spikes to `changes/spike_gpui_surface/tasks.md`, where
the surface clauses are written from. Rules this change does not own and names on `Open:`: the
Rules this decision implies and this change does not own are listed under Gaps below, each with the
tasks line that tracks it; the cancel-mid-stream scenario with real latency belongs to
`add_subagent_support`, and the surface stream's bound and coalescing to `add_realtime_gui_output`.
Eval: none needed — the decision is about who polls what, and no turn's quality depends on it.

Open: the gaps this decision leaves, each with the tasks line that tracks it.

- The provider HTTP leaf's runtime rule — share a tokio runtime that exists, build a 1-worker one
  only where none exists — is decided and spiked, not gated: a tokio binary that builds a second
  runtime turns nothing red. Its clause and scenario land with the leaf's own change (tasks.md:13).
- `Store` owns a non-`Sync` connection, so decision A forces one store-actor thread as its only
  caller, and no clause owns it; its request/reply types, and the clock trait that injects the three
  timers the Decision promises (the WAL checkpoint cadence, the setup backoff, the reload debounce),
  are unwritten design, not unsaid rules (tasks.md:10).
- `Send` and owned payloads on `tau-protocol` types are a cost decision A accepts; no clause in
  `spec/protocol` says it, and the ACP frontend's change is where a scenario would (tasks.md:11).
- The gates cut both ways: `core has no runtime` reads the dependency tree, dev-dependencies
  included, and `core spawns nothing` greps the crate directory, test modules included, so
  `tau-core` has no means of polling a future in its own test that passes both. That answer lands
  with core's first async code (tasks.md:12).
- `await_holding_invalid_type` denies with no type list, so its blocking-`rusqlite::Connection` half
  cannot fire yet; the list is a task, not a rule (tasks.md:9).
- The shape rests on two measurements that are handed to `spike_gpui_surface`, not run here
  (tasks.md:7).

## Sources (read 2026-11-15)

- Zed's executor, and the absence of a runtime there: <https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml> · <https://github.com/zed-industries/zed/blob/main/crates/gpui/src/executor.rs> · <https://zed.dev/blog/zed-decoded-async-rust> (2024-04-09)
- Zed's runtime-free provider client, and its one HTTP leaf: <https://github.com/zed-industries/zed/blob/main/crates/anthropic/Cargo.toml> · <https://github.com/zed-industries/zed/blob/main/crates/http_client/src/http_client.rs> · <https://github.com/zed-industries/zed/blob/main/crates/reqwest_client/src/reqwest_client.rs>
- tokio in core, and the two `await_holding_*` lints denied: <https://github.com/openai/codex/blob/main/codex-rs/core/Cargo.toml> · <https://github.com/openai/codex/blob/main/codex-rs/Cargo.toml>
- rusqlite beside tokio, with the blocking call named at the call site: <https://github.com/ankitects/anki/blob/main/rslib/Cargo.toml> · <https://github.com/ankitects/anki/blob/main/rslib/src/sync/request/multipart.rs>
- a runtime-neutral protocol that demands `Send` handlers: <https://docs.rs/agent-client-protocol>
- tokio's own rule about a foreign thread with an entered runtime: <https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#method.enter>
