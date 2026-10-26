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
2. **Poison is inspected at four sites and ignored at four hundred.** Measured
   census across `tau-core`'s 423 `.lock()` calls: 217 production
   `.lock().expect("<invariant>")`, 196 `.unwrap()` in test and test-support
   code, and **four** sites that look at the `PoisonError` at all
   (`om_integration/background.rs:118,188`, `tools/surface.rs:78`, and one
   `let … else`), where the policy is log-and-bail. The invariant the 217
   messages state — *no panic while the lock is held* — is not guaranteed: code
   under the lock parses model-supplied text. One panic poisons the mutex, after
   which **every** later acquisition panics, so a single bad turn stops the app
   for the rest of the process lifetime. Same failure family as the "agent
   silently stops" reports that opened this session. §6.2 decides the lock type;
   it is also why S3's two guardrails are requirements, not advice.
3. **Diagnostic leaf chaining was the same bug twice** in one day: two appends
   sharing one pre-computed leaf produce siblings, so the branch walk orphans the
   first record and the next open re-emits it (`harness/launch.rs`
   `record_model_at_open`, `harness/dispatch_sessions.rs:196-211`). The
   "records chain linearly" rule has no owner.
4. **A name collision across the seam — resolved:** `Event::StreamEnd` (protocol:
   the turn settled) vs `tau_protocol::StreamOutcome` (how the provider ended the
   stream). The pending `turn_cutoff` work writes code on both names, which is why
   the record vocabulary moved (§6.1).
5. **The stress leg's §8 perf bar can go red on a measurement, not a render.** A
   failing WebKit run recorded `max gap 50036 ms over 2 frames` with the 50 ms
   timer averaging 729 ms: the webview stopped painting, so `rAF` sampled a
   suspension instead of the transcript. The same code passed the next run, so a
   red there is read against the frame count before it is believed.

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
| Token estimate `chars().count() / 4` (`om/mod.rs:292-295`), used by the context gate | `tiktoken-rs` (22 M dl, 2026-09-24) | **reject; anchor instead** (§6.4, S13). A BPE tokenizer is correct only for the model family whose vocabulary it ships, and ADR-0003 keeps tau provider-generic. The estimate's *scope* shrinks instead: the provider's reported `usage` is the exact count for everything already sent, so only content appended since that response is ever guessed at. An under-estimate here is what lets a prompt over the real window through the gate — a provider 400 and a stopped agent |
| SSE parser, `provider/mod.rs:131` + `stream.rs` (~100 LOC, `sse_properties.rs` proptest) | `eventsource-stream` (30 M dl, last publish **2022-02-17**), `sse-codec` (1.7 M dl, 2026-02-12), `reqwest-eventsource` (12 M dl, 2024-03-29) | **reject — keep hand-rolled.** The stale one is unmaintained; the live one is a frame codec that cannot express what tau's contract needs: the `[DONE]` sentinel, `eof_without_done`, mid-stream error, and the idle-timeout boundary that `StreamEnd` names. Replacing it would move the distinction into a dependency and lose the property tests |
| Newtype boilerplate for `ModelName` (the N10 fix in §4) | `derive_more` (552 M dl, 2025-12-22) | **optional.** AGENTS.md's `into_inner()` convention is 4 lines; add the crate only if a second newtype appears |
| Arg loops in `tau-mock-llm/src/main.rs:20` and `tau-test/src/main.rs:126` (3 flags total) | `clap` (1.27 B dl) | **reject.** Two binaries, three flags, no subcommand trees; the dep tree costs more than the loop |
| Mutex + poisoning (§1.3 hazard 2) | `parking_lot` 0.13.0 (1.20 B dl, 2026-10-21; MSRV 1.95, edition 2024) — no poisoning concept at all | **adopt** (§6.2, S3). `0.12.5` is already in the lockfile via `gix-hashtable`, the license is in the `deny.toml` allow set, the pinned toolchain (1.98.1) clears the MSRV, and `[bans] multiple-versions = "warn"` means the 0.12/0.13 overlap is reported, not fatal. Stated plainly, what is given up: a guard that can never be poisoned cannot report that shared state was mutated mid-panic. Tau was already discarding that signal at 419 of 423 sites, and its only effect was to convert one bad turn into a permanent stop. No `RwLock`, `Condvar` or `Once` in the workspace, so 0.13's removal of automatic fairness and `hardware-lock-elision` is not felt |
| `SystemTime → UNIX_EPOCH → millis` in 7 places while `jiff 0.2` is already a dependency | (none needed) | **simplify to the existing dep** — `jiff::Timestamp::now()`, see S5 |
| Hand-built JSON: `json!` payload writers (12 `tau-core` files), 83 `payload.get("key")` readers, `Diagnostic`'s hand-written `Deserialize` | serde derive — already the repo's norm in `tau-protocol` | **adopt the derive wherever the shape has a type** (§2.2). Done for `Diagnostic`; the `json!`/key-read residue is the untyped-payload backlog, S12 |

