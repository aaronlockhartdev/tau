# Section 0 brief — Codification (phase 2)

You are the worker for section 0 of the phase 2 refactor. Read this brief fully before
touching anything. The plan: `docs/overhaul/phase2-plan.md` §0. Research sources (all
first-party-cited): `docs/research/refactor-rust.md`, `docs/research/refactor-tauri.md`,
`docs/research/refactor-svelte.md`.

## Ground rules

- Repo: `/Users/aaron/git/tau`. Create branch `phase2/section0-codification` from `main`
  and work there. Commit per work item (≈8 commits), clear messages, no WIP cruft.
- **Do not touch** `app/src-tauri/gen/schemas/*` — the user has uncommitted changes there.
  Never `git add` them. (A `cargo build -p tau-app` may regenerate them; that is fine,
  just don't commit.)
- Version pins: this repo pins tool/action versions. Look up current versions from
  primary sources (crates.io, GitHub releases) before writing them — never from memory.
- No feature work, no refactors. Config, lint, docs only.
- Wave rule for every new lint tier: fix findings mechanically. If a **single** lint
  produces more than 50 findings, drop that lint from the list and report it in your
  final message instead of fixing it.
- Tests are code: AGENTS.md conventions (comment discipline, file size) apply to anything
  you add.

## Work items

### 1. AGENTS.md — new `## Best practices` section

Insert after the `## Coding conventions` section (before `## Testing`). Terse rule lines
only; end the section with one pointer line: *Rationale and sources: `docs/research/refactor-{rust,tauri,svelte}.md`.*

Copy these rules verbatim (grouped under bold leads **Rust** / **Tauri** / **Svelte** /
**TypeScript**):

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

### 2. CSP — `app/src-tauri/tauri.conf.json`

Line 22 is `"csp": null`. Replace with a restrictive policy. Start from the official
Tauri v2 docs example (check the current docs — the `app.security.csp` field takes a
browser-style policy **string**):

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' asset: http://asset.localhost blob: data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'self'
```

Verify it actually works, don't guess: build the app, run it, and check the webview
console for CSP violation errors (load the `tauri-pilot` skill to drive the running app;
its log/console capture is the check). If something legitimate is blocked, widen the
smallest directive that unblocks it and note the reason in the commit message.

### 3. `clippy.toml` (repo root)

```toml
msrv = "1.98.1"
```

### 4. Clippy pedantic cherry-picks

No `[workspace.lints]` exists today; CI runs `cargo clippy --workspace --all-targets -- -D warnings`
(ci.yml line 36). Add `[workspace.lints.clippy]` to the root `Cargo.toml` with exactly
these lints (deny):

```
as_conversions, bool_to_int_with_if, cast_possible_truncation, cast_sign_loss,
cloned_instead_of_copied, missing_const_for_fn, needless_pass_by_value, redundant_else,
string_slice, string_to_string, trivially_copy_pass_by_ref, unnecessary_debug_formatting,
unused_self, wildcard_imports
```

Wire every workspace member to inherit it: `[lints] workspace = true` in each member
`Cargo.toml` (enumerate members from the root `[workspace] members`; `fuzz/` is a
separate cargo-fuzz workspace — leave it alone). Then fix the resulting wave
(wave rule above). `#[allow]` with a one-line reason is acceptable for deliberate
exceptions.

### 5. `typos` PR gate — `.github/workflows/ci.yml`

New job, `typos`, `runs-on: ubuntu-latest`, using the `typos-cli/typos` GitHub Action at
its current pinned release (look it up). No inputs needed.

### 6. Nightly additions — `.github/workflows/nightly.yml`

Add two jobs (ubuntu-latest, nightly toolchain, `swatinem/rust-cache`):
- `asan`: `RUSTFLAGS="-Zsanitizer=address" cargo +nightly test --workspace`
- `tsan`: `RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test --workspace`

Add a `machete` step to one of them (not a separate job): install `cargo-machete`
(taiki-e/install-action, pinned) and run `cargo +nightly machete` with
`continue-on-error: true` (informational — the tool self-declares imprecision).

If ASan/TSan of the full workspace is infeasible on the first nightly run (e.g. a crate
needs an allow), that is a follow-up ticket, not a section 0 blocker — report it.

### 7. Type-aware ESLint — `app/eslint.config.mjs`

- Swap `...tseslint.configs.recommended` → `...tseslint.configs.recommendedTypeChecked`.
- The `**/*.svelte` block already routes `.svelte` through the TS parser with
  `extraFileExtensions` + `svelteConfig`; add `projectService: true` to its
  `parserOptions` (eslint-plugin-svelte README, typed-linting wiring) so `.svelte`
  script blocks get the typed rules.
- Fix the resulting wave of typed-rule errors (floating promises, `any`-taint,
  unnecessary assertions). Wave rule applies.

### 8. tsconfig — `app/tsconfig.json`

Add to `compilerOptions`: `verbatimModuleSyntax`, `isolatedModules`,
`noUncheckedIndexedAccess`, `noFallthroughCasesInSwitch`. Fix the resulting
`| undefined` wave in `svelte/**`.

## Verification (all must pass before your final message)

Read the `justfile` for the repo's canonical commands, then run, in the main repo on
your branch:

1. `cargo nextest run --workspace`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo fmt --check`
4. frontend: eslint + svelte-check + vitest + vite build (the CI frontend job's commands)
5. `just acceptance` — expect 6/6 including e2e replay 20/20 (CSP change surfaces here)

## Final message

Per work item: what changed, lint/typo waves (count fixed, lints dropped per the wave
rule), the CSP policy string as landed, verification results (each command + pass/fail),
and any follow-ups you recommend (do not implement them).
