# Typed-facts surface audit: where a closed set is a string, a bool pair, or a comment

Method: pattern sweep over `crates/*/src` (28.8k LOC, tests excluded) for the shape ADR-0034 forbids — *a fact that exists only as a string the harness wrote, recovered downstream by `contains` / `starts_with` / `strip_prefix`* — plus its two siblings: a closed set encoded as a `String`/`bool` instead of an enum, and a sum type flattened into a struct of strings at a boundary. Each hit is classified: fix, or leave (see **Where the rule does not apply**).

## What is already right (and should be the template)

`tau-protocol` already contains the correct pattern in most of its surface: `Event`/`SubagentEventKind`/`SessionEventKind`/`SystemEventKind`/`OmStatusKind` (`events.rs`), `EntryStatus` (`snapshot.rs:67`), `TurnState`, `MessageLane`, `ContextMode`, `FileGitState`, `ProtocolError`, `CommandOutput` and `Command`, and `payload.rs` — the C9 payload surface with one typed payload per entry kind (`UserPayload`, `AssistantPayload`, `ToolPayload`, `SpawnSnapshotPayload`, `ToolOutput`), whose module doc already claims "the writers construct … never from raw maps". `subagent/states.rs` has a genuinely good sum type (`ChildState` with per-variant payloads). The audit's value is therefore narrow: **the types exist; the boundaries leak.**

## Class 1 — a sum type flattened into strings at the protocol boundary

The worst instances are not in the core, they are in `tau-protocol`, where a working enum is serialized as loose strings and the illegal states become reachable.

**`OwnState` (`snapshot.rs:179-185`) is a `ChildState` turned into two strings.** `ChildState` (`subagent/states.rs:66`) is `Running | Idle { waiting_on } | Awaiting | Done { output } | Failed { reason } | Stopped { by }`. `own_state_for` (`subagent/snapshot.rs:101-111`) then emits `state: state.kind().to_owned(), waiting_on: Some(waiting_on.as_str().to_owned())` — and `OwnState` declares its legal values in a **doc comment** (`/// running | idle | awaiting | done | failed | stopped`). Cost: the closed set is unenforced on both sides of the seam; the variant payloads (`Done.output`, `Failed.reason`, `Stopped.by`) are dropped at projection time because there is no field for them; and `state: "done", waiting_on: "user"` type-checks. Fix: `enum StateView { Running, Idle { waiting_on: WaitingOn }, Awaiting, Done { output }, Failed { reason }, Stopped { by: StoppedBy } }` — one enum, serde-tagged, TS mirror is a discriminated union (`protocol.ts` already does this for entry kinds).

**`ActivityEntry.role: String` (`payload.rs:157-158`)** — the closed set is in a comment (`/// "user" | "assistant" | "tool"`). Same for `ActivityEntry.status: Option<String>` (the tool verdict, already an enum-shaped string) and `ActivityGroup.kind: Option<String>` (`payload.rs:147`, comment: "marks Reflector rewrites"). Fix: reuse the `EntryKind` enum from #306 step 8 and the tool-verdict enum; `ActivityGroup.kind` becomes `Option<GroupKind>` (`Original | Reflected`).

**`ImageBlock.kind: String` (`payload.rs:100-101`, `#[serde(rename = "type")]`)** — a closed set of exactly one value (`"image"`), i.e. a tagged union written as a struct plus a literal. Fix: `enum ContentBlock { Text { … }, Image(ImageBlock) }`, which is also what the provider request side needs.

**`ToolResult { ok: bool, cancelled: bool }` (`tools/mod.rs:40-43`)** — three states as two bools, with a derived `&str` status beside them, while the TS mirror already models it as `'ok' | 'running' | 'error' | 'cancelled'` (`protocol.ts:114`). `ok: true, cancelled: true` compiles. Fix: one `ToolStatus` enum field; delete the bools and the stringifier.

**`ProviderResult.completed: bool` (`provider/mod.rs:394`)** and **`SseParser.terminated: bool` (`:139`)** — six distinct endings collapsed to one bit; the `[DONE]` sentinel is then re-detected by string compare. Fixed by the `StreamEnd` dependency of #306; the parser should return `SseEvent::Delta | Completed | Done` instead of setting a flag.

## Class 2 — the core writes a sentence, then parses it back

