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
   census across `tau-core`'s 423 `.lock()` calls — 428 in `tau-core` and 459
   across the three lock-bearing crates by S3's parent commit; 217 production
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
6. **The mirror's open arms hide mirror drift.** `EntryKind` and the diagnostic
   escape arm carry `(string & {})` so an unknown wire spelling still type-checks
   — correct for an open vocabulary, and it means the compiler can never catch a
   *typo in a declared arm*. `spawn_snapshot` for `spawn-snapshot` sat in the
   mirror through a green parity run for exactly this reason: the parity script
   compares declared members and the open arm absorbs the difference at every read
   site. For tag spellings the script, not `svelte-check`, is the gate.
7. **`TurnUsage::from_record` is a convention, not a type gate.** The frozen
   legacy read lives on the record type and both intended readers call it, while
   `harness/sessions.rs:376` decodes the same payload with
   `from_value::<AssistantPayload>` and would see `cached_input_tokens: None` on a
   pre-S14 record. That site reads tool calls today, so nothing is wrong yet. What
   makes it worth naming: S13 turns stored usage into a load-bearing read (the
   anchored estimate), and a second reader path appearing then would be a silent
   under-count rather than a wrong cache percentage.
8. **OM has no off switch, so configs reach for the ceiling.** `Om` is a required
   config section with no disable flag, and an empty `om_model` means *use the
   session's model*, not *off* — which is how the incident's ten doomed OM calls
   against an undeclared `qwen3.8-27b` happened, and why both E2E configs now set
   thresholds near the `u32` ceiling the validator allows. A config pressing a
   counter to its maximum to say "don't" is the symptom; S19 is the switch.

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
| **S3** | `std::sync::Mutex` → `parking_lot::Mutex`, and the `.expect("<invariant>")` clause disappears with the `Result`. Two guardrails come with it, as requirements: model-supplied text is parsed **outside** a critical section, and no guard is held across an await | 459 `.lock()` reads and 461 `.expect`/`.unwrap` clauses across `tau-core`, `tau-acp` and `tau-test`, over six commits. **Landed**: `tools::surface::lock_anchors`, its poison-recovery test, and the four `PoisonError` arms are gone; the OM cycle in `om_integration/background.rs` now reads a half-mutated `Inner` on faith where poisoning used to stop it; guardrail 1's one unmet site is accepted — see §6.2's shipped note | §6.2; hazard 2. A lock stops being a thing that can wedge the process, and the code stops asserting an invariant it cannot keep |
| **S4** | Disperse `om_integration` per §2, ending with the port replacing `Arc<Mutex<agent::Inner>>` | `om_integration/*` | hazard 1; also the prerequisite for `tau-om` if that is ever revisited |
| **S5** | Collapse the 7 `UNIX_EPOCH` conversions onto `jiff::Timestamp`, one `now_ms()` | `session/mod.rs:206`, `session/file.rs:315`, `harness/state.rs:531`, `hashline/pin_store/mod.rs:130`, `tools/impls.rs:393`, `om_integration/mod.rs:321` | already a dependency; two incompatible epoch idioms today |
| **S6** | `context_gate_budget` and `gate_unavailable_reasons` computed from one function returning `Result<budget, reasons>` | `agent/mod.rs:110,124` | they must never disagree — #306 had to reconcile exactly that drift by hand |
| **S7** | `ToolTarget` enum (`Subagent { name }` / `Core`) replacing the prefix test | `tools/surface.rs:97` | §1.2; the audit already recorded a bug this shape caused. `specs_for` also has no production caller — only `tools/tests_surface.rs` reads the role→specs table, and since S9 the root arm is shared (`Root | Seam`), so the mapping is pinned through `Seam` |
| **S8** | `ChildStatus` typed in `OwnState`, and a `ToolOutcome` enum for `ToolResult` | `tau-protocol/src/snapshot.rs:179`, `tools/mod.rs:39` | §1.2; both are wire-visible, so they need a protocol commit + TS mirror + goldens |
| **S9** | Remove the caller-supplied `Supervisor` from `SessionRole::Root` and the `root_with_supervisor` branch that exists for it; re-point the three test sites at the real open path | `harness/launch.rs` (the `root_with_supervisor` body, now deleted), `harness/tests_launch.rs`, `subagent/{testkit,tests_link,tests_gate}`, `tools/tests_surface.rs`, and `tau-test/tests_accept.rs:267,451,647` — **nine** seam call sites in five files, not the three this row named; production (`harness/sessions.rs`) always passed `None` | a test seam living in the production open path. **Landed**: `Root` is total — it carries `core`, `workspace`, `config` and the resolved `first_provider` as plain fields, so an under-specified root is a type error — and the `no providers configured` check `launch` duplicated now runs once, where the provider is resolved (`build_live`); the model check stays in the constructor, still pinned there. The seam did not vanish: the acceptance driver builds a `Supervisor` around a mock bridge, an in-process notice recorder, and a parent-specific prompt the mock scenario matches on, and none of that can come from a `Core`. It became `SessionRole::Seam`, a role with a `Supervisor` and a `Core`-less constructor, holding the branch `launch` used to branch on; the one line of that branch left unpinned by the move — replaying `gate_failed` from the record — was dropped (§6.10) |
| **S10** | `thiserror` on the error enums | 10 files | §3 |
| **S11** | `backon` for the OM retry policy | `om_integration/retry.rs` | §3 |
| **S12** | Typed payload per `EntryKind`: the `json!` writers and `payload.get` readers become the derived payload types | 12 files with `json!` (`tools/mod.rs`, `subagent/*`, `session/file.rs`, …), 83 key reads | §2.2; one shape per kind, so a reader cannot guess a key and each kind gains exactly one legal append path |
| **S13** | The prompt estimate anchors on the last recorded `usage.input_tokens`; only content appended after it is estimated. Bytes/4 rounded up, serialized tool specs included | `agent/turn/estimate.rs` (`PromptAnchor`, `prompt_size`, `fingerprint`), `provider/request.rs` (`prompt_token_estimate`, `items_token_estimate`), `om/mod.rs` (`token_count`) | §6.4, decided in §6.9. **Landed**: the provider's count is held beside the request it priced — the model that answered, its instructions, its serialized tool specs, its observation items as serialized, and the entry the request ended at — and the next call prices that count plus only the entries appended since, through the same converter assembly uses. A guard that does not match fails closed to the whole-prompt estimate; a charge past `u32::MAX` fails closed to the ceiling. The anchor lives in the turn, not the file: it describes a request, not durable state, and a turn's first call has nothing recorded to lean on |
| **S14** | The usage **record** stops mirroring one API: stored fields become the concepts (`input_tokens`, `output_tokens`, `cached_input_tokens`), the Responses/chat spellings move into the provider adapter, the total becomes an accessor instead of a stored field, and `OutputTokensDetails` + `TurnUsage::reasoning_tokens()` are deleted (no production reader) | `tau-protocol/src/payload.rs`, `provider/usage.rs` (the wire decode that used to land straight on the record type), `harness/snapshot.rs` `usage_of` (one of the two readers of a stored usage) | §6.5/§6.6. **Landed**: no golden moved (the record-side goldens carry no usage, the usage goldens are event-side) and the store fixture carries no usage; the event-side `Usage` keeps its spelling because its goldens and the TS mirror pin it. `TurnUsage::from_record` is the one frozen read for records written before the change — two readers exist (`usage_of`, `subagent/inspect`), so the legacy shim lives on the record type, not in either caller |
| **S15** | The real-app E2E legs run configs the audit channel correctly calls unhealthy, so they have been red since #306 landed: neither leg's models declare a `context_window`/`max_tokens`, so a `gate_unavailable` record really is owed at open, and the stress leg's OM pass runs against a provider with no script for the OM prompt, so each turn ends with an `om_stalled` record and the assistant message is no longer the tail. Both break entry-count and render-range assertions that predate #306 (the stress fixture grew 10050 → 10052) | `app/wdio.conf.mjs` (`CANNED_CONFIG`, `makeWorkspaces`), `app/tests/e2e/stress.spec.mjs`, `app/tests/e2e/mock-multi.spec.mjs` | The legs assert a world where nothing is recorded while their configs describe a world where something is owed. Make the configs healthy — declare the window and the cap on the E2E models, and take OM out of the stress leg (it measures windowing and the §8 perf bar, not compaction) — so a healthy open mints nothing, the invariant `a_model_that_declares_its_facts_records_no_gate_gap` pins at the gate and `a_prefixed_default_model_opens_on_the_model_it_names` now pins at reopen. A leg that wants the ungated case asserts the record it mints, by code. **Landed**: the E2E models carry a window and a cap and the OM thresholds sit past anything the fixture reaches, so the stress leg opens with one record — the `model_changed` baseline its pre-audit session never wrote — and mints no OM traffic. Its counts and render ranges are read from the store after open instead of the fixture's, and the audit records are named by code off the store |
| **S16** | A model id has two spellings and no resolution rule. The model menu sends `provider/key` (built in `ModelMenu.svelte` from `provider_list`); the config keys models by the bare `key` under a provider. `derive_turn` looks the session's id up with `provider.models.get(model)`, so after any menu switch the model's declared facts — `context_window`, `max_tokens`, `reasoning` — are dropped, the context gate goes Missing, and the audit channel records `gate_unavailable` for a model the config does describe. The same id goes into the request body (`ResponseRequest::new(self.model()…)`), so the endpoint is addressed as `dev/qwen3.8-…` instead of the model it hosts | `config/mod.rs` (one resolver over the provider maps), `harness/sessions.rs` `derive_turn`, `harness/launch.rs` (its default-model pick and its default-model lookup), `agent/turn/mod.rs` (the request's `model` field), `agent/turn/om_pass.rs` (the OM fallback had the same bug) | §6.7. Found from the legs: the mock leg's second open record is `gate_unavailable { model: "mock/mock-model-2", missing: [context_window, max_tokens] }` against a config declaring both. This is the incident family — switch model, lose the gate, run unbounded, get refused. **Landed**: `Config::model_key` is the resolver, stripping `provider/` only when the prefix names a configured provider, so a gateway key like `openai/gpt-5` passes through; the facts lookup, the thinking level and the launch default-model lookup all read it. The resolved pair reaches the request through `TurnConfig::request_address`, the one fallback the turn and the OM pass share. The session keeps the id the user chose in its meta, its records, and the GUI |
| **S17** | Bind the provider the id names, not the first one configured. The handle a session is born with — fresh or reopened — comes from `build_live`, and the live `set_model` re-derive picks the same way (`harness/dispatch_sessions.rs`): `config.providers.iter().next()` whatever the id's prefix says, so `b/k` reads `a.models` — with two *distinct* providers a declared model silently borrows another provider's window and cap. The same seam leaves the prefix ambiguous for a key whose own spelling starts with a provider name (`openai/gpt-5` under provider `openai`), and leaves `tau-acp`'s own facts read (`crates/tau-acp/src/config.rs:147,166`) resolving ids by its own second rule | `harness/sessions.rs` (`build_live`, `derive_turn`), `harness/dispatch_sessions.rs` (the `set_model` arm), `tau-acp/src/config.rs` | §6.7's deferral, and hazard: today the E2E workspace's two providers are one canned endpoint spelled twice, so nothing misbehaves visibly. Doing this means the resolver returns the provider too, which is what §6.7 declined until the multi-provider seam arrived |
| **S18** | A subagent child's model *facts* are its parent's. The child inherits the parent's `TurnConfig` verbatim (`harness/launch.rs` Child arm, `turn: d.turn`), so when an agent type overrides the child's model (`subagent/spawn.rs:159`, `ty.model`), the child runs with the parent's `context_window`, `max_output_tokens` and thinking level. §6.8 fixed the address half — the child addresses its own model — but the gate's facts still describe the parent | `subagent/mod.rs` (the Supervisor holds no `Config`), `harness/launch.rs` Child/restart arms | found while fixing S16's address: the fix is a re-derive against the child's model, which needs the config reachable from the spawn path — the same owner gap hazard 1 describes for OM |
| **S19** | Name the OM off switch. `Om` is a required section with no disable, and empty `om_model` means *the session's model* rather than *off*, so the only lever today is thresholds at the `u32` ceiling. And a configured `om_model` reaches the endpoint verbatim — `resolve_om_model` strips nothing — so `om_model = "dev/x"` asks the provider for a model named `dev/x`, the §6.7 miss again on the OM side (only its fallback path resolves) | `config/mod.rs` `Om`, `agent/turn/om_pass.rs` (`resolve_om_model`), `app/wdio.conf.mjs` (both legs press the ceiling) | hazard 8: the incident's ten doomed OM calls came from exactly this, and both E2E configs now say "don't" by maxing a counter. `enabled = false`, or empty `om_model` meaning off, is the honest version — the choice changes the config contract, so it is a decision, not a cleanup |

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
5. ~~**S14, then S13**~~ — **done**: the record carries the concepts, the
   adapter carries the spellings, `from_record` is the one frozen read, and the
   estimate reads a recorded count instead of guessing the whole prompt.
6. ~~**S3** — the `parking_lot` migration, alone in its own series~~ — **done**: ~460
   sites over six commits, one per module group, none sharing a commit with a
   behavior change. The wedge died with `lock_anchors` and its test; the one
   site still parsing model-supplied text under a guard is recorded in §6.2.
7. ~~**S9** — remove the supervisor seam from the production open path, then
   re-point its three test callers.~~ — **done** (§6.10). The row's count was
   wrong: nine call sites in five files, and the acceptance driver cannot ride the
   became total; the seam became its own role.
8. ~~Rename the `StreamEnd` collision~~ — done (§6.1). Land the
   `turn_cutoff`/`StreamOutcome`-on-`ProviderResult` ticket next.
9. **S4** (disperse `om_integration`) — four commits, each behaviour-neutral.
10. **S7, S8** — wire-visible; each needs goldens + the TS mirror + a protocol
   commit.
11. **S17, S18** — the two half-fixed seams S16 left: the provider a model id
    binds, and the facts a subagent child runs on. Both need the config reachable
    where the model is chosen, so they land after the seams §5's earlier steps have
    tidied, and S18 leans on hazard 1's owner question.
12. **The ADR**, then `tau-tokens` → `tau-provider` → `tau-session` →
    `tau-hashline`, each its own commit series, each with `lto = "thin"` in the
    same PR as the first split.
13. **S10, S11** last — dep additions are easy to review once the code is in the
    shape the plan wants. S3's dep is the exception: it lands with step 6.
14. **S19** whenever its decision is taken — it changes the config contract, so it
    is not a cleanup waiting in line; hazard 8 is the reason it is on the list at
    all.

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
   The crate makes the implicit rule explicit, deletes the 217 `.expect("<invariant>")`
   clauses (461 `.expect`/`.unwrap` clauses across all three crates by the time S3
   landed) that assert something the code cannot guarantee, and buys 1-byte lock
   state with a single-atomic uncontended path. What is accepted in exchange: a
   struct mutated halfway through a panic is loadable, silently. Hence the two
   guardrails in S3 are requirements — untrusted text is parsed outside the
   critical section (that is where the panic would come from), and the pedantic
   `await_holding_lock` gate stays `-D warnings`. Worth remembering while hunting
   the "stopped" reports: `deadlock_detection` is an experimental feature flag,
   and a *hung* lock and a poisoned one look identical from the GUI.
   Shipped (S3, measured at the tip): 459 `.lock()` reads and 461 deleted
   `.expect`/`.unwrap` clauses went across in six commits. The predicted cost never
   arrived — the guard is still `!Send`, because `send_guard` is an off-by-default
   feature — so std's spawn-boundary protection survived, and on top of it
   `await_holding_lock` fires on a `parking_lot` guard held across an await on the
   pinned toolchain. What did change: the migration deleted the one helper that
   existed only to absorb poisoning (`tools::surface::lock_anchors`), the test
   that pinned its recovery, and the four `PoisonError` arms. The three in
   `om_integration/background.rs` now read `Inner` unconditionally, so a panic
   under the inner lock leaves a half-mutated `Inner` that every later reader —
   the OM cycle included — takes on faith. That task already died on a panic (it
   is `tokio::spawn`ed and nothing catches unwinds); poisoning is what made every
   *later* acquirer die too, and that is the wedge hazard 2 was about.
   Guardrail 1 is not met everywhere, and that is **accepted**: `tools::surface.rs`
   still holds the anchor-`Registry` guard across `read`/`edit`, so model-supplied
   text is parsed
   inside a critical section there, because the single guard is what makes the
   read–resolve–patch–write sequence atomic against the served baseline; the
   alternative — parse and patch outside, guard only the registry — opens a window
   where the file and the registry diverge. What stands between that site and a
   wedged session is the hashline fuzz target's totality oracle, not the shape of
   the code; it stays the gate until a `read`/`edit` path actually panics.
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
   free — and the estimate counts the serialized tool specs, not just content
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

### 6.7 A model id is one string, resolved by one read (narrowed on review 2026-10-26)

The session's model id is the qualified form the menu offers — `provider/key` —
because a workspace can reach several providers and the bare key does not say
which one serves it. The config keeps declaring models under a provider table
both halves and exactly one read splits it, `Config::model_key`, returning the **key**
the provider declares — the provider is *not* part of the answer.
reads a model's facts — the context gate, the output cap, the reasoning level —
and everything that addresses the provider, including the `model` field of the
request body, goes through that one read. A bare id (what the open writes when
it takes the provider's first model, and what every record predating this
decision carries) resolves against the provider the session was opened on: v0
binds a session to one provider, so the prefix selects nothing today — the review
found the E2E workspace's two providers are the same canned endpoint spelled
twice, which is why the collision below is deferred rather than fixed.
Two spellings with no rule is how a model switch silently turns the gate off:
the switch stores `dev/qwen3.8-flash-next`, the lookup wants
`qwen3.8-flash-next`, and a miss is indistinguishable from an undeclared model.
The audit channel made it visible — `gate_unavailable` naming a model the
config describes — and the fix is the resolver, not a record that hushes the
symptom.

Two things this deliberately does **not** fix, both now visible instead of
silent. A switch to a model on a *different* provider keeps calling the first,
because the handle is bound at open (S17). And a model key whose own spelling
begins with a configured provider name — `openai/gpt-5` under provider `openai` —
is unreachable by its bare spelling, because the prefix is always stripped.

### 6.8 A resolved value carries what it was resolved for (taken 2026-10-26)

`Config::model_key` answers for an id, and its answer was being stored on the turn
as a bare key. A subagent child inherits its parent's turn config and an agent
type can override the child's model, so the parent's key went out naming the
child's model — the same silent-miss class, now at a different seam, and no test
saw it because the fixture parent used a bare id. The stored value is now a pair,
`ModelAddress { id, key }`, and `TurnConfig::request_address` applies it only
where the ids agree. The rule generalises: when a derived value is stored beside
the thing it was derived from, it carries that thing's identity, so a stale copy
cannot be mistaken for a current one at the read. Fixing it at the read rather
than at each write is what makes the third seam (a model change whose config
re-derive never ran) fall out of the same rule.

### 6.9 The prompt estimate anchors on the request, not the conversation (taken 2026-10-26)

The gate sized a prompt by guessing it: the shared estimator read every byte of
the conversation, on every call, in every turn. The provider already reports the
exact count for the request it answered — `input_tokens`, the field S14 made a
concept — so the turn keeps that number beside the parts of the assembly it
priced (the model that answered, the instructions, the serialized tool specs,
the observation items as serialized, and the entry the request ended at) and
prices the next call from that count plus only the entries appended since,
through the same converter assembly uses.
The estimate walks toward the truth across a turn instead of restarting from a
guess each call.

The anchor lives in the turn, not the session file: it describes a request, and
a turn's first call has nothing recorded to lean on. Every guard fails closed —
a mismatch drops to the whole-prompt estimate rather than pricing a prompt the
request never was. A compaction that consumed the anchored entries, a
re-injected plan state, a changed tool surface, a model switched from the GUI
mid-turn, a reflector that rewrote the observation log in place, a branch
without the entry: each has its own test, and each guard was removed in turn to
confirm the test stays load-bearing. The log guard reads content, not a count —
a reflector is allowed to compress the log without adding or removing an item,
and a count would then describe bytes the request never carried. When a call
reports no usage nothing is fabricated; the estimate stays unanchored. A charge
too large for `u32` reads as the ceiling: a prompt over every declared window,
never a fresh and much smaller guess.

Two base-rule changes come with it, both from §6.4: `token_count` counts bytes
over four rounded up — it was characters over four truncated, so a CJK run
counted at a third of its size and any short string at zero — and the
whole-prompt estimate carries a 20 % allowance for the JSON framing a content
estimate never sees (Codex's `estimate_request_tokens` plus a fifth). The
allowance rides the estimated parts only; a provider's own count already
includes the framing. OM thresholds shift with the estimator, which is intended:
they are soft counters measured against the same yardstick, and no record or
wire shape moves.

Not measured: the bytes-over-four rule raises the OM counters (observation,
pending, buffered chunks — four times for a 4-byte code point, three for CJK),
and the shipped thresholds were tuned against the old characters-over-four
yardstick. The first dogfood that compaction-tunes moves them; the defaults
themselves are pinned by tests, so a retune is a deliberate edit, not a drift.

What this closes is §6.4's failure mode: an under-estimate lets a prompt past the
window the model really has, the provider rejects the request, and the agent
stops mid-task.

### 6.10 A constructor takes what it needs, or its caller names the role it plays (taken 2026-10-26)

`SessionRole::Root` asked for eight fields and seven were `Option`; production
filled five and left `supervisor` and `system_prompt` `None`. Those two were the
seam: the constructor carried a branch whose doc comment named its caller — "the
acceptance driver's seam" — which handed in a pre-built `Supervisor` and skipped
model resolution, config files, the skills catalogue, the bridge, the event
wiring, and the open audit record. Every root built through it was a root the app
never opens, and its two checks — `no providers configured`, `provider x has no
never opens, and its `no providers configured` check duplicated the one
`build_live` runs where the provider is resolved.

The plan's fix — re-point the seam's callers at the real open path — was written
from a count of three call sites; the real count was nine, in five files — three
in `tests_launch`, three in the acceptance driver's subagent, reclaim, and
sender-labels suites, one each in subagent's `testkit`, `tests_gate` and
`tests_link`. A sixth file, `tools/tests_surface.rs`, built a core-less `Root` to
read the tool table and now builds a `Seam`. The driver's three hand the
`Supervisor` a mock bridge, an in-process recorder of reclaim and adopt notices,
and a parent system prompt that differs from the supervisor's own because the
mock-LLM scenario is chosen by matching request content (ADR-0010). None of that
is reachable from a `Core`: `Core::child_factory` is injectable, `Core`'s bridge
is not, and the driver asserts on notices it collects itself, not on protocol
events. Re-pointing those suites would have rewritten what they prove.

So the constructor got honest instead of smaller. `Root` is total: the four
values production resolves — the core, the workspace, the merged config, the first
provider and its name — are plain fields, and an under-specified root does not
compile. The fake got a name of its own, `SessionRole::Seam`: a role that declares
it supplies its own supervisor and provider and asks for no core. Deleting it
would have cost the deterministic-LLM contract; faking it as a `Root` with four
fabricated `expect()`s would have had a test invent a provider and a workspace to
prove something about prompt wiring, which is how a fixture starts lying.

What moved as a result is the part worth keeping. The provider check `build_live`
duplicated now fires where the provider is resolved, so the pin for `no providers
configured` sits on `session/new`, the call a user actually makes. The
`gate_failed` re-open test reads its gate from a config file the way the app
layers one, instead of a `Core` with the gate bolted on. The role→tool-spec table
is pinned through `Seam`, the root-shaped role that builds without a core.

One line of the branch did not move. It read the session's records to restore
`gate_failed` on a re-open, and the test that exercised it now drives the real
path. Nothing exercises it on the seam, and nothing can: the seam's check rides
the supervisor it adopts, and no fixture gives one a command, so its parent never
ends in that state. The line went rather than stayed untested — a fixture role
carries what its fixtures need, and a re-open that finds a failed check is what a
real open does.
