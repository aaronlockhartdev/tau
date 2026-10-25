# Diagnostics are typed session records; `system` prose notes are retired and `tracing` stays a mirror capped at error

**Status**: accepted (2026-10-25) — from the "agents randomly stop" investigation; implementation is #306, with follow-on tickets for the provider `StreamEnd` and for semantic config validation + resolved Model facts.

## The gap

A four-hour debug log (`/tmp/tau.log`, 26 MB) showed eleven turns whose provider stream ended without a completion signal at 128,899–130,290 prompt tokens against the endpoint's 131,072-token window. Nothing in the product recorded that any of them happened: the assistant payload is `{text, reasoning, calls, usage, interrupted}` and `interrupted` is set **only** by a user force (`turn/mod.rs`), so a cut turn is indistinguishable from a clean stop in the durable record. The session's own OM state had gone dark for 3 h 10 m while the prompt grew — also unrecorded. And the model change that preceded the cluster (a config-level switch, applied at a restart ten hours after the file edit) was datable only from file mtimes: of the six affected sessions, **five carry zero `system` entries**, because a config-level change never passes through the one code path that writes a model note.

The two channels tau has today are both wrong for these facts. `tracing` is process-scoped and volatile: it goes to stderr, and release builds cap it at `error` (`app/src-tauri/Cargo.toml`: `tracing` with `release_max_level_error`) — so it is inert in the builds users run, gone when the process dies, blind to session branches, and unable to answer anything after a restart. The `KIND_SYSTEM` entry is the right *place*, but it is untyped prose: one kind carrying three unrelated meanings, payloads written as `{"note": <format! string>}` at six sites (`gate_hook.rs:57,68`, `turn/mod.rs:136,367`, `dispatch_sessions.rs:157,186`) and read back by **substring matching** — `gate.rs:184` does `note.contains("marked gate_failed")` to rebuild `gate_failed` on reopen, and `snapshot.rs:346-355` does `starts_with("model: ")` + `strip_prefix` to recover a closed session's model. `preview()` (`snapshot.rs:268-275`) reads a payload-key soup (`text | note | state | output | name`) across kinds. The seam already leaks: the GUI's decoder is `text = p.note ?? ''` (`app/svelte/lib/entries.ts:139`), so a system entry without a `note` — such as the #255 gate-blind-spot record, `{prompt, budget}` — renders as a **blank card**. The durable trace exists and the product hides it.

**Decision**: a diagnostic is a session record with a typed payload, emitted once per state transition, read back by a projection — never a log line. Release builds keep `tracing` at `error` (at most `warn` later), because nothing a user or the harness must ever learn is allowed to live only in `tracing`.

## Three channels, one home per fact

| Channel | Answers | Lifetime | In the model's context? |
| --- | --- | --- | --- |
| **Record** — a `diagnostic` session entry | what is (or was) true about this session and branch | forever, branch-scoped, GUI-rendered | no |
| **Live** — `LiveState` fields | what the running session is doing right now (gauges, counters) | this run | no |
| **Log** — `tracing` | what one process run did (timing, retries, wire I/O) | stderr; capped at `error` in release | no |

The rule that makes the split enforceable: **if the harness, the GUI, a closed session, or a user after a restart must be able to read it, it is a record.** Facts that would fire per-round belong to **Live** (the OM gauge) or **Log**; only the *transition* is a record. The record channel costs nothing at the only scarce resource: `is_raw` is `user | assistant | tool` (`om_integration/mod.rs:258`), so record entries never enter the prompt or an OM transcript and never count toward `pending_tokens` — the same structural-not-conventional guarantee ADR-0012 pins for `ui_detail`. The remaining cost is file size, bounded by emitting at transitions, not per iteration.

## The `diagnostic` kind, and typing the record

