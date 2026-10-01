# Phase 2 plan — best-practice codification + deepening refactor

2026-10-01. Inputs: phase 1 test/CI overhaul (merged; 452 workspace tests, 91.7% / 86.7% coverage,
acceptance 6/6, e2e replay 20/20); three research docs (`docs/research/refactor-{rust,tauri,svelte}.md`,
all claims cited to primary sources); the architecture review
(`docs/overhaul/architecture-review-20261001.html`, candidates R1–R5, F1–F6).

Two workstreams:

1. **Codify** — turn the research verdicts into AGENTS.md rules + tooling gates (section 0).
2. **Deepen** — the architecture candidates, as five sections with a design each (sections 1–4;
   F6 excluded, see §4).

## 0. Codification (one branch, merged first)

Everything here is config/lint/docs — file-disjoint from the deepening sections, so it lands alone
and gates the rest.

### Rules → AGENTS.md (new "Best practices" section; terse rule lines, rationale pointers to the research docs)

**Rust** (source: `refactor-rust.md` §1, Rust API Guidelines):
- Conversions: `as_*` (free view), `to_*` (allocating/computing), `into_*` (consuming);
  single-value wrappers expose `into_inner()`.
- Getters are bare names (`id()`); no `get_` prefix.
- Every error type implements `std::error::Error` + `Display`; `Display` messages lowercase,
  concise, no trailing punctuation; `Result<T, ()>` is not an error design.
- Types on a public surface (protocol payloads/events, `tau-core` public API) eagerly implement
  the common std traits that apply (`Debug`, `Clone`, `PartialEq`/`Eq`, `Hash`, `Default`).
- Not codified (no primary source): "no `unwrap` in production" — left to review judgement.

**Tauri** (source: `refactor-tauri.md` §1):
- `app.security.csp` must be a restrictive policy; `csp: null` is a violation.
- Global state and business logic live in the core process; the frontend stays a thin renderer.
- Commands are `async`; extract owned handles before any blocking/await work.
- Errors cross IPC only as tagged serializable `ProtocolError`; no ad-hoc `String` paths.
- High-frequency streams: coalesced `emit` batches or `Channel` — never a raw per-item loop.
- Capabilities: individual `allow-*` grants; a `:default` set needs a reason in the file.
- Adding a `#[tauri::command]` is a deliberate, reviewable decision (the command set is the IPC
  attack surface); widen via `AppManifest::commands` scoping if the seam ever grows.
- Window creation from async context only (dormant — single-window v0).
- Everything across the IPC boundary is a typed protocol value; no raw `serde_json::Value`.
- `withGlobalTauri: true` stays (tauri-pilot dependency) — documented, not a rule.

**Svelte** (source: `refactor-svelte.md` §1–2):
- `$effect` is for the outside world only (DOM, timers, third-party libs, logging) — never for
  synchronising state; that is `$derived`. Reactive reads needed after an `await` are captured
  into a local before the `await`.
- Shared state lives in `.svelte.ts` modules exporting an object binding; functions mutate
  internals, exports are never reassigned. No context, no `svelte/store`.
- Components: snippets over `<slot>`; `onX` props over `on:`; every `$props()` destructure
  type-annotated.

**TypeScript** (source: `refactor-svelte.md` §3):
- `tsconfig`: `strict` + `verbatimModuleSyntax` + `isolatedModules` + `noUncheckedIndexedAccess`
  + `noFallthroughCasesInSwitch`. Deferred: `exactOptionalPropertyTypes` (a protocol decision,
  not a frontend one). Skipped: `noUnusedLocals`/`noUnusedParameters` (ESLint owns that rule).

### Tooling

