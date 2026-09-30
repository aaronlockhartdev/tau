# RESUME NOTE — E2E / mock LLM (B2), FINAL RUN, rewritten by the integrator

Branch: `test/e2e-mock-llm` in /private/tmp/tau-b2. The worktree's cargo `target/` is WARM.
Done and committed: the mock crate (`2db82ea`), scenarios + per-feature specs (`1574ab3`),
green mock leg with per-leg workspaces (`7b5bdb3`), Cargo.lock entry (`394cfea`).

## Uncommitted (the previous run wrote this, then died running the verification)
- `crates/tau-acceptance/src/main.rs` (17 lines) — the live-* retarget to the mock
- `justfile` (43 lines) — mock built first, TAU_LIVE gate removed
- 6 new scenario files in `dogfood/e2e-mocks/`: acceptance-tools.json,
  acceptance-subagent-parent.json, acceptance-subagent-child.json,
  acceptance-om.json, acceptance-om-observer.json, acceptance-om-reflector.json
Read these diffs/files first — they are the starting point, not a blank page.

## THIS RUN'S JOB (in order; COMMIT EARLY AND OFTEN — commit the moment a step is green)
1. **VERIFY the acceptance retarget** (the step it died on): build the acceptance driver
   (incremental — fast) and run all three live suites + core against the mock via the
   justfile path. A suite failure means the SCENARIO doesn't produce what the EXISTING
   assertion checks — fix the scenario file (or the tiny plumbing), re-run, repeat.
   Do NOT weaken any assertion in main.rs. live-subagent legitimately takes up to its
   300 s budget — be patient with long suite runs; never kill a running suite to save time.
   When all three + core are green: COMMIT (`test(acceptance): live-* suites run against the mock by default; TAU_LIVE retired`).
2. **STRESS REPLACEMENT**: `crates/tau-core/tests/fixture_gen.rs` → deterministic,
   realistic-shaped large session (a few hundred to ~1k entries; realistic mix of user /
   assistant / tool / om entries with realistic lengths; PATH-NORMALIZED content — the
   current generator embeds the per-run tmp path, which breaks the sha256 pin; generate
   against a fixed logical workspace path so every generation is byte-identical). Keep
   the two 25 ms-coalesced child streams the perf bar measures. Verify byte-identity by
   generating twice and hashing. Re-pin the new stable sha256 in `app/wdio.conf.mjs` and
   update `app/tests/e2e/stress.spec.mjs` only where the old shape is referenced (same
   perf assertions: windowing, no visible lag, 500 ms budget). Verify the stress leg
   green locally: `cd app && TAU_E2E_MODE=stress npm run test:frontend`. COMMIT.
3. **SPEC ERRATA** (`docs/spec/v0.md`, errata ONLY, matching the existing dated-errata
   style, e.g. the 2026-09-23 roadmap-G block): two short blocks recording the 2026-09-30
   user decision — (a) the generated 10k-entry stress fixture retired as unrealistic,
   replaced by a deterministic realistic-shaped large session, performance bar unchanged
   in substance; (b) TAU_LIVE retired as a CI concept — live-* acceptance suites run
   against the deterministic mock by default (red = code problem), live endpoint a local
   opt-in via --endpoint. COMMIT.
4. **CI ACCEPTANCE JOBS** (`.github/workflows/ci.yml` — the macos-acceptance +
   linux-acceptance jobs ONLY; NOTE: main has moved on and now contains new rust-side
   jobs (rust-deny, rust-coverage, fuzz-smoke) from a merged parallel branch — your
   branch predates that merge, so when you edit ci.yml touch ONLY the two acceptance
   jobs' steps; the integrator reconciles the rest at merge time): the E2E step runs
   the replay + mock legs (check wdio.conf.mjs's mode grammar for the value that selects
   exactly those; if 'all' includes stress, use an explicit selection — document your
   choice in a one-line comment); the acceptance step runs `core` + `live-tools
   live-subagent live-om` against the mock (no TAU_LIVE anywhere in CI); the mock
   server is built in the job before the acceptance step; keep the existing
   failure-artifact uploads. The realistic stress leg stays LOCAL-only (starved-CI-webview
   rationale, docs/research/tauri-ci.md §4). Validate the YAML parses. COMMIT.
5. **REGRESSION GATE** (final): the mock leg still green
   (`TAU_E2E_MODE=mock npm run test:frontend`); `cargo nextest run --workspace` green;
   `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings`
   clean. Then FINAL REPORT.

## Rules
AGENTS.md comment discipline; no file >500 LOC where practical (1000 hard —
tau-acceptance/src/main.rs is 607 LOC today: if your changes grow it past ~700, extract
the endpoint/scenario plumbing into a small new module in that crate and say so); no new
dependencies; do NOT touch crates/tau-protocol/**, app/src-tauri/**, app/svelte/**,
app/package.json, fuzz/, deny.toml, nightly.yml, the rust/frontend ci jobs,
docs/research/**, docs/overhaul/**. If the run is running low on time, STOP at the last
verified-green checkpoint, commit it, and report exactly where you stopped.

## FINAL REPORT
commit hashes; per-suite live-* result (and what each acceptance scenario scripts); the
new fixture shape (entry count, mix, path-normalization mechanism) + stable sha256 +
stress leg result; the exact errata text landed; the CI hunk summary; the mock-leg
regression result; where you stopped (if anywhere) and what remains.
