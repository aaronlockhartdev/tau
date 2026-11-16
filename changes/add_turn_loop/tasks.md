# Tasks: add_turn_loop

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.
Lands after `adopt_block_log`: these clauses name blocks, requests, and state pointers.
IDs: this change claims R-protocol-01..03; R-tools-30 and 31; R-workspace-16..18; R-agent-01..15;
R-session-41..44. `add_realtime_gui_output` holds R-protocol-04 in the same domain; the corpus check
requires every number up to a domain's highest to be issued, so the range is split between the two open
changes, recorded here and in that change's tasks.md.

- [ ] spike: one stub provider stream polled from a tokio executor and from a plain futures executor in the same process, proving the event payload crosses the executor boundary owned (gate: none; record the result here)
- [ ] gates: the rows `core has no HTTP client` and `core installs no subscriber` are in `AGENTS.md`, in the shape of `core has no runtime`, and green today; prove each red with a scratch dependency in `crates/tau-core/Cargo.toml`, then delete the scratch (R-workspace-18, R-workspace-16)
- [ ] R-protocol-01: the event set as Rust types in `crates/tau-protocol`, generated into `contracts/`; the `contracts current` gate reports the new files
- [ ] R-protocol-02: the tool's declared shape in `crates/tau-protocol`, its schema generated, and the stub tool's schema among them; the scenario reads the generated schema and asserts it names the tool's input schema, and deleting that field from the type turns it red (@R-protocol-02)
- [ ] R-protocol-03: the block kind set and the request reason set as generated shapes in `crates/tau-protocol`, into `contracts/`; the scenario reads both sets and asserts every kind a block commits and every reason a request row is there (@R-protocol-03)
- [ ] R-tools-30: the registry type core consumes, and the stub tool in `crates/tau-test` answering a call through it (@R-tools-30)
- [ ] R-tools-31: the trait that runs a tool, declared in `tau-core`, with the stub tool implementing it (@R-tools-31)
- [ ] R-workspace-18: confirm `tau-core`'s graph carries no HTTP client after the trait lands
- [ ] R-agent-01: the loop in `crates/tau-core/src/agent/`, committing a block at the stream's block stop
- [ ] R-agent-02: deltas folded, never committed; the scenario counts commits
- [ ] R-agent-03: the context built from the log and sent whole, with the stub asserting a full context on every request
- [ ] R-agent-04: a tool call block answered by a result block, carrying the reason when the tool did not run
- [ ] R-agent-05: the partial text committed on a mid-stream cancel
- [ ] R-agent-06: the truncated call not committed, on a cancel that lands mid-call
- [ ] R-agent-07: a refusal recording the request row with its reason, and no block
- [ ] R-agent-08: a stream gap failing the request with the stream-gap reason
- [ ] R-agent-09: resume closing a reason-less request as interrupted, driven by the crash child
- [ ] R-agent-10: the in-memory steering queue applied and recorded at the request boundary, in arrival order
- [ ] R-agent-11: the queue cap refusing further input, with the lane at the cap; no clause names the cap's key, so this change asks `add_user_config` for it
- [ ] R-agent-12: stop conditions read from config — token budget, wall clock, repeated tool call; these keys are unnamed today, so this change asks `add_user_config` to hold them
- [ ] R-agent-13: slot order in the builder, stable slots first, volatile last
- [ ] R-agent-14: injection blocks naming slot and bytes, and the rebuild that reproduces the sent bytes
- [ ] R-agent-15: the session start output line at `info` on stderr, and the scenario tagged `@R-agent-15` and `@R-workspace-08` that lets `change_spec_requirements` archive
- [ ] R-session-41: the session's working directory recorded, and the resume that reads it
- [ ] R-session-42: the last model recorded per request, and a mid-session model switch in the lane
- [ ] R-session-43: the adjective-noun display name, generated at the surface boundary with a seeded generator in the lane
- [ ] R-session-44: UUID v7 ids in canonical lowercase text, and the sort-by-id scenario
- [ ] R-workspace-16: the `tracing` facade in the libraries, a subscriber installed only in binaries, logs on stderr
- [ ] R-workspace-17: the log lines on stderr at `info`, with the lane asserting stdout stays empty
- [ ] lane: `crates/tau-test/tests/features/agent.feature` driven by the in-process stub provider; steps in `crates/tau-test/src/steps/agent.rs`; `STEPS.md` regenerates
- [ ] docs: `docs/stack.md` pins for `futures`, `tracing`, `uuid`, `petname`, and the row naming the provider leaf when it lands
- [ ] docs: `docs/glossary.md` — `loop` (what R-agent-12 stops) and `delta` in its token sense, both used by these clauses and defined nowhere
- [ ] archive: lands after `adopt_block_log` (its clauses own blocks and requests) and before
`change_spec_requirements`, which archives on the scenario `@R-workspace-08` this change's `R-agent-15`
writes; at archive the counts move by what the deltas add — `spec/agent/spec.md` +15, `spec/protocol/spec.md`
+3, `spec/tools/spec.md` +2, `spec/workspace/spec.md` +3, `spec/session/storage/spec.md` +4 — read off the
files, with `spec/session/spec.md`'s leaf cell and `spec/index.md`'s Corpus count moved to match
- [ ] check: `cargo run -p tau-tools --bin gates` (runs every `run` row: ci lane reports the rest — spec tool, spec lint, contracts current, doc lint, links, unit + surface, behaviour, steps current, dependencies current, no local state tracked, toolchain pinned, core has no UI logic, core has no runtime, core spawns nothing, core has no HTTP client, core installs no subscriber, lints); rows in state M0 — live output under load, headless session, desktop — reported `skipped (M0)`; manual rows — workspace layout, eval — reported by hand
- [ ] spec-review (reviewer ticks)