`KIND_SYSTEM` is retired as a payload contract; a new entry kind, `diagnostic`, has exactly one payload type, owned by `tau-protocol` (ADR-0006 is the one-seam rule, and the glossary's **Entry** already lists this typed-payload surface as clean-up item C9 — this is that item, applied).

```rust
// tau-protocol: the one owner of the shape, core and GUI both decode it.
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Diagnostic {
    ModelChanged { from: Option<String>, to: String },
    GateUnavailable { model: String, missing: Vec<String> }, // no facts → no gate (today silent)
    ContextOverflow { prompt: u32, budget: u32 },            // #255, today a note-less card
    GateAttempt { retries: u32, max: u32, output: String },
    GateFailed { retries: u32 },
    OmStalled { pending: u32, activation: u32 },             // today silent (om_pass.rs:188)
    ProviderRejected { status: u16, body: String },
    TurnCutoff { model: String, ended_by: StreamEnd, blocks: u32 }, // needs the StreamEnd decision
    #[serde(other)] Unknown { code: String },                // forward compatibility
}
```

- **Codes are append-only.** A variant is never renamed or removed; it is deprecated. Renumbering history is exactly what makes an incident undiagnosable across builds, since session files outlive the build that wrote them (ADR-0005).
- **Forward compatibility is structural**: `#[serde(other)] Unknown` (plus the TS mirror's `default:` case) so an older GUI renders an unknown code as its name instead of failing or blanking — ADR-0008's upsert-by-id already requires the transcript to tolerate entries it does not understand.
- **No stored display string.** Today's `note` is a `format!` string that both sides re-parse. The record carries the fields; the GUI composes the sentence. Storing prose duplicates the presentation decision into history and makes every reader a parser.

**Session state becomes a pure projection, not a set of scans.** The three prose readers (`gate.rs:175-187`, `snapshot.rs:327-355`, `preview()`) collapse into one function:

```rust
pub fn project_diagnostics(branch: &[Entry]) -> Diagnostics; // branch_entries(..) → fold
```

`gate.rs` and `snapshot.rs` consume it; there is one reader instead of three parsers. Legacy `{"note": …}` lines stay readable through a **frozen, tested shim** inside the projection (a closed list of the three historical formats, each with a test) — so an old session still yields its `gate_failed` and its current model, and the shim never grows, because new code cannot produce a `system` note.

The event/projection split is the point, not a style preference: the entry log is the store and session state is a read model derived at read time, branch-scoped. Diagnostics are append-only and never mutate, so there are no upserts, no tombstones, and no migration when a new code appears — ADR-0008's upsert-by-id stays reserved for genuinely mutable records like `KIND_OM`. That is also why the incident was unreadable: the harness had state derivations but no events to derive them from.

**Emission is mandatory and structural.** `Agent::diagnostic(&Diagnostic)` is the only writer: it appends the entry (always) and mirrors to `tracing` (optional, level by severity). The failure mode is forgetting to emit, so the turn loop's endings route through one place that emits whenever the outcome is not clean — the ADR-0012 precedent: the guarantee lives in the call graph, not in reviewer diligence. `append_entry(KIND_SYSTEM, …)` becomes unreachable for new code once the six sites migrate.

**The kind tags become an enum as well.** `Entry.kind` is a plain `String` (`session/entry.rs:36`, wire name `"type"`), so the nine `pub const KIND_*` are advisory rather than structural — and the core already ignores them where it hurts: `matches!(entry.kind.as_str(), "user" | "assistant" | "tool")` (`om_integration/mod.rs:259`) and bare `"om"`/`"user"`/`"tool"`/`"assistant"` literals (`:444,450`, `om_integration/turn_end/mod.rs:365`, `agent/config.rs:176`). A `&str` constant single-sources a spelling; it does not make the kind set a type, cannot be matched exhaustively, and cannot bind a payload shape. `tau-protocol` therefore owns one `EntryKind` enum — `#[serde(rename = …)]` per variant so the on-disk spellings are unchanged, and an unknown-kind fallback arm so a file written by a newer core still loads in an older one (ADR-0008 already requires the transcript to tolerate entries a build does not know). The TS side has half of this already: `protocol.ts:109-116` is a per-kind union whose last arm is exactly that escape (item C9). `is_raw` becomes `matches!(kind, User | Assistant | Tool)`, which closes the dangerous direction of drift — a new kind can no longer slip into the model's prompt by omission.

### Strings are not the enemy; re-derived facts are

This is not a ban on strings in `tau-core`. Model output, tool output, reasoning text, error `Display` messages, and the GUI's sentences are strings, and parsing model prose into a typed result is the correct shape — `plan/mod.rs:129-145` turns Markdown checkboxes into `Option<(bool, String)>`, `om/groups.rs:261-275` turns `## Group` headings into group records. A provider's error body is untrusted text, so `is_context_length_error` (`provider/mod.rs:68-70`) sniffing for `"context length"` / `"too long"` is a heuristic with no better source; what it must yield is a typed variant, not a bool the caller re-inspects. The forbidden shape is narrower and easy to name: **a fact that exists only as a string the harness wrote, recovered downstream by `contains` / `starts_with` / `strip_prefix`.** Strings are legitimate in exactly three roles — the wire spelling of a variant, text composed for a human at the edge, and opaque content the harness never branches on.

## Release logging stays error-only

`release_max_level_error` stands; lifting it to `warn` later is a display-policy choice, not a data-recovery one. This decision is what makes the cap safe: the durable channel no longer depends on the log channel. Debug builds keep `RUST_LOG=tau_core=debug` and the JSONL wire log (#98/#100), which stay where they belong — wire I/O is process telemetry, huge, and of interest only to a developer holding a live build.

## Considered

- **`tracing` only** — rejected: volatile, branch-blind, and compiled out in the builds users run; it cannot answer a question after a restart, which is precisely the question this incident asked.
- **A diagnostics sidecar or table** — rejected: a second store with its own durability and GC story, no branch semantics, and divergence from the session record; ADR-0005's per-session JSONL already gives append, CRC, archive, and the GUI stream in one place.
- **One upserted snapshot record, the `KIND_OM` pattern (ADR-0008)** — rejected: the incident's whole signal was the *sequence* (gate unavailable → OM stalled → prompt at the ceiling → cut turns). A snapshot record has one truth and no history; with both a record and events, the two diverge.
- **Keep `system`, add a `code` field** — rejected: `system` already means gate notice, model note, and rejection record, and its payload has no single owner; keeping it means the projection cannot filter by kind and old prose-shaped payloads stay permanently ambiguous.
- **Keep `note` beside the typed fields** — rejected: a derived field with two writers; the GUI renders from `code` + fields.
- **A `LiveState` diagnostics ring** — rejected: the durable entry already reaches the GUI live through ADR-0008's upsert, so a ring would be a second source of truth whose bound silently hides facts under the scroll-back. Gauges (OM pending tokens, age of the last cycle) belong in `LiveState` precisely because they are gauges, not facts.

## Consequences

The six `KIND_SYSTEM` write sites migrate; `gate.rs`'s and `snapshot.rs`'s scans are deleted (the compiler finds the call sites of the removed helpers); `preview()` gains a `diagnostic` case so the entry tree shows a one-line rendering instead of a blank card; the GUI decodes the new kind with a labelled card. Two decisions land as separate tickets and are dependencies of the last code above: the provider's **`StreamEnd`** enum (`completed | length-cap | idle-timeout | eof-without-done | mid-stream-error | killed`) replacing the single `completed: bool` on `ProviderResult`, and the `stop_reason` field on the assistant payload — without them the record can say *that* a turn was cut but not *why* the stream ended. A third: semantic **config validation** and resolved model facts, which is what makes `GateUnavailable` unreachable rather than merely recorded. Spec touchpoints: §4 (context gate, #210/#255), §12 (config, model facts), §8 (the snapshot carries previews, not payloads — so the projection stays bounded by the tail read, #302).
