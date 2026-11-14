# adopt_property_lane

## Context

`crates/tau-tools/src/bin/spec.rs` enforces the spec gates: it scans clause headings, finds a clause's byte range for `archive`, and collapses blank runs so `rumdl`'s MD012 stays quiet. Its 15 tests (`mod tests`, same file) are hand-picked inputs. Two of those scans are independent implementations over the same markdown — `clauses_in` (which IDs exist) and `clause_range` (where one lives, used by `apply_delta` at archive time) — and no test states that they agree; a divergence shows up as a refused or misplaced archive.

## Options

- A. `proptest` over the three pure functions in `spec.rs`: the two scans agree on every ID, a `REMOVED` delta leaves every other clause intact, `collapse_blank_runs` is idempotent. Evidence: external — property testing finds inputs hand-picked cases miss: a 2026-05 practitioner report found 13 surviving mutants behind 100% statement coverage, <https://mutating.tech/blog/the-mutation-testing-digest-for-may-2026/> (read 2026-11-14); `proptest` 1.12.0 is the maintained crate at 76M recent downloads, <https://crates.io/crates/proptest> (2026-11-14). Cost: one dev-dependency, no runtime cost, no tokens per turn; 3 properties × 256 cases run in well under a second under nextest.
- B. `cargo-fuzz` on the clause scanner. Evidence: fuzzing finds real Rust memory-safety bugs — 63 crates, 30 known bugs re-discovered and 12 new ones, <https://arxiv.org/abs/2506.15648v2> (IEEE S&P 2026). Cost: a nightly-gated fuzz crate and a harness; the input here is repo-authored markdown, not untrusted bytes, so there is no threat model to buy.
- C. Wait for protocol parsing, then add a lane. Evidence: the corpus is 0 requirements, so nothing parses wire data today. Cost: the spec tool keeps shipping checked only by its 15 examples.

## Decision

A: generate the inputs the 15 examples were picked to be.
Consequences: `proptest` joins `[workspace.dependencies]` as a dev-dependency of `tau-tools`; the properties run inside `spec.rs`'s test module and so fall under the existing `unit + surface` gate. Deferred, each with its trigger: snapshot tests on `contracts/` when `generated()` in `crates/tau-protocol/src/bin/contracts.rs` stops returning an empty set; `loom` when a crate holds concurrent code; `cargo-fuzz` when a parser takes untrusted bytes. Clause lands as R-harness-10 in `spec/harness/spec.md` (childless domain). See spec-authoring.
Eval: none needed

Open: none
