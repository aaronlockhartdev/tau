# Structure plan — contracts, libraries, simplification

Status: **preliminary.** Findings are measured against `typed-facts/diagnostics-306`
(`.worktrees/typed-facts-306`) at the tip of the #306 work; line numbers are from
that tree. Nothing here is decided except where a row says *adopted* — the
decisions in §6 were taken on 2026-10-26.

Read it as: §1 what the code actually is, §2 where crate boundaries belong,
§3 which hand-rolled logic a maintained crate should own, §4 what to simplify,
§5 the order of work, §6 the decisions.

## 1. Findings — where the contracts are, and where they leak

The test applied throughout: **a crate boundary is worth it only where it turns a
violation this repo has already committed into a compile error.** Module privacy
enforces conventions; packages enforce contracts. Measured backdrop: editing any
line in `tau-core` rebuilds core + `tau-acp` + `tau-test` + `tau-app` in 1.8 s,
821 Rust tests run in 28 s, and no code file exceeds the CI hard limit
(1000 LOC) — the largest are `hashline/mod.rs` 976, `hashline/registry.rs` 943,
`tools/impls.rs` 870, `harness/state.rs` 777, `harness/launch.rs` 718.

### 1.1 Contracts that already exist (evidence: implementations and who owns the fakes)

| Contract | Interface | Implementations / readers | Spec |
|---|---|---|---|
| Model transport | `TurnProvider` (`provider/canned.rs:51-247`), `TurnSink`, `TurnEvent`, `TurnProviderRef = Arc<dyn TurnProvider>` (`provider/canned.rs:29`) | 9 provider impls — production, `Canned`, `SlowCanned`, `PrefillHang`, `SlowFirst`, `Forwarding` (harness), `CannedChild`, `Flaky` (`om_integration/retry.rs`), plus `tau-mock-llm/tests/decode.rs` driving production SSE decode | ADR-0003, ADR-0009, ADR-0010 |
| Child orchestration | `SubagentBridge`, `ChildDriver`, `ChildProviderFactory` (`subagent/bridge.rs`) | production adapters `harness/forwarding.rs:59,232,269,300`; fakes `subagent/testkit.rs`, `harness/testkit.rs`, `tau-core/tests/dispatch_commands.rs`, **`tau-test/src/tests_accept.rs`** | ADR-0001, ADR-0006 |
| Durable history | no trait, correctly: the interface is the file | GUI, ACP, the byte-identical 10k fixture generator, future builds | ADR-0005, ADR-0034 |
| Client-facing core API | `harness::{Core, CoreBuilder, CoreError}` over `Command`/`Event` | `app/src-tauri`, `tau-acp` (3 files), `tau-test`, `tau-core/tests/` | ADR-0006 ("no richer in-process API") |
| Edit anchoring | 17 names used by 4 consumers; zero outbound edges | tools, agent, harness, subagent | ADR-0030–0033 + two on-disk formats |
| Audit channel | one writer `AgentSession::diagnostic` (`agent/config.rs:119`), one reader `diagnostics::project` | core + the frontend card | ADR-0034 |

### 1.2 Single-Choice violations (Meyer, via the design-principles page: *one and
only one module should know the exhaustive list*)