## 4. Simplifications

Behaviour-neutral unless the row says otherwise. Each is sized so one session can
finish it, and each keeps the existing tests as the harness.

| # | Change | Where | Why |
|---|---|---|---|
| **S1** | One fact-append helper: `diagnostics::append_facts(&mut store, facts)` that re-reads the leaf between appends | `harness/launch.rs` (`record_model_at_open`), `harness/dispatch_sessions.rs:196-211`, used by `agent/config.rs:119` too | hazard 3: the chaining rule gets one owner instead of being re-derived at each call site; also drops the `leaf()`/`leaf_cached()` inconsistency |
| **S2** | One truncation module (`bounded_head` char-boundary, `truncate_lines` line-boundary + marker) | `agent/turn/mod.rs:17`, `gate.rs:82`, `subagent/inspect.rs:198` | three helpers, two semantics, one concept; the frontend's `firstLine` mirrors the same idea |
| **S3** | `std::sync::Mutex` → `parking_lot::Mutex`, and the `.expect("<invariant>")` clause disappears with the `Result`. Two guardrails come with it, as requirements: model-supplied text is parsed **outside** a critical section, and no guard is held across an await | 217 production sites (worst `agent/mod.rs`, `harness/state.rs`, `harness/dispatch_sessions.rs`), 196 test sites, and four `Err` arms that lose their reason to exist | §6.2; hazard 2. A lock stops being a thing that can wedge the process, and the code stops asserting an invariant it cannot keep |
| **S4** | Disperse `om_integration` per §2, ending with the port replacing `Arc<Mutex<agent::Inner>>` | `om_integration/*` | hazard 1; also the prerequisite for `tau-om` if that is ever revisited |
| **S5** | Collapse the 7 `UNIX_EPOCH` conversions onto `jiff::Timestamp`, one `now_ms()` | `session/mod.rs:206`, `session/file.rs:315`, `harness/state.rs:531`, `hashline/pin_store/mod.rs:130`, `tools/impls.rs:393`, `om_integration/mod.rs:321` | already a dependency; two incompatible epoch idioms today |
| **S6** | `context_gate_budget` and `gate_unavailable_reasons` computed from one function returning `Result<budget, reasons>` | `agent/mod.rs:110,124` | they must never disagree — #306 had to reconcile exactly that drift by hand |
| **S7** | `ToolTarget` enum (`Subagent { name }` / `Core`) replacing the prefix test | `tools/surface.rs:97` | §1.2; the audit already recorded a bug this shape caused |
| **S8** | `ChildStatus` typed in `OwnState`, and a `ToolOutcome` enum for `ToolResult` | `tau-protocol/src/snapshot.rs:179`, `tools/mod.rs:39` | §1.2; both are wire-visible, so they need a protocol commit + TS mirror + goldens |
| **S9** | Remove the caller-supplied `Supervisor` from `SessionRole::Root` and the `root_with_supervisor` branch that exists for it; re-point the three test sites at the real open path | `harness/launch.rs:213,220`, exercised by `harness/tests_launch.rs:247,321,341`; production (`harness/sessions.rs:317`) always passes `None` | a test seam living in the production open path. Not a drive-by: the tests need a `Core`, which is why it is its own ticket rather than a deletion |
| **S10** | `thiserror` on the error enums | 10 files | §3 |
| **S11** | `backon` for the OM retry policy | `om_integration/retry.rs` | §3 |
| **S12** | Typed payload per `EntryKind`: the `json!` writers and `payload.get` readers become the derived payload types | 12 files with `json!` (`tools/mod.rs`, `subagent/*`, `session/file.rs`, …), 83 key reads | §2.2; one shape per kind, so a reader cannot guess a key and each kind gains exactly one legal append path |
| **S13** | The prompt estimate anchors on the last recorded `usage.input_tokens`; only content appended after it is estimated. Bytes/4 rounded up, serialized tool arguments included | `provider/request.rs:385` (`estimate_prompt_tokens`), `om/mod.rs:292` (`token_count`) | §6.4: the provider's count is the authority for what was already sent, so the estimator only covers the delta — one tool output, not the whole conversation, is ever a guess |
| **S14** | The usage **record** stops mirroring one API: stored fields become the concepts (`input_tokens`, `output_tokens`, `cached_input_tokens`), the Responses/chat spellings move into the provider adapter, the total becomes an accessor instead of a stored field, and `OutputTokensDetails` + `TurnUsage::reasoning_tokens()` are deleted (no production reader) | `tau-protocol/src/payload.rs`, `provider/usage.rs` (the wire decode that used to land straight on the record type), `harness/snapshot.rs` `usage_of` (one of the two readers of a stored usage) | §6.5/§6.6. **Landed**: no golden moved (the record-side goldens carry no usage, the usage goldens are event-side) and the store fixture carries no usage; the event-side `Usage` keeps its spelling because its goldens and the TS mirror pin it. `TurnUsage::from_record` is the one frozen read for records written before the change — two readers exist (`usage_of`, `subagent/inspect`), so the legacy shim lives on the record type, not in either caller |
| **S15** | The real-app E2E legs run configs the audit channel correctly calls unhealthy, so they have been red since #306 landed: neither leg's models declare a `context_window`/`max_tokens`, so a `gate_unavailable` record really is owed at open, and the stress leg's OM pass runs against a provider with no script for the OM prompt, so each turn ends with an `om_stalled` record and the assistant message is no longer the tail. Both break entry-count and render-range assertions that predate #306 (the stress fixture grew 10050 → 10052) | `app/wdio.conf.mjs` (`CANNED_CONFIG`, `makeWorkspaces`), `app/tests/e2e/stress.spec.mjs`, `app/tests/e2e/mock-multi.spec.mjs` | The legs assert a world where nothing is recorded while their configs describe a world where something is owed. Make the configs healthy — declare the window and the cap on the E2E models, and take OM out of the stress leg (it measures windowing and the §8 perf bar, not compaction) — so a healthy open mints nothing, the invariant `a_healthy_reopen_mints_nothing` already pins in Rust. A leg that wants the ungated case asserts the record it mints, by code. **Landed**: the E2E models carry a window and a cap and the OM thresholds sit past anything the fixture reaches, so the stress leg opens with one record — the `model_changed` baseline its pre-audit session never wrote — and mints no OM traffic. Its counts and render ranges are read from the store after open instead of the fixture's, and the audit records are named by code off the store |
| **S16** | A model id has two spellings and no resolution rule. The model menu sends `provider/key` (built in `ModelMenu.svelte` from `provider_list`); the config keys models by the bare `key` under a provider. `derive_turn` looks the session's id up with `provider.models.get(model)`, so after any menu switch the model's declared facts — `context_window`, `max_tokens`, `reasoning` — are dropped, the context gate goes Missing, and the audit channel records `gate_unavailable` for a model the config does describe. The same id goes into the request body (`ResponseRequest::new(self.model()…)`), so the endpoint is addressed as `dev/qwen3.8-…` instead of the model it hosts | `config/mod.rs` (one resolver over the provider maps), `harness/sessions.rs` `derive_turn`, `harness/launch.rs` (its default-model pick writes the bare spelling), `agent/turn/mod.rs` (the request's `model` field), `agent/turn/om_pass.rs` (the OM fallback had the same bug) | §6.7. Found from the legs: the mock leg's second open record is `gate_unavailable { model: "mock/mock-model-2", missing: [context_window, max_tokens] }` against a config declaring both. This is the incident family — switch model, lose the gate, run unbounded, get refused. **Landed**: `Config::model_key` is the resolver, stripping `provider/` only when the prefix names a configured provider, so a gateway key like `openai/gpt-5` passes through; the facts lookup and the thinking level read it, and the resolved key reaches the request as `TurnConfig.request_model` with `request_address` as the one fallback the turn and the OM pass share. The session keeps the id the user chose in its meta, its records, and the GUI |

## 5. Order of work

1. **S1, S2** — small, mechanical, no wire change. Done on the #306 branch,
   which owns the code they touch.
2. **S6, S5** — loop and store hygiene, no wire change. Done.
3. ~~**S16** — one model-id resolution rule~~ — **done**. The legs could not say
   something true without it: their `gate_unavailable` records were the audit
   channel correctly reporting a switch that dropped a model's facts (§4 S16,
   §6.7), and a spec that counted those records would have encoded the defect.
