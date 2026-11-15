# adopt_async_core

## Context

`tau-core` today is synchronous: `Store` is a blocking `rusqlite` struct and no core code is async
(`crates/tau-core/src/storage/mod.rs`, and `crates/tau-app/Cargo.toml` has no dependencies). The code
that will be async is already specced: frontends consume core's event stream and none of them polls
(`spec/index.md` invariant; R-surface-08, R-surface-09 in `changes/add_realtime_gui_output`), a
cancellation cascade must interrupt a child mid-turn (R-session-16, R-session-17 in
`changes/add_subagent_support`), the config watcher must debounce a `notify` burst (R-config-05,
R-config-12 in `changes/add_user_config`), and a model call is a stream that lives for seconds,
delivers deltas, and must be cancellable mid-stream. No clause anywhere requires a network call to a
provider, and `docs/stack.md:10` reads `Async | tokio | latest stable` and names no owner, so the
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
  (all read 2026-11-15). Measured here: the store is not the long I/O — a commit is ~0.06 ms and the
  worst commit under two writers is 41 ms, while a model stream is seconds; the setup race a sync
  open survives is 28 refusals in ~1.4 s (add_session_storage tasks.md:24, 2026-11-15). Cost:
  `futures` joins `tau-core` (`Stream` is not in std); trait objects over async methods box
  (`Pin<Box<dyn Future>>`); every payload crossing a task boundary is `Send` + owned (the ACP SDK
  demands `Send` handlers and connection futures anyway); `futures::select!` is more verbose than
  `tokio::select!` (no default arm, needs `&mut` futures); the desktop binary runs two schedulers,
  gpui's and the HTTP leaf's, and that bridge has known sharp edges (Zed's own regression test covers
  a `StreamReader` that dropped its reader on `Pending` and silently truncated uploads).
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
  `Duration::sleep`-based debounce. Measured here: 20 children at depth 3 with a reader per pipe is
  ~60 threads, and a stream per session adds a thread each, where one runtime makes them tasks.
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
R-workspace-09's lint row. `docs/stack.md`'s Async row states the rule and names today's pins, and
gains the pin when `futures` lands. The provider HTTP leaf is a crate, not a decision inside core:
the binary that has a tokio runtime shares it (`Handle::try_current()`), the binary that has none
gets a 1-worker runtime behind the client trait, which is Zed's exact resolution. No core code moves
in this change: nothing async exists yet, so the deliverable is the shape, its gates, and the two
measurements the shape depends on — added as spikes to `changes/spike_gpui_surface/tasks.md`, where
the surface clauses are written from. Rules this change does not own and names on `Open:`: the
surface stream's bound and coalescing (add_realtime_gui_output), the cancel-mid-stream scenario with
real latency (add_subagent_support), the shape of the clock and store-actor traits, and where the
provider leaf crate lives.
Eval: none needed — the decision is about who polls what, and no turn's quality depends on it.

Open: the clock/executor trait's shape (gpui injects `BackgroundExecutor`; a `Clock` trait is the
minimal core-side form); the store actor's request/reply types; the provider leaf's crate name; the
per-binary runtime topology written down in `docs/stack.md` once the first consumer lands.

## Sources (read 2026-11-15)

- Zed's executor, and the absence of a runtime there: <https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml> · <https://github.com/zed-industries/zed/blob/main/crates/gpui/src/executor.rs> · <https://zed.dev/blog/zed-decoded-async-rust> (2024-04-09)
- Zed's runtime-free provider client, and its one HTTP leaf: <https://github.com/zed-industries/zed/blob/main/crates/anthropic/Cargo.toml> · <https://github.com/zed-industries/zed/blob/main/crates/http_client/src/http_client.rs> · <https://github.com/zed-industries/zed/blob/main/crates/reqwest_client/src/reqwest_client.rs>
- tokio in core, and the two `await_holding_*` lints denied: <https://github.com/openai/codex/blob/main/codex-rs/core/Cargo.toml> · <https://github.com/openai/codex/blob/main/codex-rs/Cargo.toml>
- rusqlite beside tokio, with the blocking call named at the call site: <https://github.com/ankitects/anki/blob/main/rslib/Cargo.toml> · <https://github.com/ankitects/anki/blob/main/rslib/src/sync/request/multipart.rs>
- a runtime-neutral protocol that demands `Send` handlers: <https://docs.rs/agent-client-protocol>
- tokio's own rule about a foreign thread with an entered runtime: <https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#method.enter>
