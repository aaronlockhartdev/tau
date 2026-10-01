# Research: Rust coding & architectural practice for the tau refactor

Researched 2026-10-01. All claims below are cited to primary sources fetched during this task (official
Rust docs, the API Guidelines, first-party crate READMEs/docs, crates.io release metadata). Scope: Rust
practice for the refactor — `crates/tau-core` (27.8k LOC, 14 top-level modules), `crates/tau-protocol`
(2.8k LOC, the serde IPC contract), and the small crates. Testing/CI tooling is out of scope
(`docs/research/testing-rust.md` covers it; nothing here relitigates the adopted set: clippy `-D warnings`,
cargo-deny, Miri, cargo-fuzz, insta, proptest, cargo-llvm-cov, the 500/1000 LOC gate). Tauri is a separate
agent's document.

**Question.** What enforceable coding + architectural practices does the official Rust guidance recommend
for this codebase, and which code-smell/static-analysis tools beyond the adopted set are worth adding?

**Short answer.** Four candidate AGENTS.md rules survive contact with the primary sources: conversion
naming (`as_`/`to_`/`into_`), no `get_`-prefixed getters, "meaningful error types, never `()`", and
"eager common-trait impls on public types" — all from the Rust API Guidelines, mostly already followed by
the code (the rules' value is keeping new code compliant, especially at the `tau-protocol` seam). One
mechanical win: **thiserror 2.0.21** replaces the six hand-rolled error enums' boilerplate (7 manual
`Display` + 6 manual `From` impls today). Tooling: five small additions with adopt verdicts below
(clippy pedantic cherry-picks, `clippy.toml` msrv, cargo-machete nightly, ASan/TSan nightly test run,
typos on PRs) and two explicit skips (cargo-udeps, cargo-semver-checks).

---

## 1. Candidate AGENTS.md rules (coding practice)

