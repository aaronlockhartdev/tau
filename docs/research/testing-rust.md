# Research: Rust testing & code-quality tooling for tau

Researched 2026-09-30. All claims below are cited to primary sources (official docs, upstream GitHub repos, crates.io, release metadata). Versions are the current stable at research time.

**Question.** Which coverage, fuzzing, snapshot, property-based, and supply-chain tools are best-in-class for a 4-crate Rust workspace (edition 2024, tokio, serde, reqwest/rustls) whose CI is `cargo fmt` + `clippy -D warnings` + `cargo nextest` on macos-15 and ubuntu-latest — and does the user-agreed direction (coverage with 80%+ aim and no hard floor; short PR + long nightly fuzzing; snapshot goldens for protocol payloads; property tests for parsers/invariants) hold up against what the primary sources say?

**Short answer.** Yes, with one correction: **cargo-llvm-cov** (coverage), **cargo-fuzz/libFuzzer** (fuzzing — the only engine OSS-Fuzz supports for Rust), **insta** (snapshots), **proptest** (property-based; already a dep), **cargo-deny alone** (supply chain; cargo-audit becomes redundant), and **Miri scoped to pure crates only**. The one conflict with the agreed direction: **OSS-Fuzz is not adoptable now** — not because of the AGPL license (OSS-Fuzz imposes no license restriction), but because its acceptance bar is "significant user base and/or critical to global IT infrastructure," which a new repo does not meet; its free PR-level service (CIFuzz) also requires that integration. Short-PR/long-nightly fuzzing must therefore be self-hosted CI.

---

## 1. Coverage: cargo-llvm-cov vs tarpaulin

### cargo-llvm-cov (v0.9.1, released 2026-09-06)