| Change | Where | Gate |
|---|---|---|
| `csp: null` → restrictive policy (docs' example shape + `blob:`/`data:` in `img-src` for file previews) | `app/src-tauri/tauri.conf.json` | build + e2e replay |
| `clippy.toml` with `msrv = "1.98.1"` | repo root | existing clippy gate |
| Clippy `pedantic` cherry-picks — a short named list, chosen at adoption in this section, explicit `#[allow]` for deliberate exceptions | workspace lint config | existing clippy gate |
| `typos` (crate-ci/typos) | new PR job | PR gate |
| cargo-machete (unused deps) | nightly job | informational only (self-declared imprecision) |
| ASan + TSan `cargo test` of the workspace | nightly job, x86_64 Linux | nightly |
| ESLint `recommendedTypeChecked` + `projectService`, `.svelte` files routed through the TS parser per the plugin README (`extraFileExtensions: ['.svelte']`, `svelteConfig`) | `app/eslint.config.mjs` | existing frontend job |

Skipped with reasons (recorded in the research docs): cargo-udeps, cargo-semver-checks,
cargo-audit, a standalone ACL-audit CLI, the svelte plugin's experimental rules.

### Verification for section 0
`cargo test` + clippy + fmt (new lints: fix the resulting wave), frontend job (lint + svelte-check
+ tests + build), `just acceptance` 6/6 (CSP change surfaces in the webview), e2e replay 20/20.

## 1. Core surface: the session constructor (R2 + R5)

**Hot spot:** harness/ = 37 commits since 09-20 — the crate's hottest cluster.

**Design.**
- Interface: one core-side constructor —
  `AgentSession::launch(workspace, &config, store, role, event_tx) -> Result<AgentSession, _>`.
  `role` (root / child / observer / reflector) absorbs the role-dependent defaults that the
  13-field `SupervisorParams` and 10-field `SessionParams` currently spread across callers.
- Behind the seam: the bridge↔supervisor circular-reference dance, parameter assembly, and the
  four post-construction hook setters (kind-string → event shaping included) all become internal.
  Both test kits collapse to "build a session with a canned provider".
- Accessor shrink (R5, same branch): the 27 `pub fn` surface → behaviour
  (`send` / `process` / `stop`) plus projections the harness actually needs (snapshot, task view);
  the single-field accessors drop to `pub(crate)`.
- Tests: all existing behaviour tests survive (they drive the harness API, not the wiring).
  New: construction wiring is directly testable — a child built through the constructor routes
  through its parent, no supervisor harness required.

**Ownership:** `crates/tau-core` (agent/, subagent/, harness/, both test kits).

## 2. Core surface: tool routing, child links, turn-end OM (R1 → R3 → R4, one branch, in order)

**Design, in sequence.**
- **R1 — tool surface module.** `specs_for(role) -> Vec<ToolSpec>` and
  `dispatch(role, name, args, ctx) -> ToolOutcome` own the 7 spec builders, the routing table
  (core / supervisor / child link / task store / scoped recall), the spawn-time agent-type
  filter, and the child refusal rule. The turn loop dispatches against its surface and stops
  matching tool names; the load-bearing comment at turn.rs:318 gets a home.
- **R3 — ChildLink owns the pointer model.** `task_view()` (folded, filtered to the worker),
  `send`/`stop` routed through the parent; the handle becomes opaque — the
  `rsplit_once` string parse in dispatch_commands goes away. Snapshot builder and the four
  dispatch sites call link methods instead of re-opening the parent store.
- **R4 — `OmState::settle_turn(store, …)`.** One method owns the turn-end sequence
  (unobserved → activation → plan → observe/reflect) with its own store reads and status hook;
  the turn loop makes one call. The 80 GB scenario becomes a unit test.
- Why one branch: all three touch `turn.rs` / `agent.rs` / dispatch — internal sequence, no
  parallelism. R3 after R1 (the surface owns child routing); R4 last (turn loop settled).
- Tests: existing OM tests (plan / window / record) and dispatch tests survive. New: the
  role→specs table, child refusal, handle round-trip, and the settle sequence — each a direct
  unit test of a module that was previously only reachable by running a whole turn.

**Ownership:** `crates/tau-core` (tools, agent/turn, subagent, om_integration, harness dispatch).

## 3. Frontend: store + transcript (F2 → F1 → F5, one branch, in order)

**Design, in sequence.**
- **F2 — read facade over the bag.** Derived accessors: current session-or-null, pane for
  workspace, file cache for workspace. The eight components read through the facade; the raw
  bag becomes an implementation detail; the mock store implements the facade shape, not
  20+ raw fields.
- **F1 — windowing policy module.** A pure module: state over (visible range, last-fetched per
  direction, turn state) → fetch requests; owns hysteresis, open-tail, the mid-turn union, and
  the turn-end re-read. Transcript's three `$effect`s become thin adapters (read virtualizer
  state → policy → `fetchWindow`). Characterization tests capture current behaviour
  (hysteresis values, edge cases) *before* the move.
- **F5 — session command wrapper.** One wrapper owns clear-banner → invoke → refetch
  `session_list` → reapply; per-command differences pass as parameters.
- Why this order: the facade first gives F1 and F5 a stable seam to read through.
- Tests: the 336 component tests are the behaviour net; new: windowing policy unit-tested
  directly (the interface is the test surface), facade and wrapper tests.

**Ownership:** `app/svelte` (lib/store.svelte, lib/windowing.ts (new), lib/commands.ts (new),
components/Transcript.svelte, the reading components, mock-store).

## 4. Frontend leaf modules (F3 + F4, one branch)

- **F3 — entry presentation module.** `present(entry) -> { label, icon, kv rows, sections }`;
  the 10-branch per-kind ladder moves out of the 814-LOC card, which becomes a generic
  renderer. Kind knowledge testable without mounting a component.
  Files: `lib/entries.ts`, `lib/markdown.ts`, `lib/presentation.ts` (new), `components/EntryCard.svelte`.
- **F4 — one selection rule.** The multi-select interaction (cmd/ctrl toggle, shift range,
  click clears) implemented once, parameterized by the ordered id sequence; both the tabs
  variant (`panes.ts`) and the tree variant (`SessionNode`) call it.
  Files: `lib/panes.ts`, `lib/selection.ts` (new), `components/SessionNode.svelte`, `components/LeftPane.svelte`.

The small, mechanical pair — the warm-up of the frontend track.

**Excluded: F6** (expansion-state module) — framework-blocked (Svelte 5.57 can't proxy Maps);
the current workaround is stable, commented, tested. Revisit if the limitation goes away.

## 5. Execution model

- **Section 0 alone first**, merged.
- Then **two parallel tracks** (file-disjoint trees), each strictly sequential inside:
  - Rust track: 1 → 2
  - Frontend track: 4 → 3
- Per section: one branch → implement (subagent where the section is mechanical, direct work
  where it isn't) → reviewer subagent after staging → user reviews notes → merge in order.
- **Verification net after every merge:** `cargo nextest run` · clippy `-D warnings` · fmt ·
  `svelte-check` · eslint · coverage (informational, ≥80% target) · `just acceptance` 6/6 ·
  e2e replay 20/20 · protocol parity.
- During sections 1–2 (the hot cluster): tests + acceptance after each commit, not just at
  merge.

## 6. Non-goals

- F6 (see §4).
- `exactOptionalPropertyTypes` (deferred — protocol decision).
- No protocol shape changes (the pointer model is settled in ADR-0001).
- No multi-window work (R8 stays dormant).
- The e2e stress leg's local 'all' hang is issue #39 — tracked separately, not phase 2 scope.
