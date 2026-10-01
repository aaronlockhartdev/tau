# Tau test/CI overhaul — phase 1 (testing) shared briefing

Read this first. It binds every subagent working on phase 1. Coding conventions live in
`AGENTS.md` (repo root) — they are rules, not suggestions. Tooling rationale with primary-source
citations: `docs/research/testing-rust.md` and `docs/research/testing-frontend-e2e.md`.

## Repo map

- Cargo workspace (root `Cargo.toml` members):
  - `crates/tau-core` — the agent harness: sessions, subagents, tool dispatch, LLM provider
    (OpenAI **Responses API** SSE dialect), observation/compaction (om), hashline, skills, tasks.
  - `crates/tau-protocol` — serde IPC payload/event types shared with the TS mirror.
  - `crates/tau-acceptance` — acceptance-test driver binary (launch smoke, core, live-* suites).
  - `app/src-tauri` — Tauri 2 glue (crate `tau-app`); the `e2e` cargo feature (debug-only) gates
    the WebdriverIO/pilot plugins.
- Frontend (`app/`): Svelte 5 (runes) + TypeScript + Vite 8. Source in `app/svelte/`
  (`lib/*.ts` logic, `components/*.svelte`, routes `++*.svelte`). Tests:
  `app/svelte/**/*.test.ts` (Vitest 5 + jsdom), `app/tests/e2e/*.spec.mjs`
  (WebdriverIO 9 + `@wdio/tauri-service` 1.4 driving the DEBUG binary over its embedded WebDriver
  server; config `app/wdio.conf.mjs`, a "legs" model selected by `TAU_E2E_MODE`).
- CI (`.github/workflows/ci.yml`): jobs `rust` (fmt, clippy -D warnings, nextest; macos-15 +
  ubuntu), `file-size`, `frontend` (build, protocol parity, vitest), `macos-acceptance`
  (release build + E2E replay), `linux-acceptance` (same under xvfb).
- `justfile` is the single build/acceptance entry point; `just test` = nextest + vitest.
- Existing test infrastructure to build on: `crates/tau-core/src/provider/tests/server.rs`
  (one-shot tokio SSE mock), the `canned://` provider scheme, hash-pinned `.jsonl` session
  fixtures (`dogfood/sessions/`, sha256-pinned in `wdio.conf.mjs`),
  `app/svelte/scripts/protocol-parity.mjs` (Rust↔TS command-surface parity).

## Agreed direction (user-approved 2026-09-30)

1. **Coverage**: aim ≥80% per logic crate / `svelte/**`; **no hard CI floor**; report-only
   (CI artifact, no Codecov). Rust: `cargo-llvm-cov` (native `--nextest`). TS:
   `@vitest/coverage-v8` (default provider).
2. **CI shape**: PR = fast (unit/integration, E2E replay, lint, cargo-deny, coverage report,
   ~300 s fuzz smoke). Nightly (new `.github/workflows/nightly.yml`) = long fuzz + corpus
   `cmin` + Miri on pure crates.
3. **Rust tooling**: cargo-fuzz 0.13 (`fuzz/` workspace member); insta 1.48 (goldens +
   redactions; CI fails on new/changed snapshots by default — keep that); proptest 1.11
   (already a workspace dep); cargo-deny 0.20 (advisories + licenses + bans;
   `[licenses.private] ignore = true`; do **not** add cargo-audit); Miri nightly, scoped to
   pure crates only (never a PR gate).
4. **Frontend tooling**: `@testing-library/svelte` 5.4.2 + `@testing-library/user-event` +
   `@testing-library/jest-dom` on the existing vitest+jsdom (add the `svelteTesting()` plugin);
   ESLint 10.11 flat config + `eslint-plugin-svelte` 3.23 + `typescript-eslint` 8.71 +
   `@stylistic` (2-space, single quotes, semicolons on). **No Biome** (experimental Svelte
   support + live file-corrupting formatter regression in the current release).