- **`harness/snapshot.rs:320-325` → `:346-355`**: `model_note` formats `"model: A → B"`; `last_model_note` recovers the model with `starts_with("model: ")` + `strip_prefix` + `split`. Migrating with #306.
- **`gate.rs:110-120` → `:184`**: `failed_note`/`failure_note` format the gate verdict into prose; `gate.rs` then rebuilds `gate_failed` with `note.contains("marked gate_failed")`. Renaming the sentence silently deletes the state. Migrating with #306.
- **`agent/turn/mod.rs:136`**: writes `{prompt, budget}` under `KIND_SYSTEM` — a payload that does **not** match `SystemPayload { note: String }`, i.e. the writer already bypasses the C9 type, and the GUI renders it blank (`entries.ts:139`). Migrating with #306.
- **`om/mod.rs:301` → `:305`**: `format!("--- message boundary ({iso}) ---")` is emitted into the transcript and re-detected with `starts_with(…) && ends_with(" ---")`. Here the marker legitimately has to be text (the model sees it), but it must be *rendered from* structured state — the boundary's timestamp and owning entry id belong to the OM record — not be the only place the fact exists.
- **`om/groups.rs:188` → `:261-275`**: the writer emits `"## Group `{id}`\n_range: `{r}`_\n\n{body}"` and the reader re-extracts id and range with `strip_prefix`/`split_once`. Same fix: keep `Group { id, range, body }`, render the Markdown for the prompt, and never re-parse what was rendered.
- **`tools/surface.rs:97`**: `name if name.starts_with("subagent_")` — "this call targets a sub-agent" is a fact recovered from a name prefix; the comment at `:88-90` documents a bug this exact shape caused (every core tool swallowed into `route_parent`). Fix: parse the call name once into `ToolTarget::Core(name) | ToolTarget::Subagent(name)` at the boundary and dispatch on the enum.
- **`provider/mod.rs:170-172`**: `strip_prefix("data:")` — leave, that is the SSE wire format, not our prose.

## Class 3 — Option and bool soup where the set is closed

- **Model facts (`config/mod.rs:49,51`) and `TurnConfig` (`agent/mod.rs:85,95,98`)**: `context_window: Option<u32>`, `max_output_tokens: Option<u64>` — `None` means "the safety valve does not exist", which is nameless and silent; the context gate (`turn/mod.rs:107-112`) degrades to no gate. This is the incident's root cause. Fix (follow-on ticket): resolve once into a total `ModelFacts` with a `source: FactSource` (`Declared | Assumed`), so the gate always has a budget or `derive_turn` returns `ModelFactsMissing`. Distinguish from genuinely optional limits — `image_max_bytes`/`image_max_pixels` (`tools/surface.rs:62-64`), `max_request_bytes` (`config/mod.rs:341`) — where `None` means "no limit" and that is honest.
- **`ProviderError.mid_stream_errors: Vec<String>` (`provider/mod.rs:401`, pushed at `stream.rs:118`)**: errors as strings, consulted by exactly one consumer (`om_integration/turn_end/commit.rs:84`) and by the agent loop never at all. Fix: `Vec<StreamEnd>`-adjacent typed error, surfaced by the `turn_cutoff` diagnostic.
- **`hashline/mod.rs:390,429-440`**: `hashes: Vec<String>` for a 4-character code over a fixed 52-char alphabet (ADR-0030) — the alphabet is a type the code does not have; `warnings: Vec<String>` and `removed_lines: Vec<String>` are likewise untyped. Fix: `LineHash` newtype (fixed-size, parse-checked) and a `EditWarning` enum — a warning is a diagnostic, so it belongs on the #306 vocabulary.
- **`degenerate: bool` duplicated** (`om/observer.rs:377`, `om/reflector.rs:16`) and **`prefix_demoted: bool`** (`om/mod.rs:227`), **`gate_failed: bool`** (`agent/mod.rs:229`): durable state kept as independent flags rather than derived. `gate_failed` becomes a projection read (#306 step 6); the OM ones deserve a small `enum PassQuality { Ok, Degenerate(Reason) }` shared by both passes rather than two booleans that can disagree.
- **`Result<String, String>` on the tool surface** (12 sites, e.g. `tools/surface.rs` via `ToolResult::from_result`, `tools/mod.rs:103-108`): the tool's own failure taxonomy is erased before the dispatcher sees it, so the dispatcher cannot distinguish "not found" from "denied" from "bad arguments" without reading text. Fix: `ToolError` enum (Display still lowercase-concise per the Rust conventions) with the message as a field.

## Class 4 — raw `json!` writers bypass the payload types

The writer surface is literally untyped: `append_entry(&self, kind: &str, payload: Value)` (`agent/config.rs:114`), 38 call sites, and only one of them passes a `payload.rs` type on the line where the payload is built. Raw `serde_json::json!` appears **65 times outside tests** (23 files), concentrated in `subagent/snapshot.rs` (12), `tools/mod.rs` (10), `subagent/inspect.rs` (9), `subagent/child.rs` (6), `harness/forwarding.rs` (4) — e.g. `snapshot.rs:186` `json!({ "event": "state", "state": state })`, the write side of Class 1's flattening. Readers show the same shape from the other end: `payload.get(…)` at **25 sites**, led by `harness/snapshot.rs` (8, including `preview()`'s `text | note | state | output | name` key soup), `agent/turn/assemble.rs` (5), `subagent/restore.rs`/`inspect.rs` (2 each), `harness/turn.rs` (2), `tau-acp/src/mapper.rs` (2). Fix: `append_entry(kind: EntryKind, payload: impl Serialize)`, so the kind and the payload type are checked at the call site and a `json!` becomes a visible exception; `preview()` gets per-kind arms instead of key probing.

