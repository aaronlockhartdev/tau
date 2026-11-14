# adopt_quality_gates

## Context

Every gate in `AGENTS.md` §Gates is a command an agent runs by hand, so a commit that breaks one is caught only when the next agent reads the table; nothing runs on a push. The lint set today is `clippy::all = warn` and `unsafe_code = "forbid"` in `[workspace.lints]` (root `Cargo.toml`), and the `lints` gate row checks `--all-targets` without `--all-features`, so a non-default feature set is unchecked. `Cargo.lock` pins 1.1k lines of dependencies (tokio, cucumber, gherkin and their trees) with no advisory gate, and the storage pin (`rusqlite`, `bundled`) will add SQLite C code no Rust-based advisory tool reads. The corpus is 0 requirements, so no existing `R-` clause is in scope.

## Options

- A. Lint set that denies the bug-catching groups, nextest `ci` profile, `cargo deny check` plus Dependabot as the second advisory source, and a workflow that runs every `run` gate. Evidence: external — RustSec-vs-GitHub drift measured on 2026-10-17: 185 of the 310 RustSec advisories added in 2026 had no GitHub-reviewed entry (so Dependabot is silent on them) and 364 of the 472 GitHub-reviewed crates.io advisories had no RustSec entry (so `cargo audit`/`cargo deny` are silent on them), <https://safeguard.sh/resources/blog/cargo-audit-vs-dependabot-rustsec-github-advisory-gap-2026> → one source alone leaves a hole. External — nextest's own benchmark table: 1.4×–3.4× faster than `cargo test`, more on larger workspaces, <https://nexte.st/docs/benchmarks/> (read 2026-11-14). External — Clippy study over 94,715 crates: mean 21 warnings/KLOC, and developers decline to gate on Clippy because of false positives and missing auto-fix, <https://arxiv.org/abs/2310.11738> (2023-10-18) → the deny-set is scoped to `correctness`/`suspicious`/`perf`/panic lints, style groups stay `warn`. Cost: cargo-deny and cargo-nextest install as prebuilt binaries; rumdl and lychee build from source in CI (first run ~5 min, cached after); no runtime cost, no tokens per turn.
- B. Configs only, no workflow: lint set, `deny.toml`, `.config/nextest.toml`. Evidence: the Clippy and nextest evidence above applies unchanged. Cost: cheapest, but nothing runs on a push, and Dependabot needs `.github/` to exist anyway.
- C. Tier-1 plus `cargo-machete`, `cargo-hakari`, `cargo-llvm-cov`, `cargo-mutants`, `cargo-fuzz`. Evidence: `cargo-machete` 0.9.2 and `cargo-hakari` 0.9.39 are maintained (crates.io, 2026-11-14). Cost: five more tools, three of which have no target in this repo today — `contracts/` generates nothing, no crate has concurrent code, no crate parses untrusted bytes.

## Decision

A: gates become runnable by a machine, not only by an agent reading a table.
Consequences: `.github/workflows/gates.yml` installs what the commands need and runs one lane step, `cargo run -p tau-tools --bin gates`, which reads the gate table — so a gate is defined once, in `AGENTS.md`, and the lane follows it; `deny.toml` carries a license allow list that a new dependency may need extended; `.config/nextest.toml` and `.github/` join the layout table in `spec/index.md`; the pins (`cargo-deny` 0.20.2, nextest `ci` profile, Dependabot as the second advisory source) join `docs/stack.md`. Clauses land as R-workspace-09..13 in `spec/workspace/spec.md` (childless domain). Layout rows go into `spec/index.md` in place. See spec-authoring.
Eval: none needed

Open: none
