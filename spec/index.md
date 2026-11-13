# Spec index

Cross-cutting invariants live here and are never restated in a domain:

- Core carries no user-interface logic: no UI crate, no frontend crate. Frontends: `crates/tau-app` (GPUI), `crates/tau-acp`. Proof: gate `core has no UI logic` (M0).
- Every requirement has an allocated `R-<domain>-<NN>` and an acceptance ref: a scenario in `crates/tau-test/tests/features/` tagged with it, or a named gate when the rule constrains a build fact. Proof: spec lint (IDs, refs) and the behaviour lane (M0).
- Wire shapes live in Rust types; `contracts/` is generated, never hand-edited. Proof: gate `contracts current` (M0).
- Real-LLM eval is run by hand when deciding whether a feature is worth building; never in CI. Proof: spec-review check 14.
- Live output is independent of storage: frontends consume core's in-memory event stream, rendering reads no storage, and no storage write, checkpoint, or reader gates emission. Proof: gate `live output under load` (M0).
- A scenario uses only steps in the generated step vocabulary. Proof: gate `steps current` (M0).

## Domains

Counts: `cargo run -p tau-tools --bin spec -- count` (manual until M0).

| Domain | Owns | Spec | Leaves | Lines |
|---|---|---|---|---|
| domain-model | `crates/tau-core/model` | [domain-model/spec.md](domain-model/spec.md) | — | 2 |
| protocol | `crates/tau-protocol` | [protocol/spec.md](protocol/spec.md) | — | 2 |
| session | `crates/tau-core/session`, `crates/tau-core/storage` | [session/spec.md](session/spec.md) | — | 2 |
| context | `crates/tau-core/context` | [context/spec.md](context/spec.md) | — | 2 |
| tools | `crates/tau-core/tools` | [tools/spec.md](tools/spec.md) | — | 2 |
| acp | `crates/tau-acp` | [acp/spec.md](acp/spec.md) | — | 2 |
| surface | `crates/tau-app` | [surface/spec.md](surface/spec.md) | — | 2 |
| harness | `crates/tau-test`, `crates/tau-test/suites`, `crates/tau-tools` | [harness/spec.md](harness/spec.md) | — | 2 |
| config | `crates/tau-core/config` | [config/spec.md](config/spec.md) | — | 2 |
| workspace | root `Cargo.toml`, `crates/`, `.gitignore`, `contracts/`, `rust-toolchain.toml` | [workspace/spec.md](workspace/spec.md) | — | 2 |

Corpus: 0 requirements, 0 / 40k tokens. Each requirement arrives through a change.
