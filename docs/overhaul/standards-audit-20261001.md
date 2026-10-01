# Standards audit — 2026-10-01 (post maximum-strictness lint waves)

- **Date:** 2026-10-01
- **Scope:** pre-existing code vs AGENTS.md (Coding conventions, Best practices, Testing); dead/orphaned code; comment discipline + stale pointers.
- **Method:** sweep 1 = rule-by-rule greps over the listed rule set; sweep 2 = bounded orphan-class checks, 2-check cap per reference question; sweep 3 = sampled comment review in largest files per crate + stale-token greps over living docs only (README, CONTEXT.md, AGENTS.md, docs/spec/, docs/adr/, .github/workflows/, justfile, crate Cargo.tomls, source doc comments). docs/overhaul/ and docs/research/ are archived records — exempt.
- **Read-only** on all source; this file is the only write.
- Severities: `fix-now` (clear violation, small fix) / `ticket` (real issue, needs a ticket + judgment) / `accept` (deliberate, documented, or not a true violation) / `verify` (ambiguous under the 2-check cap; exact check named).

## Sweep 1 — standards adherence (pre-existing code)

### Rust

**fix-now**

- `crates/tau-core/src/hashline.rs:176` — `pub struct Edit` (all-`String` fields) has **no `#[derive]` at all**: missing `Debug` (and the equally applicable `Clone`, `PartialEq`, `Eq`, `Default`) on a tau-core public API type [fix-now]
- `crates/tau-core/src/subagent.rs:122,135,150` — `StateNotice`, `SpawnNotice`, `WakeNotice` derive only `Clone`; all fields are `Debug`/`PartialEq`-compatible (`String`, `ChildState`, `WaitingOn`, `WakeKind`, `ContextMode`, `ResumeContract`, `serde_json::Value`), so `Debug` + `PartialEq` apply and are missing [fix-now]
- `crates/tau-core/src/hashline.rs:55` — `EditError` Display message ends with a trailing `.` ("…copy the fresh 3-char anchors (the 3 chars before {SEP}).") — violates *no trailing punctuation*; also verbose imperative prose where a short factual message is the rule [fix-now]

**verify**

- `crates/tau-core/src/om_integration.rs:42` — `OmState` derives only `Clone`; likely legitimate if it holds a non-`Debug` sink/handle. Exact check: `grep -n -A 12 'pub struct OmState' crates/tau-core/src/om_integration.rs` — if every field is `Debug`, add `Debug` [verify]
- `crates/tau-core/src/harness/launch.rs:22` — `SessionRole` derives only `Clone`. Exact check: `sed -n '18,30p' crates/tau-core/src/harness/launch.rs` — small enum, almost certainly should gain `Debug, PartialEq, Eq` [verify]

**accept**

- `crates/tau-protocol/src/payload.rs:303,378` — `to_value` is correct `to_*` usage (computing/allocating a serde `Value`), not a conversion-naming violation [accept]
- `crates/tau-core/src/harness/state.rs:60,105` — `Core` / `CoreBuilder` have no derives: handle/builder types whose internals are not `Debug`-by-construction; `Debug` does not apply [accept]
- All 51 `pub struct`/`pub enum` in `tau-protocol` (payload, snapshot, lib, events) derive `Debug, Clone, PartialEq, Serialize, Deserialize` (+`Eq`/`Default`/`Copy` where applicable) — public-surface trait rule satisfied [accept]
- `fn get_*`: zero matches workspace-wide; `Result<T, ()>`: zero matches workspace-wide — both rules clean [accept]
- All 8 `impl fmt::Display` in the workspace are lowercase, concise, no trailing punctuation — except the one `fix-now` above [accept]

### Tauri (app/src-tauri)

All nine rules compliant — no findings.