4. ~~**S15** — the real-app E2E legs~~ — **done**: the configs declare the window
   and the cap, OM is out of the stress leg, and the specs name the one baseline
   record a pre-audit fixture owes at open. Both legs green in both browsers.
5. **S14, then S13** — the usage record's shape first, because the anchored
   estimate reads the fields S14 names (S14 landed: the record carries the
   concepts, the adapter carries the spellings, `from_record` is the one frozen
   read). Wire-visible, so one reviewable series:
   the protocol type, the adapter decode, the single record reader, then the
   goldens + the store fixture + the TS mirror together.
6. **S3** — the `parking_lot` migration, alone in its own series: ~400 sites, no
   semantics, one commit per module group. It must never share a commit with a
   behavior change, and it is where hazard 2 dies.
7. **S9** — remove the supervisor seam from the production open path, then
   re-point its three test callers.
8. ~~Rename the `StreamEnd` collision~~ — done (§6.1). Land the
   `turn_cutoff`/`StreamOutcome`-on-`ProviderResult` ticket next.
9. **S4** (disperse `om_integration`) — four commits, each behaviour-neutral.
10. **S7, S8** — wire-visible; each needs goldens + the TS mirror + a protocol
   commit.
11. **The ADR**, then `tau-tokens` → `tau-provider` → `tau-session` →
    `tau-hashline`, each its own commit series, each with `lto = "thin"` in the
    same PR as the first split.