Source base: the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/about.html) — "authored
largely by the Rust library team, based on experiences building the Rust standard library"; "crates that
conform well to these guidelines integrate better with the existing crate ecosystem." The
[Style Guide](https://doc.rust-lang.org/style-guide/) is the reference rustfmt already implements, so it
adds no rules beyond the existing "rustfmt defaults" line.

### 1.1 Conversions follow `as_`, `to_`, `into_` (C-CONV) — **ADOPT**

- [API Guidelines, Naming → C-CONV](https://rust-lang.github.io/api-guidelines/naming.html#c-conv):
  conversions are methods prefixed `as_` (free, borrowed→borrowed), `to_` (expensive), `into_`
  (owned→owned, deconstructing); "a type that wraps a single value … should provide access to the wrapped
  value by an `into_inner()` method."
- Codebase state: already compliant in the sample checked — e.g. `pub fn as_str(&self) -> &'static str`
  at `crates/tau-core/src/subagent.rs:50,68` (free borrowed→borrowed ⇒ `as_` is the correct prefix).
- Rule wording: "Conversion methods are named `as_*` (free view), `to_*` (allocating/computing),
  `into_*` (consuming); single-value wrappers expose `into_inner()`."

### 1.2 No `get_`-prefixed getters (C-GETTER) — **ADOPT**

- [API Guidelines, Naming → C-GETTER](https://rust-lang.github.io/api-guidelines/naming.html#c-getter):
  "With a few exceptions, the `get_` prefix is not used for getters in Rust code."
- Codebase state: zero `pub fn get_` methods across `crates/tau-core` + `crates/tau-protocol` today — the
  rule's job is to keep new code (and the refactor's new seams) compliant.
- Rule wording: "Getters are bare names (`id()`, `count()`); no `get_` prefix."

### 1.3 Error types are meaningful; never `()` (C-GOOD-ERR) — **ADOPT (message-style half is new)**

- [API Guidelines, Interoperability → C-GOOD-ERR](https://rust-lang.github.io/api-guidelines/interoperability.html#c-good-err):
  error types "should always implement `std::error::Error`" and `Send + Sync`; "Never use `()` as an
  error type … define a meaningful error type specific to your crate"; "The error message given by the
  `Display` representation … should be lowercase without trailing punctuation, and typically concise."
- Codebase state: six hand-rolled public error enums carry the `Display` impls
  (`session.rs:206`, `config.rs:318`, `provider.rs:28`, `om_integration.rs:175`, `hashline.rs:38`,
  `agent.rs:41`); no `Result<_, ()>` found. The lowercase/no-trailing-punctuation message style is the
  part not currently codified anywhere.
- Rule wording: "Every error type implements `std::error::Error` + `Display`; `Display` messages are
  lowercase, concise, without trailing punctuation; `Result<T, ()>` is not an error design."

### 1.4 Public types eagerly implement common traits (C-COMMON-TRAITS) — **ADOPT (scoped to `tau-protocol`)**

- [API Guidelines, Interoperability → C-COMMON-TRAITS](https://rust-lang.github.io/api-guidelines/interoperability.html#c-common-traits):
  the orphan rule means "crates that define new types should eagerly implement all applicable, common
  traits" (`Debug`, `Clone`, `Eq`/`PartialEq`, `Hash`, `Display`, `Default`, …).
- Codebase state: applies most at the `tau-protocol` seam (the serde payload/event types the GUI mirrors)
  and to `tau-core`'s public surface; internal helper types are out of scope — that keeps the rule
  enforceable rather than aspirational.
- Rule wording: "Types on a public surface (protocol payloads/events, `tau-core` public API) implement the
  common std traits that apply: `Debug`, `Clone`, `PartialEq`/`Eq`, `Hash`, `Default` where meaningful."

### Considered and **skipped**

- **C-CASE (casing)** — [naming.html#c-case](https://rust-lang.github.io/api-guidelines/naming.html#c-case):
  already enforced by the compiler's naming conventions; a rule line would be a no-op.
- **C-WORD-ORDER (consistent word order)** — [naming.html#c-word-order](https://rust-lang.github.io/api-guidelines/naming.html):
  the guideline is vague ("names use a consistent word order"); review-judgement material, not an
  enforceable rule.
- **C-ITER / C-ITER-TY (iterator naming)** — [naming.html#c-iter](https://rust-lang.github.io/api-guidelines/naming.html#c-iter):
  the codebase exposes almost no public iterators; the rule would govern a surface that doesn't exist.
  Revisit if the refactor creates public collection types.
- **`unwrap`/`expect` in production code** — 289 occurrences in non-test `tau-core` src (e.g.
  `session.rs:141` uses a *justified* `expect("Entry is always serializable")`). No official primary
  source was found that makes "no unwrap in production" a rule (the API Guidelines'
  [C-QUESTION-MARK](https://rust-lang.github.io/api-guidelines/documentation.html#c-question-mark)
  addresses docs examples, not production code). Left to review judgement; flagged here so the refactor
  doesn't accidentally codify an unsourced rule.

---

## 2. Tooling candidates beyond the adopted set

### 2.1 Clippy `pedantic` cherry-picks — **ADOPT (selective)**

- [Clippy's Lints](https://doc.rust-lang.org/stable/clippy/lints.html): the `pedantic` group is "for
  Clippy power users that want an in depth check of their code"; the official note: "Instead of enabling
  the whole group (like Clippy itself does), you may want to **cherry-pick** lints out of the pedantic
  group"; "expect to also use `#[allow]` attributes generously". The `restriction` group: "It is **not**
  recommended to enable the whole group, but rather cherry-pick lints that are useful for your code base."
- Fit: the existing `clippy -D warnings` gate is the vehicle; the refactor turns on a short named list of
  pedantic lints per crate, chosen at adoption time from the pedantic group,
  with explicit `#[allow]` where a lint is deliberately not wanted. Cost: one-time `#[allow]` churn; zero new CI time.

### 2.2 `clippy.toml` with `msrv` — **ADOPT (trivial)**

- [Clippy Configuration](https://doc.rust-lang.org/stable/clippy/configuration.html): `msrv` in
  `clippy.toml` makes MSRV-aware lints behave correctly — "Some lints change their behavior depending on
  the configured MSRV … Clippy may suppress a lint entirely to avoid suggesting APIs or syntax
  unavailable for the configured MSRV."
- Fit: the workspace pins `1.98.1` (`rust-toolchain.toml`); declaring it to clippy is one file, zero CI
  cost, and prevents lint suggestions that would break a pinned-toolchain build.

### 2.3 cargo-machete (unused dependencies) — **ADOPT (nightly, informational)**

- [cargo-machete README](https://github.com/bnjbvr/cargo-machete): "detects unused dependencies in Rust
  projects, in a **fast (yet imprecise)** way"; stable-compatible (`cargo install cargo-machete`).
  v0.9.2 on [crates.io](https://crates.io/crates/cargo-machete) (2026-04-15).
- Fit: runs in the existing nightly job, **not** as a PR gate — the self-declared imprecision (it scans
  source text, so build-script-only or macro-generated uses false-positive) means its output is a
  cleanup hint, and a red gate on imprecise data would be a false-alarm factory. Cost: seconds.

### 2.4 Sanitizer test run (ASan/TSan) — **ADOPT (nightly, x86_64 Linux)**

- [rust-san](https://github.com/japaric/rust-san) (the canonical how-to): sanitize via
  `RUSTFLAGS="-Z sanitizer=address" cargo test --target x86_64-unknown-linux-gnu`; "sanitizer support is
  available on x86_64 Linux and on x86_64 macOS (ASan and TSan only)"; `-Z` ⇒ nightly.
- Fit: the nightly job already exists and already runs Miri on `tau-protocol`; an ASan/TSan `cargo test`
  of the workspace on the x86_64 Linux runner extends UB/memory-error detection from "pure crates only"
  to the whole workspace (tokio, serde, the session/JSONL I/O paths Miri can't reach). Cost: one nightly
  leg, ~a full test-run duration on one runner — no PR impact.

### 2.5 typos (spelling in source) — **ADOPT (PR gate)**

- [crate-ci/typos README](https://github.com/crate-ci/typos): "Source code spell checker — Finds and
  corrects spelling mistakes among source code: Fast enough to run on monorepos; **Low false positives
  so you can run on PRs**." v1.50.3 on [crates.io](https://crates.io/crates/typos-cli) (2026-09-25).
- Fit: the repo is comment-dense by its own anti-slop discipline, so typo density in identifiers/comments
  is a live quality signal; the tool's own positioning is exactly the PR-gate use. Cost: seconds per PR.

### Considered and **skipped**

- **cargo-udeps** — [README](https://github.com/est31/cargo-udeps): "needs Rust nightly to actually run"
  (type-checking based). Overlaps cargo-machete's job with a heavier mechanism and a nightly requirement;
  one unused-dep tool is enough — machete (stable, fast) wins on cost.
- **cargo-semver-checks** — [Rust Project Goals](https://rust-lang.github.io/rust-project-goals/2024h2/cargo-semver-checks.html):
  "a linter for semantic versioning (SemVer) in Rust" — i.e. for **published** library crates checking
  compatibility across releases. Tau is an application workspace (nothing published; `0.1.0` internal
  crates); the tool has no surface to lint.
- **cargo-audit** — subsumed by the adopted cargo-deny (advisories + licenses + bans in one tool);
  decided in `docs/research/testing-rust.md`, restated here only so the shortlist is complete.

---

## 3. Architectural scope — what the primary sources actually cover

Honesty note: the official Rust sources are strong on *coding* practice (style, naming, error design,
trait design) and on **public API** design (the API Guidelines' stated domain), but thin on *internal*
module architecture — there is no authoritative "how to decompose an internal crate" document to cite.
What the sources do bind, at tau's seams:

1. **`tau-protocol` is the one API the Guidelines govern directly.** It is a published-to-the-GUI
   contract (the TS mirror in `app/svelte/lib/protocol.ts` must not drift — `app/svelte/scripts/protocol-parity.mjs`
   enforces that). C-GOOD-ERR (`ProtocolError` at `crates/tau-protocol/src/payload.rs:85`),
   C-COMMON-TRAITS (payload/event types), and C-CONV naming apply to it with full force; the refactor
   should treat protocol changes as API changes (the guidelines' review posture: the
   [checklist](https://rust-lang.github.io/api-guidelines/checklist.html) is "suitable for quick scanning
   during crate reviews").
2. **Internal architecture is already settled** by the v0 clean-up (module splits, the store
   decomposition — `docs/v0-cleanup-roadmap.md`) and the file-size gate; the primary sources add nothing
   enforceable beyond what AGENTS.md already says. The refactor's architectural work is *applying* the
   §1 rules at the seams, not re-decomposing modules.

## Sources

- Rust Style Guide: https://doc.rust-lang.org/style-guide/ (and [RFC 2436](https://rust-lang.github.io/rfcs/2436-style-guide.html))
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/about.html · [checklist](https://rust-lang.github.io/api-guidelines/checklist.html) · [naming](https://rust-lang.github.io/api-guidelines/naming.html) · [interoperability](https://rust-lang.github.io/api-guidelines/interoperability.html) · [documentation](https://rust-lang.github.io/api-guidelines/documentation.html)
- Clippy: [lint groups](https://doc.rust-lang.org/stable/clippy/lints.html) · [configuration](https://doc.rust-lang.org/stable/clippy/configuration.html)
- thiserror: https://github.com/dtolnay/thiserror · https://crates.io/crates/thiserror (2.0.21, 2026-09-23)
- anyhow: https://crates.io/crates/anyhow (1.0.104, 2026-07-18)
- cargo-machete: https://github.com/bnjbvr/cargo-machete · https://crates.io/crates/cargo-machete (0.9.2, 2026-04-15)
- cargo-udeps: https://github.com/est31/cargo-udeps
- typos: https://github.com/crate-ci/typos · https://crates.io/crates/typos-cli (1.50.3, 2026-09-25)
- cargo-semver-checks: https://rust-lang.github.io/rust-project-goals/2024h2/cargo-semver-checks.html
- rust-san (sanitizers): https://github.com/japaric/rust-san