1. CSP: set and restrictive (`default-src 'self'`, `object-src 'none'`, `base-uri 'self'`, no wildcards) in `tauri.conf.json:26` [accept]
2. State/business logic in core: the shell wraps `tau_core::harness::Core` (`lib.rs:14`); frontend is a thin renderer [accept]
3. Commands async: the single `#[tauri::command]` (`lib.rs:30`) is `async fn` and moves dispatch to `spawn_blocking` [accept]
4. Tagged `ProtocolError` at IPC: `tau_command -> Result<CommandOutput, ProtocolError>` [accept]
5. Coalesced events: `pump_events` emits a `batch` on `tau://event` (`main.rs:139-141`) [accept]
6. Capabilities: individual `allow-*` grants with a reason in the file (`capabilities/default.json` description documents the whole surface) [accept]
7. Command-set surface: exactly one dispatch command [accept]
8. Window creation: no `WebviewWindowBuilder`/`WindowBuilder` in the crate (single-window v0) [accept]
9. Typed boundary: no `serde_json::Value` in `app/src-tauri/src`; `withGlobalTauri: true` kept per the documented tauri-pilot exception [accept]

### Svelte / TS (app/svelte)

- `$effect` (13 sites): all are outside-world (title, scroll/`scrollIntoView`, document listeners, network fetches) or deliberately documented (pin re-arm `Transcript.svelte:148`, per-entry override merge `EntryCard.svelte:60`, rename-input seed+focus `SessionNode.svelte:92`). No state-synchronisation misuse found [accept]
- `<slot>`: zero usages — snippets rule clean [accept]
- `$props()`: all 6 destructures type-annotated [accept]
- `any`: exactly one, at the documented virtua bind site `Transcript.svelte:181` [accept]

## Sweep 2 — dead / orphaned code

**fix-now**

- `app/src-tauri/Cargo.toml:[target.'cfg(unix)'.dependencies]` — `libc = "0.2"` is never referenced anywhere in the crate (zero `libc` matches in `app/src-tauri/src`) — unused dependency [fix-now]

**verify**

- `fixtures/e2e-mocks/acceptance-{tools,om,om-observer,om-reflector,subagent-parent,subagent-child}.json` (6 files) — not referenced **by name** from any e2e spec, wdio config, justfile, or CI workflow (only the three `e2e-*.json` scenarios are named). The mock server loads the whole directory and `ScenarioSet::select` matches by request pattern, so they may still be selected dynamically. Exact check: read the `pattern` field of each of the six files, then grep those patterns in `app/tests/` and `app/wdio.conf.mjs` — a scenario whose pattern no spec request can match is dead [verify]

**accept**

- pub items: all sampled workspace-wide have call sites — tau-protocol 5/5 (`to_value`, `from_value`, `resume_contract`, `to_value` ×2, `reasoning_tokens` — the last is a public protocol accessor exercised by tests, normal for a library API); tau-core 12/12 sampled (`build_reflector_prompt_frozen`, `bare_specs`, `task_child_refusal`, `route_parent`, `new_session_id`, `set_entry_event_hook`, `combine_group_ranges`, `derive_group_provenance`, `reconcile_groups_from_reflection`, …); tau-app surface (`CoreState`, `tau_command`) wired in `main.rs`. Unsampled pub items in `pub mod` paths are crate API (rustc cannot dead-code them); pub items in private submodules would trip `clippy -D warnings` in CI, so the residual risk is low [accept]
- orphan files: none — every `.rs` file maps to a `mod` declaration or is a Cargo-convention root/test file (`lib.rs`, `main.rs`, `tests/*.rs`) [accept]
- dead constants: 13 sampled across tau-core/tau-mock-llm (all six `om/observer.rs` prompt constants, `REFLECTOR_PROMPT_TEMPLATE`, `COMPRESSION_GUIDANCE`, `CANNED_SCHEME`, `IDLE_ACTIVATION_SECS`, `NUDGE`, `MODEL_ID`, …) — every one has production or test call sites [accept]
- commented-out code: the `grep '^[[:space:]]*//...(' ` heuristic returns only ordinary prose comments (parenthetical *why* comments); no commented-out code blocks found [accept]
- `cargo machete`: **skipped (cargo-machete not installed; `cargo +nightly machete` → "no such command")** — compensated by the per-dep manual checks below [accept]
- dependencies: every dependency of tau-core (12), tau-protocol (2), tau-mock-llm (3), tau-test (5) verified referenced in source; app deps verified except `libc` above [accept]
- fixture files: `fixtures/references/mastra-om/*.ts` referenced by the verbatim-parity tests (`om/tests_observer.rs`, `tests_reflector.rs`); `fixtures/sessions/*.jsonl` referenced by `app/wdio.conf.mjs` + `replay.spec.mjs` + the fuzz corpus seeds — all in use [accept]
- `tau-test` and `tau-mock-llm` crates: both driven by the justfile (`./target/release/tau-test …`, `./target/release/tau-mock-llm --port … --scenarios fixtures/e2e-mocks`) and the wdio config — not orphaned [accept]
## Sweep 3 — comment discipline + stale pointers