12. **S10, S11** last — dep additions are easy to review once the code is in the
    shape the plan wants. S3's dep is the exception: it lands with step 6.

## 6. Decided (2026-10-26)

1. **The record vocabulary was renamed, not the event.** `tau_protocol::StreamEnd`
   is now `StreamOutcome`; the TS mirror followed. `Event::StreamEnd` keeps its
   name — it is what the GUI subscribes to and its wire spelling is already
   pinned by goldens. The record type had no writers, so the rename cost four
   Rust files and two TS lines and changed no bytes on the wire.
2. **`parking_lot` is adopted** (decided 2026-10-26, against the first
   recommendation). What the rejection was protecting — the poison signal — was
   already being discarded at 419 of 423 sites, so std was selling a signal
   nobody read at the price of a process that stops forever after one bad turn.
   The crate makes the implicit rule explicit, deletes 217 `.expect("<invariant>")`
   clauses that assert something the code cannot guarantee, and buys 1-byte lock
   state with a single-atomic uncontended path. What is accepted in exchange: a
   struct mutated halfway through a panic is loadable, silently. Hence the two
   guardrails in S3 are requirements — untrusted text is parsed outside the
   critical section (that is where the panic would come from), and the pedantic
   `await_holding_lock` gate stays `-D warnings`. Worth remembering while hunting
   the "stopped" reports: `deadlock_detection` is an experimental feature flag,
   and a *hung* lock and a poisoned one look identical from the GUI.
3. **`tau-hashline` stays workspace-internal.** No crates.io means no semver
   policy, no separate CI job, and no public-API discipline past the crate
   boundary.
