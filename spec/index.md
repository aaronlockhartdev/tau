# Spec index

Cross-cutting invariants live here and are never restated in a domain:

- Core carries no user-interface logic: no UI crate, no frontend crate. Frontends: `crates/tau-app` (GPUI), `crates/tau-acp`. Proof: gate `core has no UI logic` (M0).
- Every requirement has an allocated `R-<domain>-<NN>` and an acceptance ref: a scenario in `crates/tau-test/tests/features/` tagged with it, or a named gate when the rule constrains a build fact. Proof: spec lint (IDs, refs) and the behaviour lane (M0).
- Wire shapes live in Rust types; `contracts/` is generated, never hand-edited. Proof: gate `contracts current` (M0).
- Real-LLM eval is run by hand when deciding whether a feature is worth building; never in CI. Proof: spec-review check 14.
- Live output is independent of storage: frontends consume core's in-memory event stream, rendering reads no storage, and no storage write, checkpoint, or reader gates emission. Proof: gate `live output under load` (M0).
- A scenario uses only steps in the generated step vocabulary. Proof: gate `steps current` (M0).
- `AGENTS.md` carries commands and rules that bind every task; it states no fact a `spec/` file owns. Proof: spec-review check 9.

## Domains

Counts: `cargo run -p tau-tools --bin spec -- count` (manual until M0).

| Domain | Owns | Spec | Leaves |
|---|---|---|---|
| domain-model | `crates/tau-core/model` | [domain-model/spec.md](domain-model/spec.md) | — |
| protocol | `crates/tau-protocol` | [protocol/spec.md](protocol/spec.md) | — |
| session | `crates/tau-core/session`, `crates/tau-core/storage` | [session/spec.md](session/spec.md) | — |
| context | `crates/tau-core/context` | [context/spec.md](context/spec.md) | — |
| tools | `crates/tau-core/tools` | [tools/spec.md](tools/spec.md) | — |
| acp | `crates/tau-acp` | [acp/spec.md](acp/spec.md) | — |
| surface | `crates/tau-app` | [surface/spec.md](surface/spec.md) | — |
| harness | `crates/tau-test`, `crates/tau-test/suites`, `crates/tau-tools` | [harness/spec.md](harness/spec.md) | — |
| config | `crates/tau-core/config` | [config/spec.md](config/spec.md) | — |
| workspace | root `Cargo.toml`, `crates/`, `.gitignore`, `contracts/`, `rust-toolchain.toml`, `docs/stack.md` | [workspace/spec.md](workspace/spec.md) | — |

Corpus: 0 requirements. Each requirement arrives through a change.

## Layout

| Path | What | Read when |
|---|---|---|
| `spec/index.md` | cross-cutting invariants, domain table, corpus count | any spec work |
| `spec/<domain>/spec.md` | domain index: invariants, child links; rules too when the domain has no children | entering a domain |
| `spec/<domain>/<area>/spec.md` | leaf: EARS requirements + acceptance refs | implementing that area |
| `changes/<id>/` | active change: proposal, deltas, tasks | before coding a behaviour change |
| `changes/archive/` | closed changes, the record of *why* | history of a rule |
| `crates/` | workspace members, listed in `spec/workspace/spec.md` | any code |
| `crates/tau-test/tests/features/` | Gherkin acceptance, tagged `@R-…` | writing/reading acceptance |
| `crates/tau-test/tests/features/STEPS.md` | allowed step vocabulary | before writing a scenario |
| `crates/tau-test/suites/` | eval suites, run by hand | deciding whether a feature is worth building |
| `contracts/` | generated wire/tool schemas | touching the wire |
| `.rumdl.toml` | doc-gate tool config | changing a doc gate |
| `docs/glossary.md` | canonical terms | a term is ambiguous |
| `docs/stack.md` | decided dependencies and pins | choosing a dependency, bumping a pin |
| `.agents/skills/`, `.agents/agents/` | skills; subagent definitions | task matches |