- [crates.io `cargo-llvm-cov`](https://crates.io/crates/cargo-llvm-cov): max stable 0.9.1, updated 2026-09-06.
- [README](https://github.com/taiki-e/cargo-llvm-cov/blob/master/README.md) (taiki-e/cargo-llvm-cov):
  - "Support `cargo test`, `cargo run`, and **`cargo nextest`** with command-line interface compatible with cargo" — the `--nextest` flag "internally calls `cargo nextest run`". This is the only one of the two tools with native nextest support, which matters because tau's CI test runner *is* nextest.
  - Coverage is LLVM source-based (`-Cinstrument-coverage`), reporting **line and region** metrics; export via `--lcov`, `--json`, `--cobertura`, `--html`, and `--summary-only` (per-file summary).
  - Per-crate/workspace control: `--workspace`, `--exclude <SPEC>`, `--exclude-from-test`, `--exclude-from-report`.
  - Source-based exclusion: `--ignore-filename-regex` for file patterns (vendored sources excluded by default), and the `#[coverage(off)]` attribute for functions/modules (unstable — requires nightly, gated behind the `coverage_nightly` cfg the tool sets).
  - Thresholds exist if wanted later: `--fail-under-lines`, `--fail-under-regions`, etc.
  - Platform support: prebuilt binaries for Linux (x86_64/aarch64 gnu+musl, powerpc64le, riscv64, s390x), **macOS (x86_64, aarch64, universal)**, Windows (x86_64/aarch64 msvc + gnullvm), FreeBSD — covers tau's macos-15 + ubuntu-latest matrix.
  - CI: a worked GitHub Actions example (`taiki-e/install-action@cargo-llvm-cov` → `cargo llvm-cov --all-features --workspace --lcov` → `codecov/codecov-action@v5`); note from the README: "when using `--lcov` flag, only line coverage is available on Codecov" — use `--codecov` for region coverage.
  - [Releases](https://github.com/taiki-e/cargo-llvm-cov/releases): v0.9.1 (2026-09-06), v0.9.0 (2026-08-16) — actively released.

### tarpaulin (v0.37.5, released 2026-09-27)

- [crates.io `cargo-tarpaulin`](https://crates.io/crates/cargo-tarpaulin): max stable 0.37.5, updated 2026-09-27 (the crate is named `cargo-tarpaulin`; there is no `tarpaulin` crate).
- [README](https://github.com/xd009642/tarpaulin/blob/develop/README.md) (xd009642/tarpaulin — the repo moved from andreyng):
  - `-b, --branch` — "**Branch coverage: NOT IMPLEMENTED**". Line coverage only, in practice.
  - `--exclude-files` (wildcard file exclusion), `#[coverage(off)]` (nightly-only, same as llvm-cov).
  - `--fail-under <PERCENTAGE>` threshold; `--workspace`; outputs Json/Stdout/Xml/Html/Lcov.
  - Engines: on Linux the default is **ptrace**; macOS/Windows default to the **LLVM** engine. The ptrace engine is the reason nextest doesn't work (below).
- **No nextest integration**: [issue #992 "Cargo nextest integration"](https://github.com/xd009642/tarpaulin/issues/992) has been open since 2022-04-16. In [issue #1280](https://github.com/xd009642/tarpaulin/issues/1280) the maintainer states nextest "won't work well with ptrace based tracers like kcov or tarpaulins ptrace engine" and the work was "slightly put off" (only the LLVM engine would be feasible).

### VERDICT

**Best-in-class: cargo-llvm-cov 0.9.1.** It is the only candidate with native `--nextest` support (tau's CI runner), line+region source-based coverage that works on both macOS and Linux without engine/ptrace caveats, per-crate workspace reporting, and stable file-pattern exclusion (`--ignore-filename-regex`) plus the `#[coverage(off)]` attribute for code-level exclusion. tarpaulin 0.37.5 is actively maintained and fine for line coverage, but lacks nextest integration and branch coverage.

**Adoption shape (matches the agreed 80%-aim, no-hard-floor direction):** run `cargo llvm-cov --workspace --nextest` in CI *without* `--fail-under-*`, publish the HTML/lcov report as an artifact (and optionally to Codecov via `--codecov`); treat 80% as a review-time aim. The `--fail-under-lines` flag is available the day a hard floor is wanted — no tooling change required.

---

## 2. Fuzzing: cargo-fuzz (libFuzzer) vs AFL++ harnesses; OSS-Fuzz fit

### cargo-fuzz (v0.13.2, released 2026-06-09)

- [crates.io `cargo-fuzz`](https://crates.io/crates/cargo-fuzz): 0.13.2 (2026-06-09); companion [`libfuzzer-sys`](https://crates.io/crates/libfuzzer-sys) 0.4.13 (2026-06-04).
- [README](https://github.com/rust-fuzz/cargo-fuzz/blob/main/README.md) (rust-fuzz/cargo-fuzz): libFuzzer-based; "only works on x86-64 and Aarch64, and only on Unix-like operating systems (not Windows)"; needs a nightly compiler.
  - **Workspace support (directly relevant to tau's 4-crate layout):** "If your crate uses cargo workspaces, add `fuzz` directory to `workspace.members` … or use an independent workspace" (`cargo fuzz init --fuzzing-workspace=true`).
  - Corpus/bug workflow: `cargo fuzz add/run/tmin/cmin/coverage/fmt` — `tmin` minimizes a failing input, `cmin` minimizes the corpus, `coverage` generates coverage over the fuzzed program.
- [Rust Fuzz Book, "Fuzzing in CI"](https://rust-fuzz.github.io/book/cargo-fuzz/ci.html) (official book, rust-fuzz.github.io): the recommended CI pattern is exactly the agreed one — "It can be helpful, as a smoke test, to build and run your fuzz targets for a small amount of time in CI", with a worked GitHub Actions workflow: install nightly + pinned `cargo-fuzz`, `cargo fuzz build <target>`, `cargo fuzz run <target> -- -max_total_time=300` per target in a matrix, and **upload `fuzz/artifacts` on failure** for post-mortem. A longer nightly run is the same workflow on a `schedule:` trigger with a bigger `-max_total_time` (the book's pattern is time-bounded by design).
- [Book, "Structure-Aware Fuzzing"](https://rust-fuzz.github.io/book/cargo-fuzz/structure-aware-fuzzing.html): `libfuzzer-sys` offers two routes — `Arbitrary`-based generation and `fuzz_mutator!`-based structure-aware mutation — and reports that "an experiment has shown that `fuzz_mutator!`-based mutation provides better coverage over time than `arbitrary`-based generation" for systems that must accept well-formed structured input.
- [Book, "Targets"](https://rust-fuzz.github.io/book/cargo-fuzz/targets.html): links a community-maintained collection of cargo-fuzz targets.
- [Trophy case](https://github.com/rust-fuzz/trophy-case): tracked real-world bugs found by cargo-fuzz/libFuzzer.

### The 2026 alternative worth knowing: afl.rs (AFL++)

- [Rust Fuzz Book, "Fuzzing with afl.rs"](https://rust-fuzz.github.io/book/afl.html): "AFL++ is a popular, effective, and modern fuzz testing tool. afl.rs allows one to run AFL on code written in the Rust programming language."
- [Book, afl.rs setup](https://rust-fuzz.github.io/book/afl/setup.html): "afl.rs works on x86-64 Linux, x86-64 macOS, and ARM64 macOS"; install via `cargo install cargo-afl`.
- [crates.io `cargo-afl`](https://crates.io/crates/cargo-afl): 0.18.2 (2026-05-11) — maintained.
- Practical read: afl.rs is a legitimate second engine (AFL++'s power scheduling), but for this repo it adds a parallel toolchain (separate corpus format, separate target harness conventions, no OSS-Fuzz path) for marginal gain. **cargo-fuzz is the right single choice**; know afl.rs exists if a specific target ever resists libFuzzer.

### OSS-Fuzz fit for an AGPL public repo

- [google/oss-fuzz README](https://github.com/google/oss-fuzz/blob/master/README.md): "OSS-Fuzz: Continuous Fuzzing for Open Source Software" — free, run by Google with the Core Infrastructure Initiative and OpenSSF; "Projects that do not qualify for OSS-Fuzz (e.g. closed source) can run their own instances of ClusterFuzz or ClusterFuzzLite"; "Currently, OSS-Fuzz supports C/C++, **Rust**, Go, Python, Java/JVM, JavaScript and Lua code."
- [OSS-Fuzz, "Integrating a Rust project"](https://google.github.io/oss-fuzz/getting-started/new-project-guide/rust-lang/): "**Rust integration with OSS-Fuzz is expected to use `cargo fuzz`** to build fuzzers"; the only supported engine/sanitizer combination is "libfuzzer and address, respectively"; the builder image `gcr.io/oss-fuzz-base/base-builder-rust` ships nightly + cargo-fuzz preinstalled. (I.e. OSS-Fuzz's own Rust path *is* cargo-fuzz — choosing cargo-fuzz now is the prerequisite for OSS-Fuzz later; AFL++ has no OSS-Fuzz Rust path.)
- [OSS-Fuzz, "Accepting New Projects"](https://google.github.io/oss-fuzz/getting-started/accepting-new-projects/): "To be accepted to OSS-Fuzz, an **open-source project must have a significant user base and/or be critical to the global IT infrastructure**." AGPL-3.0-only is an OSI-approved open-source license ([OSI license page](https://opensource.org/licenses/AGPL-3.0)), so the license is not the blocker — the user-base/criticality bar is. A new repo like tau does not meet it today.
- [OSS-Fuzz, "Continuous Integration"](https://google.github.io/oss-fuzz/getting-started/continuous-integration/): CIFuzz (the free GitHub Action that fuzzes PRs, ~10 min default, using OSS-Fuzz's 30-day-old regressions/corpora) has an explicit requirement: "**Your project must be integrated with OSS-Fuzz.**" So CIFuzz is unavailable until acceptance.

**Recommended fuzz targets for a `tau-protocol`-style crate** (serde IPC payloads, SSE streams, session `.jsonl`):

1. **serde JSON parsing** — one target per top-level payload enum: `serde_json::from_slice::<Command>(data)` / `from_str`, oracle = "returns `Result`, never panics". Raw-bytes target for parser robustness; optionally a structure-aware `Arbitrary`/`fuzz_mutator!` target for well-formed-but-adversarial JSON (per the structure-aware page, mutation-based beats generation-based for deep coverage).
2. **SSE stream parsing** — feed raw bytes into the incremental SSE parser (the OpenAI-compatible provider's stream path); oracle = no panic, and re-parsing a truncated-then-extended stream stays consistent.
3. **Session `.jsonl` parsing** — raw-bytes target over the line-delimited reader; oracle = no panic, malformed lines produce per-line errors, not whole-file failure.
4. **Round-trip invariants** (property-testable, §4): serialize → deserialize → re-serialize stability for every payload type.

Corpus management: seed each target with a handful of real captured payloads in `fuzz/corpus/<target>/` (committed), let the fuzzer grow them, run `cargo fuzz cmin` in the nightly job to keep corpora small, and commit `tmin`-ed regressions as unit tests.

### VERDICT

**Best-in-class: cargo-fuzz 0.13.2 + libfuzzer-sys 0.4.13 (libFuzzer, nightly).** It is the de-facto standard for Rust fuzzing, the only engine in OSS-Fuzz's Rust path, has first-class workspace support, and its official CI guidance matches the agreed short-PR/long-nightly split. afl.rs (cargo-afl 0.18.2 / AFL++) is the one alternative worth knowing; not worth adopting alongside.

---

## 3. Snapshot testing: insta

- [crates.io `insta`](https://crates.io/crates/insta): **1.48.0** (2026-06-11). [Repo](https://github.com/mitsuhiko/insta) (mitsuhiko/insta, default branch `master`) pushed 2026-09-27 — actively maintained.
- [README](https://github.com/mitsuhiko/insta/blob/master/README.md): snapshot/approval testing for complex values with "comprehensive tools to review changes"; inline snapshots; the companion **cargo-insta** tool for the review workflow; VS Code extension for `.snap` files; diffing via `similar`.
- [insta docs, "Getting Started"](https://insta.rs/docs/quickstart/) (official site): the workflow is `cargo insta test` → `cargo insta review` (interactive review of new snapshots stored as `.snap.new`) → accept; `cargo insta test --review` combines them. cargo-insta is "entirely optional" — `cargo test` works directly, controlled by `INSTA_UPDATE`.
- **CI behavior (the key property for goldens):** `INSTA_UPDATE` default `auto` means "**no for CI environments or new otherwise**" ([docs, "Controlling Snapshot Updating"](https://insta.rs/docs/advanced/)). Consequence: on CI, a new or changed snapshot is *not* written and the test *fails* — exactly the "CI fails on new/changed snapshots" behavior the direction wants, with zero configuration.
- **Redactions** ([docs, "Redactions"](https://insta.rs/docs/redactions/)): an opt-in `redactions` cargo feature for any snapshot based on `serde::Serialize` output — selector syntax (`.key`, `["key"]`, `[index]`, `.*`, `.**` deep match), **static redactions** (hardcoded replacements, e.g. `".id" => "[uuid]"`) and **dynamic redactions** via `insta::dynamic_redaction` (a callback that can also *assert* the value matches a pattern before replacing it). A separate `filters` feature redacts strings. This is the mechanism for stabilizing protocol-payload goldens that contain ids/paths/timestamps.
- Alternatives worth naming: **expect-test** ([crates.io](https://crates.io/crates/expect-test) 1.5.1, last updated 2024-12-21; rust-analyzer/expect-test) — inline-snapshot focused, quiet for ~21 months; **similar-asserts** (pretty `assert_eq!` diffs, not a snapshot framework). Neither threatens insta's position.

### VERDICT

**Best-in-class: insta 1.48.0 (+ cargo-insta).** Active maintenance, the `cargo insta review` workflow, fail-on-new/changed-by-default in CI, and serde-aware redactions — the exact feature set for protocol-payload goldens. For tau: `assert_json_snapshot!`/`assert_yaml_snapshot!` over `tau-protocol` payload types, with `.**`-style redactions for volatile fields, committed `.snap` files as the goldens.

---

## 4. Property-based: proptest vs quickcheck vs rapid

- **proptest** — [crates.io](https://crates.io/crates/proptest): **1.11.0** (2026-03-24). [Repo](https://github.com/proptest-rs/proptest) (proptest-rs/proptest) pushed 2026-09-27. [README](https://github.com/proptest-rs/proptest/blob/main/proptest/README.md): "a property testing framework … inspired by the Hypothesis framework for Python"; status "fairly close to being feature-complete … mainly sees passive maintenance" (stable, not abandoned).
  - [Book, "Proptest vs Quickcheck"](https://proptest-rs.github.io/proptest/proptest/vs-quickcheck.html) (official book): "QuickCheck generates and shrinks values based on type alone, whereas Proptest uses explicit Strategy objects," and lists QuickCheck's resulting disadvantages: one generator/shrinker per type (custom generation requires hand-rolled newtypes), a single "size" knob, and non-composable struct generation.
  - [Book, "Failure Persistence"](https://proptest-rs.github.io/proptest/proptest/failure-persistence.html): failing cases are persisted under `proptest-regressions/` and "replay[ed] … before generating novel cases"; "It is recommended to check these files in to your source control so that other test runners (e.g., collaborators or a CI system) also replay these cases" — this is a built-in regression corpus that nextest will run on every CI invocation for free.
  - [Book, "proptest-derive"](https://proptest-rs.github.io/proptest/proptest-derive/getting-started.html): `derive(Arbitrary)` for generating structured values (relevant for protocol payload types).
  - [Book, "Limitations of Property Testing"](https://proptest-rs.github.io/proptest/proptest/limitations.html): honest framing — random sampling, "extremely unlikely to find single-value edge cases in a large space" (hence pairing with fuzzing and targeted unit tests).
- **quickcheck** — [crates.io](https://crates.io/crates/quickcheck): **1.1.0**, released **2026-02-10** (after 1.0.3 in 2021) — i.e. it saw a new release recently; [repo](https://github.com/BurntSushi/quickcheck) has commits through 2026-04 ([release 1.1.0](https://github.com/BurntSushi/quickcheck/releases)). [README](https://github.com/BurntSushi/quickcheck/blob/master/README.md): type-based `Arbitrary` + shrinking. Fine, small, and stable — but strictly less expressive per the proptest book's own comparison.
- **rapid** — **no maintained crate by this name exists in the 2026 ecosystem.** crates.io's `rapid` is an unrelated 2017 console-app crate ([crates.io `rapid`](https://crates.io/crates/rapid), last published 2017-11); a GitHub repository search for a Rust property-based "rapid" library returns no maintained project. Treat "rapid" as not a live option; the real 2026 field is proptest vs quickcheck.

### Best fit for JSON/serde parsing + string/hash invariants

proptest: `Arbitrary` derive on payload structs, string/byte strategies for parser inputs, `proptest!` cases asserting "parse never panics / round-trip is stable / hash invariants hold", with `proptest-regressions/` committed so CI replays historical failures. This is the same pattern the proptest book documents for parser-style code.

### VERDICT

**Best-in-class: proptest 1.11.0 — and it is already a tau workspace dependency (`proptest = "1.11"` in the root `Cargo.toml`).** No switch is warranted: quickcheck 1.1.0 is maintained but less expressive; "rapid" is not a maintained library. Keep proptest.

---

## 5. Supply chain: cargo-deny vs cargo-audit

### cargo-deny (v0.20.2, released 2026-07-09)

- [crates.io `cargo-deny`](https://crates.io/crates/cargo-deny): 0.20.2 (2026-07-09). [Repo](https://github.com/EmbarkStudios/cargo-deny) (EmbarkStudios/cargo-deny) pushed 2026-09-18; [README](https://github.com/EmbarkStudios/cargo-deny/blob/main/README.md): MSRV Rust 1.88, SPDX 3.25.0; `cargo deny init && cargo deny check`; official GitHub Action [`EmbarkStudios/cargo-deny-action`](https://github.com/EmbarkStudios/cargo-deny-action).
- Four checks, each with a [book page](https://embarkstudios.github.io/cargo-deny/):
  - **[advisories](https://embarkstudios.github.io/cargo-deny/checks/advisories/index.html)**: "detect issues for crates by looking in an advisory database" (the RustSec DB by default; unmaintained-crate advisories included) — i.e. it subsumes cargo-audit's core function.
  - **[licenses](https://embarkstudios.github.io/cargo-deny/checks/licenses/index.html)**: "verify that every crate you use has license terms you find acceptable", evaluated as SPDX expressions against your config. [Config reference](https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html): `[licenses] allow = [ ... ]` (SPDX identifiers, `WITH` exceptions supported), `[[licenses.exceptions]]` (per-crate allow), `[[licenses.clarify]]` (manual SPDX expression + license-file hash when metadata is missing), `confidence-threshold` for derived expressions, and **`[licenses.private] ignore = true` — "workspace members will not have their license expression checked if they are not published"**.
  - **[bans](https://embarkstudios.github.io/cargo-deny/checks/bans/index.html)**: "deny (or allow) specific crates, **as well as detect and handle multiple versions of the same crate**" (duplicate detection), plus feature-flag and build-script policy.
  - **[sources](https://embarkstudios.github.io/cargo-deny/checks/sources/index.html)**: crates only from trusted registries/sources.

### cargo-audit (v0.22.2, released 2026-06-05)

- [RustSec `cargo audit` README](https://github.com/RustSec/rustsec/blob/main/cargo-audit/README.md): "Audit your dependencies for crates with security vulnerabilities reported to the RustSec Advisory Database." MSRV 1.74. That is its entire scope — advisories only; no license checking, no duplicate detection.

### Fit for an AGPL-3.0-only repo with a vendored `third_party/`

- The repo's *own* license (AGPL-3.0-only) doesn't constrain its dependencies; the question is which *dependency* licenses are acceptable alongside it. A deny.toml like `[licenses] allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", ...]` (SPDX expressions) plus per-crate `exceptions`/`clarify` entries is the standard shape.
- The vendored `third_party/` dir: its crates are unpublished workspace/path members, so `[licenses.private] ignore = true` (the default `cargo deny init` produces this) keeps cargo-deny from trying to evaluate their license metadata — the same carve-out the existing CI `file-size` job already makes for `./third_party/*`.
- **cargo-deny is a strict superset** of cargo-audit: advisories (same RustSec DB) + licenses + duplicates/bans + sources. Running both would double-report the same advisories.

### VERDICT

**Best-in-class: cargo-deny 0.20.2, alone.** It covers advisories (replacing cargo-audit), license allow-listing with SPDX precision, duplicate-crate detection, and source pinning in one tool with an official GitHub Action. **cargo-audit 0.22.2 is redundant once cargo-deny is in CI** — do not add both.

**Recommended CI config:** one PR job (ubuntu-latest) running `cargo-deny-action` (or `cargo install cargo-deny && cargo deny check`) with a committed `deny.toml`: `[licenses] allow = [...]` + `private.ignore = true`, `[bans] duplicates = "deny"` (warn at first if the current lockfile has dupes), `[advisories]` at defaults.

---

## 6. Best practices: test organization, async tests, negative tests, Miri

### Colocation: same-file unit tests, `tests/` for integration

- [The Rust Book, ch. 11.3 "Test Organization"](https://doc.rust-lang.org/book/ch11-03-test-organization.html) (the language's canonical reference): "You'll put unit tests in the `src` directory **in each file with the code that they're testing**. The convention is to create a module named `tests` in each file … and to annotate the module with `cfg(test)`." Integration tests live in the crate's `tests/` directory and "use your code in the same way any other external code would, using only the public interface."
- Applied to a workspace: keep unit tests colocated per module in each of the 4 crates (fast, can touch private items); put cross-crate/behavioral tests (e.g. `tau-acceptance`-style driver tests, core↔protocol round-trips) in the consuming crate's `tests/`. This is also what the coverage tooling assumes — cargo-llvm-cov excludes `tests/` directories from coverage *reports* by default (its default ignore regex), so colocated unit tests count toward coverage while integration-test harness code doesn't inflate it.

### `#[tokio::test]` vs `#[test]` + `block_on`

- [tokio `#[tokio::test]` API docs](https://docs.rs/tokio/latest/tokio/attr.test.html) (the crate's own documentation): the attribute is first-class; "The default test runtime is single-threaded. Each test gets a separate current-thread runtime"; it shows the manual `#[test] fn … { tokio::runtime::Builder::new_current_thread()…block_on(…) }` only as the "Equivalent code not using `#[tokio::test]`"; `flavor = "multi_thread"` is available "when the `rt-multi-thread` feature flag" is enabled.
- [tokio.rs, "Unit Testing"](https://tokio.rs/tokio/topics/testing) (official topics page): async-specific test ergonomics — e.g. `tokio::time::pause()` to fast-forward time in tests, recommended "for unit tests … to run with paused time throughout".
- Practice: **prefer `#[tokio::test]`** for all async tests (it *is* the block_on form, with less ceremony and a sane single-threaded default); reach for `#[test]` + a manual `Runtime::block_on` only when a test needs runtime configuration the attribute doesn't expose (custom thread counts, specific scheduler settings). For a `rt-multi-thread` workspace, tests that specifically need multi-thread scheduling use `#[tokio::test(flavor = "multi_thread")]`.

### Negative-test patterns

- `#[should_panic]` for code expected to panic — [Rust Book, ch. 11.1](https://doc.rust-lang.org/book/ch11-01-writing-tests.html) ("the test attribute, a few macros, and the should_panic attribute").
- For parsers (the tau-protocol case), the stronger pattern is **asserting on the `Result`, not the panic**: `assert!(serde_json::from_str::<Event>(bad).is_err())` with `matches!` on the error variant — panics in a parser are a bug, so "must not panic on any input" belongs in proptest (§4) and fuzzing (§2), not in `#[should_panic]`.
- Layering: targeted negative unit tests (specific malformed input → specific `Err` variant) ⊂ proptest "never panics / well-formed input round-trips" ⊂ fuzz targets for unbounded input space. Each layer catches what the one below can't (proptest book, "Limitations": random sampling misses single-value edge cases — hence the targeted layer).

### Miri (UB detection) — is it cheap enough for this codebase?

- [Miri README](https://github.com/rust-lang/miri/blob/master/README.md) (rust-lang/miri, official):
  - Install/run: `rustup +nightly component add miri`, then `cargo miri test` — **nightly-only**, always.
  - **Nextest integration is documented and recommended**: `cargo miri nextest run` — "Nextest spawns a separate instance of Miri for each test … Tests can run in parallel … you end up with a full list of which tests worked in Miri and which tests had a problem" (without nextest, one UB finding aborts the whole run).
  - Escape hatch: `cfg(miri)` is set under Miri; the README's own example ignores a tokio test with `#[cfg_attr(miri, ignore)]`.
  - Hard limits (README "caveats"): "Miri currently **does not support networking**. System API support varies between targets"; no FFI; execution is one of many possible interleavings ("will miss bugs that only occur in a different possible execution"); and it's an interpreter — the README notes seed-range runs "can be quite slow".
- Practicality for tau specifically: `tau-core`'s network paths (reqwest/rustls, SSE) and much of its tokio I/O are **unrunnable** under Miri; `tau-app` (Tauri glue) is FFI-heavy and out of scope. The cheap, honest scope is the pure-logic crates — **`tau-protocol`** (serde parsing, payload invariants) and pure `tau-core` modules — as an *optional/nightly* job: `cargo +nightly miri nextest run -p tau-protocol`. That is a small, fast, meaningful job; a whole-workspace Miri gate is not.

### VERDICT

**Conventions:** colocated `#[cfg(test)] mod tests` per file (Rust Book) + `tests/` for cross-crate integration tests; `#[tokio::test]` as the default async test form; `Result`-asserting negative tests for parsers with `#[should_panic]` reserved for genuinely-panicking paths. **Miri:** cheap *only scoped to pure crates* (nightly, `cargo +nightly miri nextest run -p <pure-crate>`); not a PR gate for the workspace in v0.

---

## Conflicts with agreed direction

1. **OSS-Fuzz is not adoptable now (refinement, not a reversal).** The direction assumed OSS-Fuzz ("continuous, free, for public GitHub repos") is appropriate for this AGPL public repo. Primary sources confirm the *license* is a non-issue (AGPL-3.0-only is OSI-approved; OSS-Fuzz's stated disqualifier is "closed source"), but acceptance requires "a significant user base and/or [being] critical to the global IT infrastructure" (oss-fuzz "Accepting New Projects"), which a new repo does not meet — and CIFuzz (the free PR-fuzzing service) explicitly requires prior OSS-Fuzz integration. **Consequence:** implement short-PR + long-nightly fuzzing as self-hosted GitHub Actions jobs (the Rust Fuzz Book's official CI pattern) from day one; file the OSS-Fuzz integration PR later, when the user base justifies it. The tooling choice (cargo-fuzz/libFuzzer) is unchanged, because it is exactly what OSS-Fuzz's Rust path mandates — so nothing is lost by starting self-hosted.
2. **No other conflicts.** Coverage with an 80% aim and no hard floor matches cargo-llvm-cov's model (report + artifact now, `--fail-under-lines` available later). Fuzzing short-PR/long-nightly matches the Rust Fuzz Book's official CI guidance. Snapshot goldens match insta's default CI behavior (fail on new/changed, no config needed). proptest is already the workspace dependency and is the best of the maintained field.

---

## Recommended adoption list

| Tool (current stable) | One-line role | CI placement |
|---|---|---|
| **cargo-llvm-cov 0.9.1** | Line/region coverage over the workspace, reported per crate; 80% is an aim, no `--fail-under` floor | PR job: `cargo llvm-cov --workspace --nextest` (both OSes), publish lcov/HTML artifact (optionally Codecov via `--codecov`) |
| **cargo-fuzz 0.13.2 + libfuzzer-sys 0.4.13** | libFuzzer targets for `tau-protocol` JSON/SSE/`.jsonl` parsers + round-trip invariants | PR: smoke run, `-max_total_time≈300` per target, upload `fuzz/artifacts` on failure; nightly: same targets with a long budget + `cargo fuzz cmin` corpus trim. (OSS-Fuzz + CIFuzz later, once the project qualifies) |
| **insta 1.48.0 (+ cargo-insta)** | Snapshot goldens for protocol payloads, with serde-aware redactions for volatile fields | PR: runs under nextest like any test; new/changed snapshots fail CI by default (`INSTA_UPDATE=auto`) |
| **proptest 1.11.0** (already a dep) | Property tests for parser non-panic, round-trip, and string/hash invariants; `proptest-regressions/` committed as a replayed regression corpus | PR: ordinary tests under nextest (nothing special to wire up) |
| **cargo-deny 0.20.2** | Supply chain in one tool: RustSec advisories, SPDX license allow-list (`[licenses.private] ignore = true` for vendored `third_party/`), duplicate-crate bans, source pinning | PR: one job via `EmbarkStudios/cargo-deny-action` with a committed `deny.toml`. **Do not also add cargo-audit** (strictly subsumed) |
| **Miri (nightly component)** | UB/aliasing checks for pure logic only | Nightly (optional): `cargo +nightly miri nextest run -p tau-protocol` (+ pure `tau-core` modules as they stabilize); not a PR gate in v0 |
| *(already present)* cargo-nextest 0.9.146 | Test runner — the repo's pin (2026-09-21 release) is current; keep it | PR (existing) |

**Suggested rollout order:** (1) insta goldens + proptest parser properties (pure test code, zero new CI surface), (2) cargo-deny job (one workflow file), (3) cargo-llvm-cov report job (no gate), (4) cargo-fuzz PR smoke + nightly long run (new `fuzz/` workspace member), (5) nightly Miri scoped to `tau-protocol`, (6) OSS-Fuzz integration PR when the user base warrants it.