5. **Mock LLM**: a new Rust binary serving the OpenAI **Responses API** SSE dialect —
   `response.output_text.delta`, `response.output_item.done` (complete `function_call` item:
   `{id, call_id, name, arguments}` where `arguments` is a complete JSON string in one event),
   `response.completed` + usage, `[DONE]`, `:`-prefixed comment keep-alives. **Not**
   chat/completions. Deterministic: file-driven scenarios, sha256-pinned, fixed
   ids/call_ids/usage, lorem-ipsum text at realistic lengths, multi-frame streaming with small
   sleeps, a fixed fallback response for unscripted prompts.
6. **E2E**: keep WebdriverIO (official Tauri way, user decision 2026-09-24). Per-feature spec
   files + a `support/` dir. `specFileRetries: 1` on CI legs only. Keep the existing
   log-capture + failure-artifact upload.
7. **Stress / TAU_LIVE (spec errata, lands with section 4)**: retire the generated 10k stress
   fixture; replace with a realistic-shaped large session (same perf property). Retire
   `TAU_LIVE` as a CI concept: the `live-*` acceptance suites retarget to the mock LLM by
   default (deterministic; red = code problem); `--endpoint`/`--model` stay for local live
   dogfood.

## Test conventions (formalized for phase 1; landed in AGENTS.md with the phase completion)

- **Rust**: simple correctness tests colocated (`#[cfg(test)] mod tests` in the same file);
  module/boundary-crossing tests in the crate's `tests/` dir; `#[tokio::test]` for async
  (`flavor = "multi_thread"` only when the test needs it); every behavior gets a **positive and
  a negative** test; parser negatives assert the `Result` (specific `Err` via `matches!`), never
  `#[should_panic]` unless the path genuinely panics.
- **TS**: `*.test.ts` colocated next to the code; component tests via `@testing-library/svelte`
  (render + user-event + jest-dom matchers); settle effects with `await tick()` / `vi.waitFor`
  — no fixed sleeps.
- **Tests are code**: AGENTS.md comment discipline applies (no narration, no banners),
  rustfmt + `clippy -D warnings` clean, 2-space/single-quote/semicolons on in TS, no file over
  1000 LOC.

## Ground rules for subagents

- You work in **your own git worktree**. Create and check out a **new branch with the exact
  name given in your brief**; commit your work there (conventional-commit style, e.g.
  `test(tau-core): …`). Do not push. Do not touch `main`.
- **File ownership**: only the files your brief lists. If you must touch a file outside the
  list (e.g. a shared config), keep the edit minimal and local, and call it out in your final
  report.
- **Shared-file hotspots** (parallel agents edit these; keep your hunks minimal and local —
  the integrator merges the branches sequentially): root `Cargo.toml` (workspace
  members/deps), `.github/workflows/ci.yml` (your jobs only), `app/package.json` (your
  deps/scripts only).
- **No speculative code**: no future scaffolding, no abstractions the brief doesn't ask for, no
  dependencies beyond your brief's list.
- **If a new test exposes a genuine production bug**: fix it minimally, add the regression
  test, and report it prominently. No refactors beyond the minimal fix.
- **Verify before committing**: the suites you touched are green
  (`cargo nextest run -p <crate>` / `npx vitest run`), rustfmt `--check` + clippy
  `-D warnings` clean (Rust), svelte-check + eslint clean (TS).
- **Final report** (your returned message): what changed (by file), test/coverage numbers
  before → after, known gaps, bugs found+fixed, and anything the integrator needs
  (merge-order notes, follow-ups for later phases).

## Section ownership (for merge planning)

- §1 Rust correctness: A1 = `crates/tau-core` (batch A); B1 = `crates/tau-protocol` +
  `app/src-tauri` (batch B, together with §2 tooling).
- §2 Further Rust testing: B1 — `fuzz/`, `deny.toml`, insta/proptest, `ci.yml` rust-side jobs,
  `nightly.yml`, root `Cargo.toml`.
- §3 TS correctness: A2 — `app/` (package.json, svelte/**, vitest config, eslint config,
  `ci.yml` frontend job only).
- §4 E2E: B2 — `crates/tau-mock-llm` (new), `dogfood/e2e-mocks/`, `app/tests/e2e/`,
  `app/wdio.conf.mjs`, `crates/tau-acceptance`, `justfile`, `docs/spec/v0.md` (errata),
  `ci.yml` acceptance jobs only, root `Cargo.toml` (member), minimal `data-testid` additions in
  `app/svelte/components/**`.