**ticket**

- `docs/spec/v0.md:196` — stale pointer: "the A/B/C variants are preserved on branch `prototype/gui-ia-variants-a-c`" — that branch does not exist locally or on any remote (`git branch -a`). The spec is the approved v0 contract, so rewording is a spec-owner judgment, not an audit fix [ticket]
- (observation, from the largest-file sampling) `crates/tau-core/src/session.rs` (908 LOC) and `crates/tau-protocol/src/payload.rs` (785 LOC) are over the 500-LOC soft limit — not the 1000-LOC hard blocker, but the soft limit is "a review smell that invites a split" [ticket]

**accept**

- stale tokens in living docs (README, CONTEXT.md, AGENTS.md, docs/spec/, docs/adr/, .github/workflows/, justfile, crate Cargo.tomls): the six-token grep (`dogfood/`, `third_party/`, `test/fixtures`, `tau-acceptance`, `prototype/`, `dev/config`) returns exactly the one finding above; docs/overhaul/ and docs/research/ exempt as archived [accept]
- stale tokens in source doc comments: zero matches across all crates [accept]
- comment discipline, sampled in the largest file per crate — `session.rs` (908), `payload.rs` (785), `EntryCard.svelte` (617), `scenario.rs` (338), `main.rs` (161): every comment is a *why* (spec §/ADR citations, non-obvious invariants, constraint explanations); no what-narration, no name-in-prose restatements, no change narration, no banners [accept]
- mechanical banner/section-divider check (`// ====`, `// ----`, numbered markers) over all Rust/Svelte/TS source: zero true matches (one false positive: an example URL in a doc comment) [accept]

## Summary

| sweep | fix-now | ticket | accept | verify |
|---|---|---|---|---|
| 1 | 3 | 0 | 18 | 2 |
| 2 | 1 | 0 | 8 | 1 |
| 3 | 0 | 2 | 4 | 0 |

## Recommended ticket breakdown

1. **tau-core derive gaps** (sweep 1, fix-now ×3, one diff): add the applicable derives to `hashline::Edit` (`Debug, Clone, PartialEq, Eq, Default`), `StateNotice`/`SpawnNotice`/`WakeNotice` (`Debug, PartialEq`), and fix the `EditError` Display message (drop the trailing period and the imperative prose). Two `verify` items resolve inside the same ticket: `OmState` and `SessionRole` (each needs a 1-line field look; the exact checks are in the report).
2. **Drop unused `libc` dep from `app/src-tauri`** (sweep 2, fix-now): one-line Cargo.toml deletion, then a build to confirm.
3. **E2E scenario inventory** (sweep 2, verify): resolve the six `fixtures/e2e-mocks/acceptance-*.json` files — read their `pattern` fields, grep `app/tests/` + `wdio.conf.mjs` for matching requests; delete the dead ones or wire a spec to each.
4. **Spec stale branch pointer** (sweep 3, ticket): `docs/spec/v0.md:196` — spec-owner decision on rewording.
5. **Soft file-size splits** (sweep 3, ticket): `session.rs` (908) and `payload.rs` (785) — split when next touched; low urgency, no hard-limit breach.