| Site | The leak |
|---|---|
| `tools/surface.rs:97` `name if name.starts_with("subagent_")` | tool routing decided by a name prefix; the file's own comment records a past bug from this shape |
| `tau-protocol/src/snapshot.rs:179-185` `OwnState { state: String, waiting_on: Option<String> }` | flattens the `ChildState` sum type (`subagent/states.rs`); `{state:"done", waiting_on:"user"}` type-checks |
| `tau-core/src/tools/mod.rs:39-44` `ToolResult { ok: bool, cancelled: bool }` | three-valued state in two bools; the TS mirror already models four states |
| `harness/snapshot.rs` `preview()`, `om_integration/mod.rs:258+` `entry_text()` | payload key soup — `text｜note｜state｜output｜name` probed in order; typed only for `diagnostic` since #306 |
| `harness/snapshot.rs` `recorded_model`, `gate.rs` (pre-#306) | facts recovered by prose matching; closed by #306 for the audit channel only |

### 1.3 Hazards

1. **Background compaction mutates the agent's private lock** —
   `om_integration/background.rs:59,115,181,234` hold
   `Arc<Mutex<crate::agent::Inner>>`; `turn_end/mod.rs:147` takes
   `&crate::agent::OmStatusHook`. The invariant "the store and the OM record move
   together" lives in a module that cannot see either owner.
2. **Poisoned mutexes are unhandled at every lock site.** ~423 `.lock()` calls,
   production written as `.lock().expect("agent inner: no panic while the lock is
   held")` (e.g. `agent/mod.rs:346`) and many `.lock().unwrap()` in test code.
   The stated invariant is not guaranteed: code under the lock parses
   model-supplied text. One panic poisons the mutex, after which **every** later
   call panics — a single bad turn stops the app for the rest of the process
   lifetime. This is the same failure family as the "agent silently stops"
   reports that opened this session.
3. **Diagnostic leaf chaining was the same bug twice** in one day: two appends
   sharing one pre-computed leaf produce siblings, so the branch walk orphans the
   first record and the next open re-emits it (`harness/launch.rs`
   `record_model_at_open`, `harness/dispatch_sessions.rs:196-211`). The
   "records chain linearly" rule has no owner.
4. **A name collision across the seam — resolved:** `Event::StreamEnd` (protocol:
   the turn settled) vs `tau_protocol::StreamOutcome` (how the provider ended the
   stream). The pending `turn_cutoff` work writes code on both names, which is why
   the record vocabulary moved (§6.1).

## 2. Crate boundaries

Adopted shape — **three library crates plus one forced leaf**, `tau-core`
unchanged in name and still the largest member:

| Crate | Owns | Prerequisite cut |
|---|---|---|
| `tau-tokens` | "how many tokens is this text" — OM accounting and request sizing must agree | move `om::token_count` (`om/mod.rs:292`) out; 7 of `provider`'s 9 outbound edges are `provider/request.rs` → `om` |
| `tau-provider` | `TurnProvider`/`TurnSink`/`TurnEvent`, request build, SSE decode, canned providers | the tokenizer edge; nothing else (fan-in 29, fan-out 9 → 2) |
| `tau-session` | the file: JSONL+CRC, sidecars, branch semantics, **a typed payload per `EntryKind`** | none (fan-out already 0). Also relocate `om_integration::branch_entries` (`om_integration/mod.rs:179`) here — 4 of its 5 callers are not OM |
| `tau-hashline` | anchors + pin store (ADR-0030–0033, ADR-0031/0032 on-disk formats) | none (fan-out already 0) |

Kept as modules, with reasons recorded so the question is not reopened every
quarter: `om` (after the dispersal below it has one consumer and fan-out 1 — a
single-consumer leaf does not earn a package), `subagent` (the ports are already
the contract and its fakes already live in three crates; splitting it forces
`SessionRole` and child-state types above the seam), `config`/`plan`/`skills`/
`gate`/`diagnostics` (policy of the loop), `tools` (until `ToolTarget` replaces
the name-prefix dispatch, §1.2).

`om_integration` is **dispersed, not packaged**:

| Piece | To | Note |
|---|---|---|
| `branch_entries` (`:179`) | `tau-session` | a branch walk, not an OM concern |
| `entry_text` + the `is_raw` shim (`:251-270`) | the entry model | `EntryKind::is_raw()` already lives in `tau-protocol/src/kind.rs:69` |
| `fork_record` (`:208`), `seed_compacted_child` (`:335`) | `om` | record semantics on fork/spawn |
| `recall` (`:361`) | `tools` | it is a tool; today it takes three hops from `tools/surface.rs:94` to reach it |
| `OmState` (`:37`), `NoopSink` (`:320`) | `agent` | `Inner.om` already holds it; living elsewhere is what forces hazard 1 |
| `turn_end/`, `turn_end/commit.rs`, `retry.rs`, `background.rs` | `agent/om_pass/` | the only genuine OM adapter — and its `agent::Inner` reach-in is replaced by a named port (store handle, status hook, cancellation) |

Costs to carry, both from *Prefer small crates* and both verified against this
repo: the workspace declares **no `[profile]` at all**, so release builds run
with no LTO — splitting crates removes cross-crate inlining from the shipped app
and the release E2E binary, so the split ships with `lto = "thin"` in the release
profile. And the boundary must be written down: supersede ADR-0002 with an ADR
stating the rule (leaves out; no cycles; `tau-protocol` stays the only
shared-vocabulary crate; adapters only in the root) and naming the Single-Choice
+ design-by-contract criterion this plan used.

### 2.1 The split has to be visible in the tree

A boundary that exists only in prose is not a boundary. When functionality
splits, the directory layout splits with it: **one directory per contract, named
for the contract, holding its interface, its adapters and its fakes.** Nothing
that belongs to a contract stays in a file named after a container, and no
contract is spread across two unrelated names. The target tree, annotated with
where today's code comes from:

```
crates/
  tau-tokens/src/lib.rs        om::token_count (om/mod.rs:292) + the calibrated
                               estimator, once the usage-fact fix lands
  tau-provider/src/
    lib.rs                     TurnProvider, TurnSink, TurnEvent, TurnProviderRef,
                               ProviderError          (provider/mod.rs)
    production.rs              ProductionProvider     (provider/mod.rs)
    canned.rs                  Canned, SlowCanned, PrefillHang, SlowFirst
    request.rs                 request build          (unchanged)
    sse/mod.rs                 SseParser              (provider/mod.rs:131)
    sse/decode.rs              the stream loop        (provider/stream.rs)
    sse/properties.rs          proptests              (provider/sse_properties.rs)
    wire.rs                    wire shapes            (unchanged)
  tau-session/src/
    lib.rs                     SessionStore facade    (session/mod.rs)
    file.rs                    JSONL + CRC + sidecars (session/file.rs)
    branch.rs                  branch_entries         (om_integration/mod.rs:179)
    entry.rs                   EntryKind ↔ its payload shape per kind
  tau-hashline/src/            lib.rs, registry.rs, pin_store/, diagnostics.rs
  tau-core/src/
    agent/
      mod.rs                   Inner, AgentSession, TurnConfig, gate facts
      config.rs                the diagnostic writer, entry append
      gate_hook.rs             completion-gate hooks
      turn/
        mod.rs                 the round loop, context gate, rejection record
        assemble.rs            entry → prompt items
        gate_turn.rs           the gate's own model call
        om.rs                  *calls* the pass (today turn/om_pass.rs)
      om_pass/                 the whole compaction operation, one directory:
        mod.rs                 om_turn_end + record_om_stall
        commit.rs              (turn_end/commit.rs)
        retry.rs               (om_integration/retry.rs) → backon per §3
        background.rs          (om_integration/background.rs)
        port.rs                the named port that replaces &mut agent::Inner
        transcript.rs          windowing over the log (turn/tests/transcript.rs)
    om/                        records, groups, observer, reflector
                               + fork_record, seed_compacted_child
    tools/
      mod.rs, surface.rs, impls.rs
      recall.rs                (om_integration/mod.rs:361)
    diagnostics.rs             the projection + append_facts (§4 S1)
    harness/
      open_facts.rs            record_model_at_open + open_facts, out of launch.rs
      launch.rs, dispatch_sessions.rs, state.rs, snapshot.rs, turn.rs, …
```

Two rules that make the tree stay honest, both reviewable:

1. **A contract's fakes live in its own directory.** `TurnProvider`'s live next
   to it in `canned.rs`; `SubagentBridge`'s in `subagent/testkit.rs`. If a
   contract has fakes that live somewhere else, the contract is in the wrong
   place — that is how `tau-test` ended up with its own copies.
2. **One operation, one name, one directory.** `om_integration` and
   `agent/turn/om_pass.rs` are the same operation under two names in two trees;
   the dispersal ends with `agent/om_pass/` and the shim file deleted, its
   `om_turn_end` entry point moved into the directory it triggers.

The same rule applies inside `tau-core`: `launch.rs` (718) and
`dispatch_sessions.rs` (628) are both over the soft limit, and the code pushing
them over — open-time fact emission — is a named piece of the audit channel, so
it moves to `harness/open_facts.rs` rather than being split by accident of
length.

### 2.2 Wire conversion is derived serde, not built JSON

A type that crosses the wire derives `Serialize`/`Deserialize`; the conversion is
not written by hand. `tau-protocol` already lives by this: the payload types,
events and snapshots are derived, and `to_value` is a thin wrapper over the derive
(a macro at `payload.rs:190`). Two hand-written impls were left; one was a defect.

- `Diagnostic`'s hand-written `Deserialize` is replaced by the derive. Tolerance
  sits in `from_value`, the one place that needs a name: a `code` this build
  cannot decode becomes `Unknown { unknown_code }`, and no field value is
  invented. The defaults the hand-written reader filled — `""`, `0` — were a live
  hazard: a truncated `model_changed` read back as a model named by the empty
  string, which the harness would then take as the session's model.
- `EntryKind`'s hand-written `Serialize`/`Deserialize` stay, three lines each
  delegating to `as_str()`/`parse()`: an open tag set with `Other(String)` cannot
  be derived without dropping the unknown tag's name, and nothing is defaulted.

The rest of the hand-built JSON is not a serde problem but an untyped-payload
one: `json!` literals in 12 `tau-core` files and 83 `payload.get("key")` reads are
payloads that have no type yet. Converting them is the per-kind typed-payload work
the `tau-session` cut is meant to carry (§2); each one converted is a reader that
stops guessing at keys.


## 3. Library audit

Dep hygiene is already good — `jiff`, `gix`, `similar`, `notify` +
`notify-debouncer-full`, `xxhash-rust`, `zstd`, `sha2`, `rand` + `names`,
`reqwest`, `insta`, `proptest` are all current and doing real work. Nothing was
found that should be *removed*. Numbers are crates.io downloads and last publish
as of 2026-10-25.

| Hand-rolled today | Candidate | Verdict |
|---|---|---|
| Error `Display`/`Error` impls, 10 files | `thiserror` (1.70 B dl, 2026-09-23) | **adopt.** AGENTS.md requires `std::error::Error` + lowercase concise `Display`; `#[derive(Error)] #[error("…")]` is that, with less code and no `match` arms drifting from variants. Keep hand impls where the message is computed |
| Backoff arithmetic + jitter, `om_integration/retry.rs:60-70` (`base * (0.8 + 0.4 * jitter)`, 1 s → 120 s) | `backon` (102 M dl, 2025-10-18; the tokio-recommended retry crate) | **adopt.** Replace the hand-rolled policy with `ExponentialBackoffBuilder` (max interval, jitter) and keep tau's budget/logging loop. Do not adopt the archived `backoff` crate |
| Token estimate `chars().count() / 4` (`om/mod.rs:292-295`), used by the context gate | `tiktoken-rs` (22 M dl, 2026-09-24) | **reject as stated, adopt a better fix.** A BPE tokenizer is correct only for the model family whose vocab it ships, and ADR-0003 keeps tau provider-generic. The honest fix is calibration: every assistant record already carries `usage.prompt_tokens` from the provider, so measure the real count and correct the estimate, keeping the estimator as the cold-start fallback. An under-estimate here is what lets a prompt over the real window through the gate — i.e. a provider 400 and a stopped agent |
| SSE parser, `provider/mod.rs:131` + `stream.rs` (~100 LOC, `sse_properties.rs` proptest) | `eventsource-stream` (30 M dl, last publish **2022-02-17**), `sse-codec` (1.7 M dl, 2026-02-12), `reqwest-eventsource` (12 M dl, 2024-03-29) | **reject — keep hand-rolled.** The stale one is unmaintained; the live one is a frame codec that cannot express what tau's contract needs: the `[DONE]` sentinel, `eof_without_done`, mid-stream error, and the idle-timeout boundary that `StreamEnd` names. Replacing it would move the distinction into a dependency and lose the property tests |
| Newtype boilerplate for `ModelName` (the N10 fix in §4) | `derive_more` (552 M dl, 2025-12-22) | **optional.** AGENTS.md's `into_inner()` convention is 4 lines; add the crate only if a second newtype appears |
| Arg loops in `tau-mock-llm/src/main.rs:20` and `tau-test/src/main.rs:126` (3 flags total) | `clap` (1.27 B dl) | **reject.** Two binaries, three flags, no subcommand trees; the dep tree costs more than the loop |
| Mutex + poisoning (§1.3 hazard 2) | `parking_lot` (1.20 B dl, 2026-10-21) — no poisoning concept at all | **reject the crate, keep `std::sync::Mutex`** (§6.2). A guard that can never be poisoned hides the one signal that shared state was mutated mid-panic; the guard-size and speed difference is noise against file IO and provider latency. The fix is dependency-free: recover with `PoisonError::into_inner()` where continuing is safe, and stop parsing untrusted model text under the lock |
| `SystemTime → UNIX_EPOCH → millis` in 7 places while `jiff 0.2` is already a dependency | (none needed) | **simplify to the existing dep** — `jiff::Timestamp::now()`, see S5 |
| Hand-built JSON: `json!` payload writers (12 `tau-core` files), 83 `payload.get("key")` readers, `Diagnostic`'s hand-written `Deserialize` | serde derive — already the repo's norm in `tau-protocol` | **adopt the derive wherever the shape has a type** (§2.2). Done for `Diagnostic`; the `json!`/key-read residue is the untyped-payload backlog, S12 |

## 4. Simplifications

Behaviour-neutral unless the row says otherwise. Each is sized so one session can
finish it, and each keeps the existing tests as the harness.

| # | Change | Where | Why |
|---|---|---|---|
| **S1** | One fact-append helper: `diagnostics::append_facts(&mut store, facts)` that re-reads the leaf between appends | `harness/launch.rs` (`record_model_at_open`), `harness/dispatch_sessions.rs:196-211`, used by `agent/config.rs:119` too | hazard 3: the chaining rule gets one owner instead of being re-derived at each call site; also drops the `leaf()`/`leaf_cached()` inconsistency |
| **S2** | One truncation module (`bounded_head` char-boundary, `truncate_lines` line-boundary + marker) | `agent/turn/mod.rs:17`, `gate.rs:82`, `subagent/inspect.rs:198` | three helpers, two semantics, one concept; the frontend's `firstLine` mirrors the same idea |
| **S3** | Poison recovery at the lock: one `lock()` helper (or `parking_lot`) and no `expect` at lock sites | ~423 sites, worst in `agent/mod.rs`, `harness/state.rs` | hazard 2; converts a process-wide stop into one failed turn |
| **S4** | Disperse `om_integration` per §2, ending with the port replacing `Arc<Mutex<agent::Inner>>` | `om_integration/*` | hazard 1; also the prerequisite for `tau-om` if that is ever revisited |
| **S5** | Collapse the 7 `UNIX_EPOCH` conversions onto `jiff::Timestamp`, one `now_ms()` | `session/mod.rs:206`, `session/file.rs:315`, `harness/state.rs:531`, `hashline/pin_store/mod.rs:130`, `tools/impls.rs:393`, `om_integration/mod.rs:321` | already a dependency; two incompatible epoch idioms today |
| **S6** | `context_gate_budget` and `gate_unavailable_reasons` computed from one function returning `Result<budget, reasons>` | `agent/mod.rs:110,124` | they must never disagree — #306 had to reconcile exactly that drift by hand |
| **S7** | `ToolTarget` enum (`Subagent { name }` / `Core`) replacing the prefix test | `tools/surface.rs:97` | §1.2; the audit already recorded a bug this shape caused |
| **S8** | `ChildStatus` typed in `OwnState`, and a `ToolOutcome` enum for `ToolResult` | `tau-protocol/src/snapshot.rs:179`, `tools/mod.rs:39` | §1.2; both are wire-visible, so they need a protocol commit + TS mirror + goldens |
| **S9** | Delete `root_with_supervisor` (2 references, both inside `launch.rs`; no non-test caller) | `harness/launch.rs:213,220` | a test seam living in the production open path |
| **S10** | `thiserror` on the error enums | 10 files | §3 |
| **S11** | `backon` for the OM retry policy | `om_integration/retry.rs` | §3 |
| **S12** | Typed payload per `EntryKind`: the `json!` writers and `payload.get` readers become the derived payload types | 12 files with `json!` (`tools/mod.rs`, `subagent/*`, `session/file.rs`, …), 83 key reads | §2.2; one shape per kind, so a reader cannot guess a key and each kind gains exactly one legal append path |

## 5. Order of work

1. **S1, S2, S9** — small, mechanical, no wire change. Do on the #306 branch,
   which owns the code they touch.
2. **S6, S5, S3** — loop and store hygiene; S3 is the only one with real churn
   and it is where the app-wide stop lives.
3. ~~Rename the `StreamEnd` collision~~ — done (§6.1). Land the
   `turn_cutoff`/`StreamOutcome`-on-`ProviderResult` ticket next.
4. **S4** (disperse `om_integration`) — four commits, each behaviour-neutral.
5. **S7, S8** — wire-visible; each needs goldens + the TS mirror + a protocol
   commit.
6. **The ADR**, then `tau-tokens` → `tau-provider` → `tau-session` →
   `tau-hashline`, each its own commit series, each with `lto = "thin"` in the
   same PR as the first split.
7. **S10, S11** last — dep additions are easy to review once the code is in the
   shape the plan wants.

## 6. Decided (2026-10-26)

1. **The record vocabulary was renamed, not the event.** `tau_protocol::StreamEnd`
   is now `StreamOutcome`; the TS mirror followed. `Event::StreamEnd` keeps its
   name — it is what the GUI subscribes to and its wire spelling is already
   pinned by goldens. The record type had no writers, so the rename cost four
   Rust files and two TS lines and changed no bytes on the wire.
2. **`std::sync::Mutex` stays; no `parking_lot`.** A lock whose guards are never
   poisoned is wrong for a codebase whose product is a durable, CRC-checked
   record, and the speed difference is invisible next to file IO and provider
   latency. Still open: how far recovery reaches — named boundaries or every
   acquisition (§1.3 hazard 2).
3. **`tau-hashline` stays workspace-internal.** No crates.io means no semver
   policy, no separate CI job, and no public-API discipline past the crate
   boundary.
4. **No tokenizer.** `token_count`'s `chars() / 4` stays a soft estimator. The
   gate's real fix is calibration against the `usage.prompt_tokens` every
   assistant record already carries; a tokenizer would buy precision the gate
   does not need, at the cost of a model-family dependency ADR-0003 rules out
   for a provider-agnostic core.
