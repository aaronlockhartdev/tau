## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues — the wayfinder maps and their child tickets among them — driven via the `gh` CLI. See `docs/agents/issue-tracker.md` (conventions + wayfinding operations).

### Triage labels

Five triage roles: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` (the glossary), `docs/spec/v0.md` (the approved v0 contract), and `docs/adr/` at the repo root. The v0 clean-up's planning doc: `docs/v0-cleanup-roadmap.md`; maps and tickets live in GitHub Issues. See `docs/agents/domain.md`.

### tauri-pilot

To inspect, drive, or screenshot a running dev build, load the `tauri-pilot` skill. The app embeds its plugin in debug builds only; the CLI auto-detects the socket.

**The line (2026-09-24).** You, the agent, use **tauri-pilot** for direct interaction with the running app — debugging, dogfooding, screenshots. Automated testing uses **WebdriverIO + `@wdio/tauri-service`** in the official Tauri way ([WebDriver guide](https://v2.tauri.app/develop/tests/webdriver/), [CI guide](https://v2.tauri.app/develop/tests/webdriver/ci/); the embedded `tauri-plugin-wdio-webdriver` provider is the default). tauri-pilot never drives a test suite; WebDriverIO never drives ad-hoc agent interaction.
## Coding conventions

Rules, not suggestions. Enforced in CI where mechanical, in review where not.

**Comments — anti-slop discipline.**
- A comment is justified only by a *why* that is not obvious from the code: a constraint, a workaround, a non-obvious invariant, a decision citation (spec §/ADR/ticket).
- Never narrate what the code does ("loads the config"), never restate the name in prose, never use banner or section-divider comments, never write "Adds/Updates X" change narration.
- No speculative TODOs — open the ticket instead. A TODO that names no ticket and no reason is noise.

**Naming over commenting.** If a name needs a comment to be understood, fix the name first.

**Formatting.** Rust: `rustfmt` defaults + `clippy -D warnings`, both enforced in CI. Svelte/TypeScript: ESLint 10 flat config (`eslint-plugin-svelte` + `@stylistic`: 2-space indent, single quotes, semicolons), enforced in CI by the frontend job.

**File editing.** Repo files are edited with the file tools (read → edit with anchors, or write) — never with shell scripts (`sed`, `python`, `awk`). The file tools are atomic, leave a reviewable diff, and surface conflicts a silent in-place rewrite hides.
**No speculative code.** No dead abstractions, no "for the future" scaffolding, no flags for nonexistent features, no second implementation kept "just in case". Build the thing the ticket asks for; the next ticket extends it.

**File size.** Every code file stays under a soft 500-LOC limit and a hard 1000-LOC limit. The soft limit is a review smell that invites a split; the hard limit is a blocker, enforced in CI by the `file-size` job: any code file over 1000 LOC fails the build. No grandfathering.

**Doc comments** appear only on public API items whose contract is not self-evident from the signature — one line where possible, with a spec/ADR citation when a rule comes from one.

## Best practices

**Rust**
- Conversions: `as_*` (free view), `to_*` (allocating/computing), `into_*` (consuming); single-value wrappers expose `into_inner()`.
- Getters are bare names (`id()`); no `get_` prefix.
- Every error type implements `std::error::Error` + `Display`; `Display` messages lowercase, concise, no trailing punctuation; `Result<T, ()>` is not an error design.
- Types on a public surface (protocol payloads/events, `tau-core` public API) eagerly implement the common std traits that apply (`Debug`, `Clone`, `PartialEq`/`Eq`, `Hash`, `Default`).

**Tauri**
- `app.security.csp` must be a restrictive policy; `csp: null` is a violation.
- Global state and business logic live in the core process; the frontend stays a thin renderer.
- Commands are `async`; extract owned handles before any blocking/await work.
- Errors cross IPC only as tagged serializable `ProtocolError`; no ad-hoc `String` paths.
- High-frequency streams: coalesced `emit` batches or `Channel` — never a raw per-item loop.
- Capabilities: individual `allow-*` grants; a `:default` set needs a reason in the file.
- Adding a `#[tauri::command]` is a deliberate, reviewable decision (the command set is the IPC attack surface).
- Window creation from async context only (dormant — single-window v0).
- Everything across the IPC boundary is a typed protocol value; no raw `serde_json::Value`.
- `withGlobalTauri: true` stays (tauri-pilot dependency) — documented, not a rule.

**Svelte**
- `$effect` is for the outside world only (DOM, timers, third-party libs, logging) — never for synchronising state; that is `$derived`. Reactive reads needed after an `await` are captured into a local before the `await`.
- Shared state lives in `.svelte.ts` modules exporting an object binding; functions mutate internals, exports are never reassigned. No context, no `svelte/store`.
- Components: snippets over `<slot>`; `onX` props over `on:`; every `$props()` destructure type-annotated.

**TypeScript**
- `tsconfig`: `strict` + `verbatimModuleSyntax` + `isolatedModules` + `noUncheckedIndexedAccess` + `noFallthroughCasesInSwitch`. (`exactOptionalPropertyTypes` deferred — protocol decision; `noUnusedLocals`/`noUnusedParameters` skipped — ESLint owns that rule.)

Rationale and sources: `docs/research/refactor-{rust,tauri,svelte}.md`.

## Testing

**Placement.** Rust: simple correctness tests colocate (`#[cfg(test)] mod tests` in the same file); tests that cross a module boundary live in the crate's `tests/` dir. TS: `*.test.ts` colocated next to the code.

**Positive and negative.** Every behavior gets a positive and a negative test. Negatives assert the `Result` (specific `Err` via `matches!`); `#[should_panic]` only where the path genuinely panics.

**Rust.** `#[tokio::test]` for async — the `multi_thread` flavor only when the test needs it. One tool per job: `nextest` (all Rust test runs), `cargo-llvm-cov` (coverage: report-only CI artifact, ≥80% is a review-time aim, no floor), `cargo-fuzz` (`fuzz/`; ~300 s smoke per target on PR, 1 h + corpus `cmin` nightly), `insta` (snapshot goldens; CI fails on new/changed — keep that), `proptest` (properties), `cargo-deny` (supply chain), Miri on the pure crates (nightly only). Selection rationale with primary sources: `docs/research/testing-rust.md`.

**Frontend.** Vitest + `@testing-library/svelte`; settle effects with `await tick()` / `vi.waitFor`, no fixed sleeps. Rationale: `docs/research/testing-frontend-e2e.md`.

**Acceptance and E2E.** The `live-*` acceptance suites are mock-first — deterministic `tau-mock-llm` with hash-pinned scenarios in `dogfood/e2e-mocks/`, so a red is a code problem, never a network/model problem; a real endpoint is a local dogfood opt-in via `TAU_ENDPOINT`/`TAU_MODEL`. E2E drives the DEBUG binary (built with `--features e2e`) over its embedded WebDriver server; `TAU_E2E_MODE` selects the leg (replay in CI, stress local).

**Tests are code.** The comment discipline, rustfmt/`clippy -D warnings`, and file-size limits above apply to test files.

## Verifying UI work

Confirm a visual bug by taking a screenshot of the running app and reading it — a green DOM assertion does not prove the UI renders correctly.