## Class 5 — the TS mirror drifts because nothing pins it

`app/svelte/lib/protocol.ts` hand-mirrors `payload.rs`: entry payloads as a per-kind union (`:112-116`) — good shape, but written by hand, and where a Rust field is a `String` the TS side has no way to be narrower (`kind: string` at `:68,86`). Enforcement today is `crates/tau-protocol/tests/wire_goldens.rs` (insta snapshots of the emitted JSON for "each principal type") plus `protocol-parity.mjs`, which by its own doc comment checks *command names* only. So the field-level shape of `OwnState`, `ChildActivity`, `ActivityEntry` and friends is unpinned, and a Rust rename reaches the GUI as a silently-`undefined` field. Fix: one golden per payload/protocol type (cheap, mechanical), and the `Unknown`/`default:` arm on every tagged union so an unknown variant renders its name rather than nothing — the same forward-compat rule #306 sets for diagnostic codes.

## Where the rule does not apply

Deliberately left alone, so the audit does not become a licence to enum-ify everything: Markdown plan checkboxes (`plan/mod.rs:129-145` → `Option<(bool, String)>`), observer/reflector bullet parsing (`om/observer.rs:359`, `om/reflector.rs:86-93`), provider error-body sniffing (`provider/mod.rs:68-70`), SSE framing (`provider/mod.rs:170`), and every `Display` message. All of these parse *someone else's* text at a boundary and return a typed result — that is the correct shape, and the model's prose is genuinely a string. The test stays narrow: does the harness parse text **that the harness itself wrote**, to recover a fact? If yes, it is a bug.

## Rules worth keeping as conventions

1. A closed set is an enum, never a `String`, a `bool` pair, or a doc comment listing the legal values.
2. A sum type crosses the seam as a tagged union — do not flatten its discriminant into fields and drop its payloads.
3. Every tagged union has an unknown-value arm on both sides (serde `#[serde(other)]`, TS `default:`).
4. Render, don't store: structured state is the source of truth; any prose the model must see is generated from it and never re-parsed.
5. `Option<T>` on a load-bearing number means "the guardrail is off" and must be named as such (total resolution, or an explicit `NoLimit` variant).
6. Facts written into a session go through a typed payload type; `json!` at a writer is a visible exception, not the idiom.
7. Identity is a newtype when mixing two ids would be a silent bug (`SessionId` vs `EntryId` vs `GroupId`'s 16-hex handle vs a hashline's 4-char `LineHash`).

## Suggested ticket split

| Ticket | Classes | Blast radius |
| --- | --- | --- |
| #306 (open) | 2 (three sites), 4 (`gate_failed` derivation), 5 | protocol + core + GUI |
| `StreamEnd` + `stop_reason` + `SseEvent` | 1, 3 | provider, turn loop, GUI badge |
| Model facts resolved total + semantic config validation | 3 | config, harness, GUI warning |
| Protocol de-flattening: `StateView`, `ActivityEntry`/`ActivityGroup`, `ContentBlock`, `ToolStatus` | 1 | tau-protocol, subagent projections, TS mirror |
| Render-don't-store: OM group headings and boundary markers | 2 | om_integration, session fixtures |
| Tool surface typing: `ToolTarget`, `ToolError`, `append(impl Serialize)`, `preview()` arms | 2, 4 | tools, harness, ACP mapper |
| Wire goldens per protocol type + unknown-variant arms | 5 | tests, TS mirror |