4. **No tokenizer — and the provider's number is the only recorded number.**
   Any token count that is stored or displayed comes from the provider's `usage`
   on the assistant record (`TurnUsage`, with its `prompt_tokens_details` cache
   split), never from `token_count`. The estimator has exactly one job: the
   decision taken before a response exists, the context gate and
   `clamp_max_output`. Every comparable harness draws the same line:

   - `pi` 1.4.0 (`core/compaction/compaction.js`): `calculateContextTokens` is
     `usage.totalTokens`, falling back to `input + output + cacheRead +
     cacheWrite`; `estimateContextTokens` takes the last assistant usage as the
     base and estimates only the messages *after* it; `shouldCompact` compares
     against `contextWindow - reserveTokens`.
   - Codex (`codex-rs/core/src/context_manager/history.rs:650`): the running
     total is the server-reported `last_token_usage.total_tokens` plus an
     estimate for items appended since that response, and the estimator's own
     comment calls it "a coarse lower bound, not a tokenizer-accurate count".
     Its constant is `APPROX_BYTES_PER_TOKEN = 4` — bytes, not characters —
     rounded up, and `with_serialization_allowance` adds 20% for JSON framing.
   - aider's docs: it "never enforces token limits", it reports the provider's,
     and its own counts are "estimates"; for the repo map it counts a *sample*
     with a real tokenizer and scales it to the whole.
   - An exact pre-flight count is something the provider sells, not something
     you ship: Anthropic `POST /v1/messages/count_tokens` (tools, images and
     documents included) and Gemini `count_tokens`. A per-adapter capability
     flag gets accuracy without a vocabulary, which is the real answer to the
     tokenizer question.

   So the gate's fix is anchoring, not regression-fitting: prompt estimate = the
   last recorded `usage.input_tokens` plus the estimate for whatever was
   appended after it, which in practice is one tool output. Bytes over four,
   rounded up, replaces characters over four — three times closer on CJK for
   free — and the estimate counts serialized tool arguments, not just content
   text. Tracked as S13. Sources: aider docs/troubleshooting/token-limits.html,
   ai.google.dev/gemini-api/docs/tokens, Anthropic count-tokens API reference.
5. **Durable records do not mirror an API shape.** `TurnUsage` today *is* the
   Responses-API usage object — `input_tokens` with a `prompt_tokens` alias, a
   nested `prompt_tokens_details`, a nested `output_tokens_details` — and
   `provider/stream.rs:64` deserializes the provider's JSON straight into the
   type that lands in the session file. The first non-Responses adapter
   (chat-completions, Anthropic messages, Gemini) would therefore be a format
   decision rather than a code decision. The record names the **concept**:
   input, output, and the cache split as a first-class field — Anthropic bills
   `cache_read_input_tokens` and `cache_creation_input_tokens` *separately from*
   `input_tokens`, and no OpenAI-shaped nesting says that. Per-API spellings go
   in the adapter: a private decode type per shape that maps into the record
   type. The event-side `Usage` (`tau-protocol/src/lib.rs:436-443`) already gets
   this right with a flat `cached_prompt_tokens`; the record side is what leaked.

6. **A struct carries what is read.** `OutputTokensDetails` exists only to be
   flattened by each reader, and its accessor `TurnUsage::reasoning_tokens()`
   has no production caller — two tests and nothing else. `total_tokens` is both
   stored and recomputable, the same two-writer defect ADR-0034 refused for a
   `note` sitting beside typed fields. So the cache count is a field, the unread
   details struct and its accessor go, and the total is an accessor with one
   owner. The rule is not "no unused fields": it is that a stored field needs a
   reader, and a derived quantity needs exactly one.

### 6.7 A model id is one string with one resolution rule

The session's model id is the qualified form the menu offers — `provider/key` —
because a workspace can reach several providers and the bare key does not say
which one serves it. The config keeps declaring models under a provider table
by bare key, because that is what a provider's model list is. So the id carries
both halves and exactly one function splits it: a resolver over the merged
config returning the provider, the key, and the declared facts. Everything that
reads a model's facts — the context gate, the output cap, the reasoning level —
and everything that addresses the provider, including the `model` field of the
request body, goes through that one read. A bare id (what the open writes when
it takes the provider's first model, and what every record predating this
decision carries) resolves against the provider the session was opened on.

Two spellings with no rule is how a model switch silently turns the gate off:
the switch stores `dev/qwen3.8-flash-next`, the lookup wants
`qwen3.8-flash-next`, and a miss is indistinguishable from an undeclared model.
The audit channel made it visible — `gate_unavailable` naming a model the
config describes — and the fix is the resolver, not a record that hushes the
symptom.

Out of scope: switching to a model on a *different* provider also needs the
session's provider handle re-resolved — it is bound at open from the first
provider, so a cross-provider switch today keeps calling the first. That stays
with the multi-provider seam; the resolver makes it a visible next step rather
than a second silent miss.
